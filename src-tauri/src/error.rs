//! command 层错误：把 launcher-core 的 [`AppCatchError`] 转成前端可读的结构化错误。
//!
//! 设计取舍：
//! - **不把 Rust 错误直接抛给前端**：`AppCatchError` 没有实现 `Serialize`，
//!   而且它的 `Debug` 形式对用户没有意义；
//! - **单一结构**：`{ kind, message }`。`kind` 是稳定的机器可读分类（前端据此决定
//!   提示样式，例如 not_found 走行内提示、io 走 toast），`message` 是中文可读文案；
//! - **文案保持中文**：`AppCatchError` 的 `Display` 已经是中文，直接复用，
//!   避免在这一层复制一份容易漂移的映射表。

use serde::Serialize;

use launcher_core::error::AppCatchError;

/// 命令层统一错误。
///
/// Tauri 会把实现了 `Serialize` 的错误类型作为命令失败值原样交给前端，
/// 因此前端拿到的永远是 `{ kind, message }` 这个形状。
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct CommandError {
    /// 稳定的机器可读分类，见各构造函数与 [`CommandError::from`]。
    pub kind: &'static str,
    /// 中文可读文案，可直接展示给用户。
    pub message: String,
}

impl CommandError {
    /// 用显式分类与文案构造。
    pub fn new(kind: &'static str, message: impl Into<String>) -> Self {
        Self {
            kind,
            message: message.into(),
        }
    }

    /// 内部错误（程序自身缺陷或无法归类的失败）。
    pub fn internal(message: impl Into<String>) -> Self {
        Self::new("internal", message)
    }

    /// 输入校验失败（表单字段不合法，前端应就地提示）。
    pub fn invalid_input(message: impl Into<String>) -> Self {
        Self::new("invalid_input", message)
    }
}

impl std::fmt::Display for CommandError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        // 直接展示文案：日志与界面看到的是同一句话
        f.write_str(&self.message)
    }
}

impl std::error::Error for CommandError {}

impl From<AppCatchError> for CommandError {
    fn from(err: AppCatchError) -> Self {
        let kind = match &err {
            AppCatchError::NotFound(_) => "not_found",
            AppCatchError::AlreadyExists(_) => "already_exists",
            AppCatchError::InvalidId { .. } => "invalid_id",
            AppCatchError::Io { .. } => "io",
            AppCatchError::Parse { .. } => "parse",
            AppCatchError::Serialize(_) => "serialize",
            AppCatchError::HomeDirUnavailable => "home_unavailable",
            AppCatchError::Unsupported(_) => "unsupported",
        };
        // message 复用 Display：launcher-core 的错误文案本身就是中文
        Self {
            kind,
            message: err.to_string(),
        }
    }
}

impl From<CommandError> for String {
    fn from(err: CommandError) -> Self {
        err.message
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::PathBuf;

    #[test]
    fn not_found_maps_to_chinese_message_and_kind() {
        let err: CommandError = AppCatchError::not_found("应用配置 ghost").into();
        assert_eq!(err.kind, "not_found");
        assert!(err.message.contains("未找到"), "文案应为中文: {}", err.message);
        assert!(err.message.contains("ghost"), "文案应带上下文: {}", err.message);
    }

    #[test]
    fn every_variant_has_a_stable_kind() {
        // 分类一旦被前端依赖就不能漂移，这里逐条钉死
        let cases: Vec<(AppCatchError, &str)> = vec![
            (AppCatchError::NotFound("x".into()), "not_found"),
            (AppCatchError::AlreadyExists("x".into()), "already_exists"),
            (
                AppCatchError::InvalidId {
                    value: "x".into(),
                    reason: "y".into(),
                },
                "invalid_id",
            ),
            (
                AppCatchError::io(
                    PathBuf::from("C:/x"),
                    std::io::Error::new(std::io::ErrorKind::NotFound, "missing"),
                ),
                "io",
            ),
            (
                AppCatchError::parse(
                    PathBuf::from("C:/x"),
                    std::io::Error::new(std::io::ErrorKind::InvalidData, "坏格式"),
                ),
                "parse",
            ),
            (AppCatchError::Serialize("x".into()), "serialize"),
            (AppCatchError::HomeDirUnavailable, "home_unavailable"),
            (AppCatchError::Unsupported("x".into()), "unsupported"),
        ];

        for (source, expected) in cases {
            let converted: CommandError = source.into();
            assert_eq!(converted.kind, expected);
            assert!(
                !converted.message.is_empty(),
                "{expected} 的文案不应为空"
            );
        }
    }

    #[test]
    fn invalid_id_message_stays_readable_for_form_errors() {
        let source = AppCatchError::InvalidId {
            value: "bad id".to_string(),
            reason: "包含非法字符 ' '".to_string(),
        };
        let err: CommandError = source.into();
        assert_eq!(err.kind, "invalid_id");
        assert!(err.message.contains("非法标识符"));
        assert_eq!(err.to_string(), err.message, "Display 与 message 应一致");
    }

    #[test]
    fn helper_constructors_keep_kind_and_message() {
        let internal = CommandError::internal("读盘失败");
        assert_eq!(internal.kind, "internal");
        assert_eq!(internal.message, "读盘失败");

        let invalid = CommandError::invalid_input("名称不能为空");
        assert_eq!(invalid.kind, "invalid_input");
        assert_eq!(String::from(invalid), "名称不能为空");
    }
}
