//! macOS `.app` 包的通用读取：定位包、读 `Info.plist`、解析真正的可执行文件。
//!
//! 图标抓取（[`crate::platform::icon_macos`]）、直接启动（把 `Foo.app` 解析成
//! `Foo.app/Contents/MacOS/<CFBundleExecutable>`）、「像 Dock 一样打开」三处共用。
//!
//! 不含 `unsafe`：plist 交给系统自带的 `plutil` 解析（二进制 / XML 两种格式都支持）。

use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};

/// 路径的最后一段是否以 `.app` 结尾（纯字符串判断，不访问文件系统）。
pub fn has_app_extension(path: &Path) -> bool {
    path.file_name()
        .and_then(|name| name.to_str())
        .is_some_and(|name| name.len() > 4 && name.to_ascii_lowercase().ends_with(".app"))
}

/// 路径本身是否是一个 `.app` 包（扩展名为 `.app` 且是目录）。
pub fn is_app_bundle(path: &Path) -> bool {
    has_app_extension(path) && path.is_dir()
}

/// 路径所在的最外层 `.app` 包（纯函数）。
///
/// 取**最外层**：Electron 的渲染进程位于
/// `Electron.app/Contents/Frameworks/Electron Helper.app/...`，用户看到的是外层应用。
/// 路径本身就是 `Foo.app` 时返回它自己。
pub fn enclosing_bundle(path: &Path) -> Option<PathBuf> {
    let mut bundle = PathBuf::new();
    for component in path.components() {
        bundle.push(component);
        if has_app_extension(Path::new(component.as_os_str())) {
            return Some(bundle);
        }
    }
    None
}

/// 用 `plutil` 读 plist 的一个字符串键；读不到返回 `None`。
pub fn plist_string(plist: &Path, key: &str) -> Option<String> {
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

/// 包的 `Info.plist` 路径。
fn info_plist(bundle: &Path) -> PathBuf {
    bundle.join("Contents/Info.plist")
}

/// 包名去掉 `.app` 后缀（`/Applications/Foo Bar.app` → `Foo Bar`）。
fn bundle_stem(bundle: &Path) -> Option<String> {
    let name = bundle.file_name()?.to_str()?;
    let stem = &name[..name.len().checked_sub(4)?];
    (!stem.is_empty()).then(|| stem.to_string())
}

/// 包里真正的可执行文件：`Contents/MacOS/<CFBundleExecutable>`。
///
/// `Info.plist` 读不到该键时退到与包同名的文件（老式包的约定），
/// 再不行、而 `Contents/MacOS` 里恰好只有一个文件时用它；都不满足返回 `None`。
pub fn bundle_executable(bundle: &Path) -> Option<PathBuf> {
    let macos_dir = bundle.join("Contents/MacOS");
    if let Some(name) = plist_string(&info_plist(bundle), "CFBundleExecutable") {
        let candidate = macos_dir.join(name);
        if candidate.is_file() {
            return Some(candidate);
        }
    }
    if let Some(stem) = bundle_stem(bundle) {
        let candidate = macos_dir.join(stem);
        if candidate.is_file() {
            return Some(candidate);
        }
    }
    let mut files = std::fs::read_dir(&macos_dir)
        .ok()?
        .flatten()
        .map(|entry| entry.path())
        .filter(|path| path.is_file());
    let only = files.next()?;
    files.next().is_none().then_some(only)
}

/// 包的展示名：`CFBundleDisplayName` → `CFBundleName` → 包名去掉 `.app`。
pub fn bundle_display_name(bundle: &Path) -> Option<String> {
    let plist = info_plist(bundle);
    plist_string(&plist, "CFBundleDisplayName")
        .or_else(|| plist_string(&plist, "CFBundleName"))
        .or_else(|| bundle_stem(bundle))
}

#[cfg(test)]
mod tests {
    use super::*;

    /// 在临时目录里造一个最小的 `.app` 包。
    fn fake_bundle(root: &Path, name: &str, executable: &str, plist_exec: Option<&str>) -> PathBuf {
        let bundle = root.join(name);
        let macos = bundle.join("Contents/MacOS");
        std::fs::create_dir_all(&macos).expect("建包目录失败");
        std::fs::write(macos.join(executable), "#!/bin/sh\n").expect("写可执行文件失败");
        let mut dict = String::from("<key>CFBundleName</key><string>Plist Name</string>");
        if let Some(exec) = plist_exec {
            dict.push_str(&format!(
                "<key>CFBundleExecutable</key><string>{exec}</string>"
            ));
        }
        std::fs::write(
            bundle.join("Contents/Info.plist"),
            format!(
                "<?xml version=\"1.0\" encoding=\"UTF-8\"?>\
                 <plist version=\"1.0\"><dict>{dict}</dict></plist>"
            ),
        )
        .expect("写 Info.plist 失败");
        bundle
    }

    #[test]
    fn enclosing_bundle_takes_outermost_and_accepts_bundle_itself() {
        let helper = Path::new(
            "/x/Electron.app/Contents/Frameworks/Electron Helper.app/Contents/MacOS/Electron Helper",
        );
        assert_eq!(
            enclosing_bundle(helper),
            Some(PathBuf::from("/x/Electron.app"))
        );
        assert_eq!(
            enclosing_bundle(Path::new("/Applications/Foo.app")),
            Some(PathBuf::from("/Applications/Foo.app"))
        );
        assert_eq!(enclosing_bundle(Path::new("/usr/local/bin/node")), None);
        // 只有 ".app" 本身不算包
        assert_eq!(enclosing_bundle(Path::new("/x/.app/y")), None);
    }

    #[test]
    fn executable_comes_from_plist_then_falls_back() {
        let dir = tempfile::tempdir().expect("临时目录");

        let declared = fake_bundle(dir.path(), "Declared.app", "real-bin", Some("real-bin"));
        assert_eq!(
            bundle_executable(&declared),
            Some(declared.join("Contents/MacOS/real-bin"))
        );

        // plist 没写 CFBundleExecutable：退到与包同名的文件
        let same_name = fake_bundle(dir.path(), "Same Name.app", "Same Name", None);
        assert_eq!(
            bundle_executable(&same_name),
            Some(same_name.join("Contents/MacOS/Same Name"))
        );

        // 都对不上但只有一个文件：用它
        let single = fake_bundle(dir.path(), "Single.app", "whatever", Some("missing"));
        assert_eq!(
            bundle_executable(&single),
            Some(single.join("Contents/MacOS/whatever"))
        );

        assert!(is_app_bundle(&declared));
        assert!(!is_app_bundle(&declared.join("Contents")));
        assert_eq!(bundle_executable(dir.path()), None);
    }

    #[test]
    fn display_name_prefers_plist_and_falls_back_to_bundle_name() {
        let dir = tempfile::tempdir().expect("临时目录");
        let bundle = fake_bundle(dir.path(), "Foo.app", "foo", Some("foo"));
        assert_eq!(bundle_display_name(&bundle).as_deref(), Some("Plist Name"));

        let bare = dir.path().join("Bare Bones.app");
        std::fs::create_dir_all(&bare).expect("建目录");
        assert_eq!(bundle_display_name(&bare).as_deref(), Some("Bare Bones"));
    }
}
