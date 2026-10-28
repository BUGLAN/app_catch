//! 「项目图标」档位：在应用的工作目录里找这个项目自己声明的图标。
//!
//! 为什么需要这一档：开发态启动的应用（`pnpm dev:electron`、`pnpm tauri dev`）跑起来的进程是
//! node / Electron 本身，从进程或可执行文件只能拿到解释器 / Electron 的默认图标；
//! 应用在运行时用 `app.dock.setIcon()` 设置的图标其它进程又读不到。
//! 而项目目录里一定有打包用的正式图标——electron-builder 的 `icon` 配置就指着它。
//!
//! 查找顺序（先找到能解码的为准）：
//!   1. electron-builder 配置（`electron-builder.yml` / `.yaml` / `.json`、`package.json` 的 `build`）：
//!      当前平台段（`mac` / `win` / `linux`）的 `icon` 优先，其次顶层 `icon`；
//!   2. 常见位置：`build/icon`、`resources/icon`、`assets/icon`、`src-tauri/icons/icon`、`public/icon-512` …
//!      无扩展名的候选按平台偏好补扩展名（macOS 先 icns、Windows 先 ico）。
//!
//! 纯文件读取，任意平台可用；解码见 [`load_image_file`]（PNG / icns / ico 内嵌 PNG）。

use std::path::{Path, PathBuf};

use crate::platform::icon::{load_image_file, RgbaImage};

/// electron-builder 里当前平台对应的配置段名。
#[cfg(target_os = "macos")]
const PLATFORM_KEY: &str = "mac";
#[cfg(windows)]
const PLATFORM_KEY: &str = "win";
#[cfg(not(any(windows, target_os = "macos")))]
const PLATFORM_KEY: &str = "linux";

/// 无扩展名候选的补全顺序：优先本平台的原生格式。
#[cfg(target_os = "macos")]
const EXT_PREFERENCE: [&str; 3] = ["icns", "png", "ico"];
#[cfg(windows)]
const EXT_PREFERENCE: [&str; 3] = ["ico", "png", "icns"];
#[cfg(not(any(windows, target_os = "macos")))]
const EXT_PREFERENCE: [&str; 3] = ["png", "icns", "ico"];

/// 常见图标位置（相对项目目录、不含扩展名），按"越像正式打包图标越靠前"排列。
const COMMON_STEMS: [&str; 8] = [
    "build/icon",
    "build/icons/icon",
    "resources/icon",
    "assets/icon",
    "src-tauri/icons/icon",
    "public/icon-512",
    "public/icon",
    "icon",
];

/// electron-builder 配置文件名。
const BUILDER_FILES: [&str; 3] = [
    "electron-builder.yml",
    "electron-builder.yaml",
    "electron-builder.json",
];

/// 在项目目录里找图标并解码；找不到返回 `None`。
pub fn project_icon(dir: &Path) -> Option<RgbaImage> {
    project_icon_candidates(dir)
        .iter()
        .find_map(|path| load_image_file(path).filter(|image| !image.is_fully_transparent()))
}

/// 项目目录里按优先级排列的、确实存在的图标文件候选。
pub fn project_icon_candidates(dir: &Path) -> Vec<PathBuf> {
    let mut candidates = Vec::new();
    for value in builder_icon_values(dir) {
        push_with_extensions(dir, &value, &mut candidates);
    }
    for stem in COMMON_STEMS {
        push_with_extensions(dir, stem, &mut candidates);
    }
    let mut seen = std::collections::HashSet::new();
    candidates.retain(|path| path.is_file() && seen.insert(path.clone()));
    candidates
}

/// 有扩展名的原样加入，没有的按平台偏好补全（electron-builder 允许写 `build/icon`）。
fn push_with_extensions(dir: &Path, value: &str, out: &mut Vec<PathBuf>) {
    let path = dir.join(value);
    if Path::new(value).extension().is_some() {
        out.push(path);
    } else {
        out.extend(EXT_PREFERENCE.iter().map(|ext| path.with_extension(ext)));
    }
}

/// 从所有 electron-builder 配置来源里收集 `icon` 值（平台段在前）。
fn builder_icon_values(dir: &Path) -> Vec<String> {
    let mut values = Vec::new();
    for file in BUILDER_FILES {
        let Ok(text) = std::fs::read_to_string(dir.join(file)) else {
            continue;
        };
        if file.ends_with(".json") {
            if let Ok(json) = serde_json::from_str::<serde_json::Value>(&text) {
                values.extend(json_icon_values(&json));
            }
        } else {
            values.extend(yaml_icon_values(&text));
        }
    }
    if let Ok(text) = std::fs::read_to_string(dir.join("package.json")) {
        if let Ok(json) = serde_json::from_str::<serde_json::Value>(&text) {
            if let Some(build) = json.get("build") {
                values.extend(json_icon_values(build));
            }
        }
    }
    values
}

/// electron-builder JSON 配置里的 `icon`：`<平台>.icon` 优先，其次顶层 `icon`（纯函数）。
fn json_icon_values(config: &serde_json::Value) -> Vec<String> {
    let platform = config
        .get(PLATFORM_KEY)
        .and_then(|section| section.get("icon"));
    [platform, config.get("icon")]
        .into_iter()
        .flatten()
        .filter_map(|value| value.as_str())
        .map(str::to_string)
        .collect()
}

/// 极简 YAML 读取：只认 `icon:` 这一个键，区分顶层与平台段（纯函数）。
///
/// 不引入 YAML 解析库：这里只需要两种形态——顶层 `icon: x` 与 `mac:` 段下缩进的 `icon: x`；
/// 注释行与行尾注释会被忽略，引号会被去掉。
fn yaml_icon_values(text: &str) -> Vec<String> {
    let mut section: Option<&str> = None;
    let mut platform = None;
    let mut top = None;
    for raw in text.lines() {
        let line = strip_yaml_comment(raw);
        let trimmed = line.trim();
        if trimmed.is_empty() {
            continue;
        }
        let indented = line.starts_with(char::is_whitespace);
        if !indented {
            if let Some(value) = yaml_key_value(trimmed, "icon") {
                top.get_or_insert(value);
                section = None;
            } else {
                section = trimmed.strip_suffix(':').map(str::trim);
            }
            continue;
        }
        if section == Some(PLATFORM_KEY) {
            if let Some(value) = yaml_key_value(trimmed, "icon") {
                platform.get_or_insert(value);
            }
        }
    }
    platform.into_iter().chain(top).collect()
}

/// 去掉 YAML 注释：整行 `#` 开头，或空白后跟 `#` 的行尾注释。
fn strip_yaml_comment(line: &str) -> &str {
    if line.trim_start().starts_with('#') {
        return "";
    }
    match line.find(" #") {
        Some(index) => &line[..index],
        None => line,
    }
}

/// `key: value` 形式取值，去掉成对引号；值为空时返回 `None`。
fn yaml_key_value(line: &str, key: &str) -> Option<String> {
    let rest = line.strip_prefix(key)?.trim_start().strip_prefix(':')?;
    let value = rest.trim().trim_matches(|c| c == '"' || c == '\'');
    (!value.is_empty()).then(|| value.to_string())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::platform::icon::{encode_png, RgbaImage};

    /// 与 opennote 的 electron-builder.yml 同形：注释掉的 icon、win / mac 两段、行尾注释。
    const BUILDER_YML: &str = "\
appId: com.example.app
# 注释里的配置不算：
#       icon: build/legacy.ico
win:
  target: nsis
  icon: build/icon.ico
mac:
  category: public.app-category.productivity
  icon: \"build/icon.icns\"   # 预生成的 icns
linux:
  icon: build/icons
";

    #[test]
    fn yaml_prefers_current_platform_section() {
        let values = yaml_icon_values(BUILDER_YML);
        let expected = match PLATFORM_KEY {
            "mac" => "build/icon.icns",
            "win" => "build/icon.ico",
            _ => "build/icons",
        };
        assert_eq!(values, vec![expected.to_string()]);
    }

    #[test]
    fn yaml_top_level_icon_comes_after_platform_icon() {
        let text = format!("icon: build/top.png\n{PLATFORM_KEY}:\n  icon: build/platform.png\n");
        assert_eq!(
            yaml_icon_values(&text),
            vec!["build/platform.png", "build/top.png"]
        );
        assert!(yaml_icon_values("# icon: x\nname: demo\n").is_empty());
    }

    #[test]
    fn json_reads_platform_then_top_level() {
        let json: serde_json::Value = serde_json::from_str(&format!(
            r#"{{ "icon": "build/top", "{PLATFORM_KEY}": {{ "icon": "build/p.png" }} }}"#
        ))
        .expect("合法 JSON");
        assert_eq!(json_icon_values(&json), vec!["build/p.png", "build/top"]);
    }

    fn write_png(path: &Path, rgba: [u8; 4]) {
        std::fs::create_dir_all(path.parent().expect("有父目录")).expect("建目录失败");
        let image = RgbaImage::solid(32, 32, rgba).expect("合法尺寸");
        std::fs::write(path, encode_png(&image).expect("编码失败")).expect("写文件失败");
    }

    #[test]
    fn builder_config_wins_over_common_locations() {
        let dir = tempfile::tempdir().expect("临时目录失败");
        write_png(&dir.path().join("public/icon-512.png"), [1, 1, 1, 255]);
        write_png(&dir.path().join("art/brand.png"), [9, 9, 9, 255]);
        std::fs::write(
            dir.path().join("package.json"),
            r#"{ "name": "demo", "build": { "icon": "art/brand.png" } }"#,
        )
        .expect("写 package.json 失败");

        let candidates = project_icon_candidates(dir.path());
        assert_eq!(candidates.first(), Some(&dir.path().join("art/brand.png")));
        assert!(candidates.contains(&dir.path().join("public/icon-512.png")));
        let image = project_icon(dir.path()).expect("应解出配置里的图标");
        assert_eq!(image.width, 32);
    }

    #[test]
    fn extensionless_value_and_common_stem_are_completed() {
        let dir = tempfile::tempdir().expect("临时目录失败");
        write_png(&dir.path().join("build/icon.png"), [5, 5, 5, 255]);
        assert_eq!(
            project_icon_candidates(dir.path()),
            vec![dir.path().join("build/icon.png")]
        );
        assert!(project_icon(tempfile::tempdir().expect("临时目录失败").path()).is_none());
    }
}
