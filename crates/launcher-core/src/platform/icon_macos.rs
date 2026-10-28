//! macOS 图标抓取：从 `.app` 包里取图标并落 PNG 缓存。
//!
//! macOS 上"一个应用的图标"属于它所在的 `.app` 包，而不是可执行文件本身：
//! `Contents/Info.plist` 的 `CFBundleIconFile` 指向 `Contents/Resources/` 下的 `.icns`。
//! 所以三档来源都归结为"找到 .app 包，读它的 icns"：
//!
//! | 档位 | 来源 |
//! | --- | --- |
//! | Custom | 配置里的 `icon`：`.png` / `.icns`，或直接指向一个 `.app` |
//! | Window | 运行中进程树里第一个位于 `.app` 包内的进程（GUI 应用真正的图标） |
//! | Project | 工作目录里的打包配置 / 常见图标文件（见 `project_icon`） |
//! | Exe | 启动命令本身位于某个 `.app` 包内 |
//!
//! 例外：位于 `node_modules` 里的包（Electron / NW.js 开发态）只给 Exe 档位，并排在项目图标之后。
//! 开发态下进程是 `node_modules/electron/dist/Electron.app`，拿到的是 Electron 默认的原子图标；
//! 应用在运行时用 `app.dock.setIcon()` 设的图标其它进程读不到。给它低档位，
//! 避免把这张"不是这个应用"的图当成高可信度缓存固化下来。
//!
//! 本模块不含 `unsafe`：进程可执行路径用 `ps -o comm=`（macOS 上输出完整路径），
//! 二进制 / XML 两种 plist 都交给系统自带的 `plutil` 解析。

use std::collections::{HashMap, VecDeque};
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};

use crate::platform::icon::{
    load_image_file, resolve_and_cache, resolve_and_cache_forced, CapturedIcon, IconSource,
    IconTier, RgbaImage,
};
use crate::platform::project_icon::project_icon;
use crate::platform::{IconCapture, IconTarget};

/// 路径所在的最外层 `.app` 包（纯函数）。
///
/// 取**最外层**：Electron 的渲染进程位于
/// `Electron.app/Contents/Frameworks/Electron Helper.app/...`，用户看到的图标是外层的。
fn enclosing_bundle(path: &Path) -> Option<PathBuf> {
    let mut bundle = PathBuf::new();
    for component in path.components() {
        bundle.push(component);
        let is_app = component
            .as_os_str()
            .to_str()
            .is_some_and(|name| name.len() > 4 && name.to_ascii_lowercase().ends_with(".app"));
        if is_app {
            return Some(bundle);
        }
    }
    None
}

/// 是否是开发态依赖里的包（`node_modules` 下的 Electron / NW.js）。
fn is_dev_dependency_bundle(bundle: &Path) -> bool {
    bundle.components().any(|c| c.as_os_str() == "node_modules")
}

/// 用 `plutil` 读 Info.plist 的一个字符串键；读不到返回 `None`。
fn plist_string(plist: &Path, key: &str) -> Option<String> {
    let output = Command::new("/usr/bin/plutil")
        .args(["-extract", key, "raw", "-o", "-"])
        .arg(plist)
        .stdin(Stdio::null())
        .stderr(Stdio::null())
        .output()
        .ok()?;
    if !output.status.success() {
        return None;
    }
    let value = String::from_utf8(output.stdout).ok()?.trim().to_string();
    (!value.is_empty()).then_some(value)
}

/// 包的 icns 路径：`CFBundleIconFile`（可省略 `.icns` 后缀），否则退到 Resources 里的第一个 icns。
///
/// 只声明了 `CFBundleIconName`（图标在 Assets.car 资源目录里）的新式应用，
/// Resources 里通常仍保留一份 `AppIcon.icns` 兼容旧系统，退路能覆盖到大多数情况。
fn bundle_icns_path(bundle: &Path) -> Option<PathBuf> {
    let resources = bundle.join("Contents/Resources");
    if let Some(name) = plist_string(&bundle.join("Contents/Info.plist"), "CFBundleIconFile") {
        let file = if Path::new(&name).extension().is_some() {
            name
        } else {
            format!("{name}.icns")
        };
        let candidate = resources.join(file);
        if candidate.is_file() {
            return Some(candidate);
        }
    }
    let mut icns: Vec<PathBuf> = std::fs::read_dir(&resources)
        .ok()?
        .flatten()
        .map(|entry| entry.path())
        .filter(|path| {
            path.extension()
                .is_some_and(|ext| ext.eq_ignore_ascii_case("icns"))
        })
        .collect();
    // 有 AppIcon.icns 就用它，否则按文件名取第一个，保证结果稳定
    icns.sort();
    icns.iter()
        .find(|path| path.file_stem().is_some_and(|stem| stem == "AppIcon"))
        .or_else(|| icns.first())
        .cloned()
}

/// 读取一个 `.app` 包的图标。
fn bundle_icon(bundle: &Path) -> Option<RgbaImage> {
    load_image_file(&bundle_icns_path(bundle)?).filter(|image| !image.is_fully_transparent())
}

/// 配置里的自定义图标：图片文件或 `.app` 包。
fn custom_icon(path: &Path) -> Option<RgbaImage> {
    if path.is_dir() {
        return enclosing_bundle(path).and_then(|bundle| bundle_icon(&bundle));
    }
    load_image_file(path).filter(|image| !image.is_fully_transparent())
}

/// 解析 `ps -A -o pid=,ppid=,comm=` 的输出（纯函数）：comm 是完整路径，可能含空格。
fn parse_ps_table(text: &str) -> Vec<(u32, u32, PathBuf)> {
    text.lines()
        .filter_map(|line| {
            let line = line.trim_start();
            let (pid, rest) = line.split_once(char::is_whitespace)?;
            let rest = rest.trim_start();
            let (ppid, comm) = rest.split_once(char::is_whitespace)?;
            Some((
                pid.parse().ok()?,
                ppid.parse().ok()?,
                PathBuf::from(comm.trim()),
            ))
        })
        .collect()
}

/// 进程树（根在前，按层序）里各进程的可执行文件路径。
fn tree_executables(root: u32) -> Vec<PathBuf> {
    let Ok(output) = Command::new("/bin/ps")
        .args(["-A", "-o", "pid=,ppid=,comm="])
        .stdin(Stdio::null())
        .stderr(Stdio::null())
        .output()
    else {
        return Vec::new();
    };
    let table = parse_ps_table(&String::from_utf8_lossy(&output.stdout));
    let mut by_parent: HashMap<u32, Vec<u32>> = HashMap::new();
    let mut exe: HashMap<u32, PathBuf> = HashMap::new();
    for (pid, ppid, comm) in table {
        if pid != ppid {
            by_parent.entry(ppid).or_default().push(pid);
        }
        exe.insert(pid, comm);
    }
    let mut out = Vec::new();
    let mut seen = std::collections::HashSet::new();
    let mut queue = VecDeque::from([root]);
    while let Some(pid) = queue.pop_front() {
        if !seen.insert(pid) {
            continue;
        }
        if let Some(path) = exe.get(&pid) {
            out.push(path.clone());
        }
        queue.extend(by_parent.get(&pid).into_iter().flatten().copied());
    }
    out
}

/// macOS 图标捕获实现。
#[derive(Debug, Default, Clone, Copy)]
pub struct MacIconCapture;

impl MacIconCapture {
    /// 构造图标捕获实现。
    pub fn new() -> Self {
        Self
    }
}

impl IconSource for MacIconCapture {
    /// 优先级链：自定义图标 → 运行中进程所在的 .app → 项目图标 → 开发依赖里的 .app → 启动命令所在的 .app。
    ///
    /// 每一档"失败即继续"：自定义图标路径失效不该连带让包图标也拿不到。
    fn produce(&self, target: &IconTarget) -> Option<CapturedIcon> {
        if let Some(custom) = target.custom_icon.as_deref() {
            if let Some(image) = custom_icon(custom) {
                return Some(CapturedIcon::new(image, IconTier::Custom));
            }
        }

        // 运行中进程所在的 .app：真正的 GUI 应用直接用；node_modules 里的 Electron 先记下，
        // 排在项目图标之后——开发态下项目里的正式图标才是这个应用的图标。
        let mut dev_bundle_icon = None;
        if let Some(pid) = target.pid {
            for executable in tree_executables(pid) {
                let Some(bundle) = enclosing_bundle(&executable) else {
                    continue;
                };
                if is_dev_dependency_bundle(&bundle) {
                    if dev_bundle_icon.is_none() {
                        dev_bundle_icon = bundle_icon(&bundle);
                    }
                    continue;
                }
                if let Some(image) = bundle_icon(&bundle) {
                    return Some(CapturedIcon::new(image, IconTier::Window));
                }
            }
        }

        if let Some(image) = target.project_dir.as_deref().and_then(project_icon) {
            return Some(CapturedIcon::new(image, IconTier::Project));
        }

        if let Some(image) = dev_bundle_icon {
            return Some(CapturedIcon::new(image, IconTier::Exe));
        }

        let bundle = target.executable.as_deref().and_then(enclosing_bundle)?;
        bundle_icon(&bundle).map(|image| CapturedIcon::new(image, IconTier::Exe))
    }
}

impl IconCapture for MacIconCapture {
    fn capture(&self, target: &IconTarget) -> Option<PathBuf> {
        let icons_dir = crate::paths::icons_dir().ok()?;
        resolve_and_cache(self, &icons_dir, target)
    }

    fn recapture(&self, target: &IconTarget) -> Option<PathBuf> {
        let icons_dir = crate::paths::icons_dir().ok()?;
        resolve_and_cache_forced(self, &icons_dir, target)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn outermost_app_bundle_is_found() {
        let helper = Path::new(
            "/x/node_modules/electron/dist/Electron.app/Contents/Frameworks/Electron Helper.app/Contents/MacOS/Electron Helper",
        );
        assert_eq!(
            enclosing_bundle(helper),
            Some(PathBuf::from("/x/node_modules/electron/dist/Electron.app"))
        );
        assert_eq!(enclosing_bundle(Path::new("/usr/local/bin/node")), None);
        assert!(is_dev_dependency_bundle(
            &enclosing_bundle(helper).expect("应找到包")
        ));
        assert!(!is_dev_dependency_bundle(Path::new(
            "/Applications/Safari.app"
        )));
    }

    #[test]
    fn ps_table_keeps_paths_with_spaces() {
        let rows = parse_ps_table(
            "  1     0 /sbin/launchd\n 501   1 /Applications/Google Chrome.app/Contents/MacOS/Google Chrome\nbad\n",
        );
        assert_eq!(rows.len(), 2);
        assert_eq!(
            rows[1],
            (
                501,
                1,
                PathBuf::from("/Applications/Google Chrome.app/Contents/MacOS/Google Chrome")
            )
        );
    }

    /// 系统自带应用一定有 icns：真实地走一遍 plist → icns → PNG 解码。
    #[test]
    fn system_app_bundle_icon_decodes() {
        let finder = Path::new("/System/Library/CoreServices/Finder.app");
        if !finder.is_dir() {
            return;
        }
        let image = bundle_icon(finder).expect("Finder.app 应能解出图标");
        assert!(
            image.width >= 128,
            "应取到大尺寸条目，实际 {}px",
            image.width
        );
        // 自定义图标直接指向 .app 也应可用
        assert!(custom_icon(finder).is_some());
    }

    /// 运行中进程档位：Finder 常驻，按它的 pid 应取到 Finder.app 的图标，且是 Window 档位。
    #[test]
    fn running_app_process_yields_window_tier() {
        let Some(pid) = Command::new("/usr/bin/pgrep")
            .args(["-x", "Finder"])
            .output()
            .ok()
            .and_then(|out| String::from_utf8(out.stdout).ok())
            .and_then(|text| {
                text.lines()
                    .next()
                    .and_then(|line| line.trim().parse().ok())
            })
        else {
            return;
        };
        let mut target = IconTarget::default();
        target.pid = Some(pid);
        let captured = MacIconCapture::new()
            .produce(&target)
            .expect("应取到 Finder 图标");
        assert_eq!(captured.tier, IconTier::Window);
    }

    /// 开发态：项目目录里声明了图标时用项目图标（Project 档），而不是 Electron 的默认图标。
    #[test]
    fn project_icon_beats_electron_dev_bundle() {
        let dir = tempfile::tempdir().expect("临时目录失败");
        let finder_icns = bundle_icns_path(Path::new("/System/Library/CoreServices/Finder.app"));
        let Some(finder_icns) = finder_icns else {
            return;
        };
        std::fs::create_dir_all(dir.path().join("build")).expect("建目录失败");
        std::fs::copy(&finder_icns, dir.path().join("build/icon.icns")).expect("复制 icns 失败");
        std::fs::write(
            dir.path().join("electron-builder.yml"),
            "mac:\n  icon: build/icon.icns\n",
        )
        .expect("写配置失败");

        let target = IconTarget::default().with_project_dir(Some(dir.path().to_path_buf()));
        let captured = MacIconCapture::new()
            .produce(&target)
            .expect("应取到项目图标");
        assert_eq!(captured.tier, IconTier::Project);
    }

    /// 运行中进程档位：自身测试进程不在 .app 里，应退到下一档而不是报错。
    #[test]
    fn running_process_outside_bundle_yields_none() {
        let mut target = IconTarget::default();
        target.pid = Some(std::process::id());
        assert!(MacIconCapture::new().produce(&target).is_none());
    }
}
