//! 「选择应用程序」入口的识别：用户选中一个 `.app` / 可执行文件后，给表单要自动填的信息。
//!
//! 只读文件系统，不启动任何进程（macOS 读 `Info.plist` 用的 `plutil` 除外）。

use std::path::Path;

use serde::Serialize;

use crate::error::{AppCatchError, Result};

/// 识别结果，字段名 snake_case 直接上线给前端。
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct ApplicationInfo {
    /// 建议写进启动命令的路径（`.app` 包本身，或可执行文件）。
    pub command: String,
    /// 建议的应用名称：`.app` 取 Info.plist 的显示名，其它取文件名去扩展名。
    pub name: String,
    /// 是否是 macOS `.app` 包（前端据此显示「启动方式」）。
    pub is_bundle: bool,
    /// `.app` 包里真正的可执行文件；不是包、或包里找不到时为 `None`。
    pub executable: Option<String>,
}

/// 识别用户选中的应用程序。
///
/// - 路径不存在：[`AppCatchError::NotFound`]；
/// - macOS 的 `.app` 包：名称取 `CFBundleDisplayName` / `CFBundleName`，并解析包内可执行文件；
/// - 其它普通文件：名称取文件名去扩展名；
/// - 是目录但不是 `.app`：[`AppCatchError::Unsupported`]（项目文件夹应走「选择项目文件夹」）。
pub fn inspect_application(path: &Path) -> Result<ApplicationInfo> {
    if !path.exists() {
        return Err(AppCatchError::NotFound(format!(
            "应用程序 {}",
            path.display()
        )));
    }
    let command = path.to_string_lossy().trim_end_matches('/').to_string();

    #[cfg(target_os = "macos")]
    if crate::platform::bundle::is_app_bundle(path) {
        use crate::platform::bundle;
        let name = bundle::bundle_display_name(path).unwrap_or_else(|| file_stem(path));
        let executable =
            bundle::bundle_executable(path).map(|exe| exe.to_string_lossy().into_owned());
        return Ok(ApplicationInfo {
            command,
            name,
            is_bundle: true,
            executable,
        });
    }

    if path.is_dir() {
        return Err(AppCatchError::Unsupported(format!(
            "{} 是文件夹而不是应用程序；项目文件夹请用「选择项目文件夹」",
            path.display()
        )));
    }
    Ok(ApplicationInfo {
        command,
        name: file_stem(path),
        is_bundle: false,
        executable: None,
    })
}

/// 文件名去掉扩展名；取不到时退回完整路径。
fn file_stem(path: &Path) -> String {
    path.file_stem()
        .map(|stem| stem.to_string_lossy().into_owned())
        .filter(|stem| !stem.is_empty())
        .unwrap_or_else(|| path.to_string_lossy().into_owned())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn plain_executable_uses_file_stem() {
        let dir = tempfile::tempdir().expect("临时目录");
        let exe = dir.path().join("my-tool.exe");
        std::fs::write(&exe, "").expect("写文件");
        let info = inspect_application(&exe).expect("应识别");
        assert_eq!(info.name, "my-tool");
        assert_eq!(info.command, exe.to_string_lossy());
        assert!(!info.is_bundle);
        assert_eq!(info.executable, None);
    }

    #[test]
    fn missing_path_and_plain_folder_are_rejected() {
        let dir = tempfile::tempdir().expect("临时目录");
        assert!(matches!(
            inspect_application(&dir.path().join("nope")),
            Err(AppCatchError::NotFound(_))
        ));
        assert!(matches!(
            inspect_application(dir.path()),
            Err(AppCatchError::Unsupported(_))
        ));
    }

    #[cfg(target_os = "macos")]
    #[test]
    fn app_bundle_reads_display_name_and_executable() {
        let dir = tempfile::tempdir().expect("临时目录");
        let bundle = dir.path().join("Voice Lab.app");
        let macos = bundle.join("Contents/MacOS");
        std::fs::create_dir_all(&macos).expect("建目录");
        std::fs::write(macos.join("voicelab"), "").expect("写可执行文件");
        std::fs::write(
            bundle.join("Contents/Info.plist"),
            "<?xml version=\"1.0\" encoding=\"UTF-8\"?><plist version=\"1.0\"><dict>\
             <key>CFBundleExecutable</key><string>voicelab</string>\
             <key>CFBundleDisplayName</key><string>语音实验室</string>\
             </dict></plist>",
        )
        .expect("写 Info.plist");

        // 末尾带斜杠（拖进来的路径常见）也应得到干净的命令
        let with_slash = std::path::PathBuf::from(format!("{}/", bundle.display()));
        let info = inspect_application(&with_slash).expect("应识别");
        assert_eq!(info.name, "语音实验室");
        assert!(info.is_bundle);
        assert_eq!(info.command, bundle.to_string_lossy());
        assert_eq!(
            info.executable.as_deref(),
            Some(macos.join("voicelab").to_string_lossy().as_ref())
        );
    }
}
