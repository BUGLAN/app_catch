//! app_catch 的核心逻辑 crate。
//!
//! 设计约束（来自需求基线「launcher-core 不依赖 UI，可独立测试」）：
//! - 本 crate 不得依赖 tauri 或任何 GUI 库，保证能在无窗口环境下跑单元测试；
//! - 配置管理、进程生命周期、会话 JSONL、图标捕获等能力从 M1 起逐步落地；
//! - M1a（当前）只落地数据层：路径布局、配置 TOML 存储、会话事件模型、平台抽象 trait，
//!   不含任何进程 spawn / 杀进程 / 写 JSONL 的运行时逻辑（那是 M1b）。

#![forbid(unsafe_code)]

pub mod config;
pub mod error;
pub mod fsutil;
pub mod paths;
pub mod platform;
pub mod session;

pub use config::{AppConfig, AppId, AppStore};
pub use error::{AppCatchError, Result};
pub use session::{ExitReason, RunState, SessionEvent, SessionId, StreamKind};

/// 返回本 crate 的版本号（取自 Cargo 包元数据）。
///
/// M0 阶段用于打通「前端 -> Tauri command -> launcher-core」的完整调用链路。
pub fn version() -> &'static str {
    env!("CARGO_PKG_VERSION")
}

/// 返回 crate 名，便于界面与日志自描述。
pub fn crate_name() -> &'static str {
    env!("CARGO_PKG_NAME")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn version_is_not_empty() {
        assert!(!version().is_empty(), "版本号不应为空");
    }

    #[test]
    fn version_matches_cargo_metadata() {
        // 与 Cargo 注入的包版本严格一致，防止后续手写版本号漂移
        assert_eq!(version(), env!("CARGO_PKG_VERSION"));
        assert_eq!(crate_name(), "launcher-core");
        assert!(version().starts_with("0.1."), "M0 基线版本应为 0.1.x");
    }
}