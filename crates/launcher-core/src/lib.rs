//! app_catch 的核心逻辑 crate。
//!
//! 设计约束（来自需求基线「launcher-core 不依赖 UI，可独立测试」）：
//! - 本 crate 不得依赖 tauri 或任何 GUI 库，保证能在无窗口环境下跑单元测试；
//! - 配置管理、进程生命周期、会话 JSONL、图标捕获等能力从 M1 起逐步落地；
//! - M1a（当前）只落地数据层：路径布局、配置 TOML 存储、会话事件模型、平台抽象 trait，
//!   不含任何进程 spawn / 杀进程 / 写 JSONL 的运行时逻辑（那是 M1b）。

//! 安全边界：除 `platform::windows`（直接调用 Win32 API，必须 unsafe）外，
//! 其余模块一律禁止 unsafe。`forbid` 无法被子模块豁免，因此用 `deny` +
//! 在唯一豁免处显式 `#[allow(unsafe_code)]`，保证豁免点可枚举、可审计。
#![deny(unsafe_code)]

pub mod config;
pub mod error;
pub mod fsutil;
pub mod paths;
pub mod platform;
pub mod runtime;
pub mod session;

pub use config::{AppConfig, AppId, AppStore};
pub use error::{AppCatchError, Result};
pub use runtime::{ExitInfo, SessionHandle, SessionManager, SessionSnapshot};
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

        // 只约束形状（MAJOR.MINOR.PATCH 三段数字），不锁死具体版本。
        // 早期这里写的是 starts_with("0.1.")：一旦发 0.2.0，CI 就会红在本行，
        // 而它想守的其实只是「版本号是个正常的语义化版本」。
        let parts: Vec<&str> = version().split('.').collect();
        assert_eq!(parts.len(), 3, "版本号应为 MAJOR.MINOR.PATCH，实际 {}", version());
        assert!(
            parts.iter().all(|part| part.parse::<u32>().is_ok()),
            "版本号各段应为数字，实际 {}",
            version()
        );
    }
}