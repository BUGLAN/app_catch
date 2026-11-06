//! Tauri 共享状态：配置仓库 + 会话管理器 + 事件读取器。
//!
//! 设计取舍：
//! - **管理器启动时构造一次，之后不再替换**：捕获参数统一用
//!   `CaptureConfig::default()`，不对界面开放，因此不需要「改配置 -> 重建管理器」
//!   这条会丢会话表的路径，也就不需要锁；
//! - **对外只交出 `Arc`**：Tauri 命令并发执行，克隆 `Arc` 后即可在 `spawn_blocking`
//!   或 `.await` 之后继续使用，不把借用绑在 `State` 的生命周期上；
//! - **不用 `unwrap`/`expect`**：所有可能失败的点都返回 `Result`。

use std::sync::Arc;

use launcher_core::config::{AppConfig, AppId, AppStore};
use launcher_core::paths;
use launcher_core::platform::default_process_backend;
use launcher_core::runtime::{CaptureConfig, SessionManager};

use crate::error::CommandError;
use crate::events_reader::SessionEventReader;

/// 应用全局状态（由 `tauri::State` 托管）。
pub struct AppState {
    /// 配置仓库；内部只持有目录路径，构造后不可变。
    store: AppStore,
    /// 会话管理器；启动时构造一次。
    manager: Arc<SessionManager>,
    /// 会话事件读取器（只是缓存容器，可长期复用）。
    ///
    /// 包一层 `Arc`：读取在 `spawn_blocking` 里做，闭包需要 `'static` 的所有权。
    reader: Arc<SessionEventReader>,
}

impl AppState {
    /// 按默认布局（`APP_CATCH_HOME` 优先）初始化：建目录 + 造管理器。
    ///
    /// 只做布局与构造，**不**恢复会话——恢复是异步的，交给 `recover_sessions` 命令。
    pub fn bootstrap() -> Result<Self, CommandError> {
        paths::ensure_layout().map_err(CommandError::from)?;
        let store = AppStore::from_home().map_err(CommandError::from)?;
        let manager = SessionManager::from_home(
            Arc::new(default_process_backend()),
            CaptureConfig::default(),
        )
        .map_err(CommandError::from)?;
        Ok(Self {
            store,
            manager: Arc::new(manager),
            reader: Arc::new(SessionEventReader::new()),
        })
    }

    /// 配置仓库。
    pub fn store(&self) -> &AppStore {
        &self.store
    }

    /// 会话事件读取器（克隆 `Arc`，可移进阻塞任务）。
    pub fn reader(&self) -> Arc<SessionEventReader> {
        Arc::clone(&self.reader)
    }

    /// 会话管理器（克隆 `Arc`）。
    pub fn manager(&self) -> Arc<SessionManager> {
        Arc::clone(&self.manager)
    }

    /// 读取应用配置；id 不合法或不存在都返回中文错误。
    pub fn get_app(&self, id: &str) -> Result<AppConfig, CommandError> {
        let app_id = parse_app_id(id)?;
        self.store.get(&app_id).map_err(CommandError::from)
    }

    /// 某个应用当前是否有运行中的会话；返回最近启动的那个根 pid。
    ///
    /// 图标抓取需要 pid 才能走「运行中窗口图标」这条路径
    /// ——这正是基线说的「启动后尽力抓取运行时图标」。
    /// `list_active` 按启动先后排序，因此取最后一个就是最近启动的。
    pub async fn running_pid_for_app(&self, app_id: &str) -> Option<u32> {
        self.manager
            .list_active()
            .await
            .into_iter()
            .filter(|snapshot| snapshot.app_id.as_str() == app_id)
            .map(|snapshot| snapshot.pid)
            .next_back()
    }
}

/// 把前端传来的字符串解析成 [`AppId`]，失败时保留中文原因。
pub fn parse_app_id(raw: &str) -> Result<AppId, CommandError> {
    AppId::new(raw.trim()).map_err(CommandError::from)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parse_app_id_rejects_invalid_and_accepts_valid() {
        assert!(parse_app_id("demo-app").is_ok());
        assert!(parse_app_id("  demo_1  ").is_ok(), "应自动去首尾空白");

        let err = parse_app_id("a b").expect_err("含空格应失败");
        assert_eq!(err.kind, "invalid_id");
        assert!(err.message.contains("非法标识符"), "文案: {}", err.message);
    }
}
