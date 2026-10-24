//! 统一错误类型。
//!
//! 设计取舍：
//! - 单一枚举 `AppCatchError` 覆盖所有公开 API，调用方（Tauri command 层）只需匹配一个类型；
//! - 只在本 crate 内部错误边界使用 `thiserror`，不把 `anyhow` 之类的动态错误带进公开 API；
//! - 每个变体都带足够上下文（路径、id、原始错误），便于 UI 直接展示与日志排查；
//! - `AppCatchError::Io` 只包装 `std::io::Error`，序列化/解析错误各自独立变体。

use std::path::PathBuf;

/// launcher-core 的统一错误类型。
#[derive(Debug, thiserror::Error)]
pub enum AppCatchError {
    /// 请求的资源不存在（应用配置、会话文件等）。
    #[error("未找到: {0}")]
    NotFound(String),

    /// 创建时目标已存在（典型场景：应用 id 冲突）。
    #[error("已存在: {0}")]
    AlreadyExists(String),

    /// 标识符不合法（如 `AppId` 含非法字符或超长）。
    #[error("非法标识符 {value:?}: {reason}")]
    InvalidId { value: String, reason: String },

    /// 底层 IO 错误，`path` 指出出错的文件或目录。
    #[error("IO 错误 ({path}): {source}")]
    Io {
        path: PathBuf,
        #[source]
        source: std::io::Error,
    },

    /// 解析失败（TOML 反序列化、时间戳解析等）。
    #[error("解析失败 ({path}): {source}")]
    Parse {
        path: PathBuf,
        #[source]
        source: Box<dyn std::error::Error + Send + Sync>,
    },

    /// 序列化失败（TOML/JSON 编码）。
    #[error("序列化失败: {0}")]
    Serialize(String),

    /// 无法确定用户主目录（`dirs::home_dir` 返回 `None`，且未设置 `APP_CATCH_HOME`）。
    #[error("无法确定用户主目录：请设置环境变量 APP_CATCH_HOME")]
    HomeDirUnavailable,

    /// 平台能力尚未实现（M1a 只定义接口，真实逻辑在 M1b 补齐）。
    #[error("平台能力尚未实现: {0}")]
    Unsupported(String),
}

impl AppCatchError {
    /// 构造一个带路径上下文的 IO 错误。
    pub fn io(path: impl Into<PathBuf>, source: std::io::Error) -> Self {
        Self::Io {
            path: path.into(),
            source,
        }
    }

    /// 构造一个带路径上下文的解析错误。
    pub fn parse(
        path: impl Into<PathBuf>,
        source: impl std::error::Error + Send + Sync + 'static,
    ) -> Self {
        Self::Parse {
            path: path.into(),
            source: Box::new(source),
        }
    }

    /// `NotFound` 的便捷构造器。
    pub fn not_found(what: impl std::fmt::Display) -> Self {
        Self::NotFound(what.to_string())
    }

    /// `AlreadyExists` 的便捷构造器。
    pub fn already_exists(what: impl std::fmt::Display) -> Self {
        Self::AlreadyExists(what.to_string())
    }
}

/// crate 内部便捷别名；公开 API 上也直接使用它，避免逐处书写完整类型。
pub type Result<T> = std::result::Result<T, AppCatchError>;

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn io_error_keeps_path_and_message() {
        // 用 expect 说明理由：错误构造本身不应失败，失败即测试环境异常
        let err = AppCatchError::io(
            PathBuf::from("C:/tmp/x.toml"),
            std::io::Error::new(std::io::ErrorKind::NotFound, "missing"),
        );
        let msg = err.to_string();
        assert!(msg.contains("x.toml"), "错误信息应包含路径，实际: {msg}");
        assert!(msg.contains("missing"), "错误信息应包含原因，实际: {msg}");
    }

    #[test]
    fn invalid_id_error_mentions_value_and_reason() {
        let err = AppCatchError::InvalidId {
            value: "bad id".to_string(),
            reason: "包含非法字符".to_string(),
        };
        let msg = err.to_string();
        assert!(msg.contains("bad id"));
        assert!(msg.contains("非法字符"));
    }
}
