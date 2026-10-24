//! Tauri 共享状态：配置仓库 + 会话管理器 + 事件读取器 + 捕获配置。
//!
//! 设计取舍：
//! - **管理器放在 `RwLock` 后面**：读写都极短（读是克隆一次 `Arc`），
//!   但 GUI 允许改捕获配置，改完后必须换出一个新的 `SessionManager`
//!   （管理器内部字段不可变）。用 `std::sync::RwLock` 而不是 tokio 的，
//!   因为这里没有任何 `.await` 会横跨持锁区间；
//! - **对外只交出 `Arc<SessionManager>`**：Tauri 命令并发执行，
//!   克隆 `Arc` 后立刻释放锁，不把长耗时操作（启动进程）留在锁内；
//! - **不用 `unwrap`/`expect`**：所有可能失败的点都返回 `Result`，
//!   锁中毒（poison）按「继续用」处理，不让一次 panic 把整个 GUI 卡死。

use std::sync::{Arc, RwLock};

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
    /// 捕获配置（可被 `set_capture_config` 整体替换）。
    capture: RwLock<CaptureConfig>,
    /// 会话管理器；改捕获配置后整体重建。
    manager: RwLock<Arc<SessionManager>>,
    /// 会话事件读取器（只是缓存容器，可长期复用）。
    reader: SessionEventReader,
}

impl AppState {
    /// 按默认布局（`APP_CATCH_HOME` 优先）初始化：建目录 + 造管理器。
    ///
    /// 只做布局与构造，**不**恢复会话——恢复是异步的，交给 `recover_sessions` 命令。
    pub fn bootstrap() -> Result<Self, CommandError> {
        paths::ensure_layout().map_err(CommandError::from)?;
        let store = AppStore::from_home().map_err(CommandError::from)?;
        let capture = CaptureConfig::default();
        let manager = build_manager(&capture)?;
        Ok(Self {
            store,
            capture: RwLock::new(capture),
            manager: RwLock::new(Arc::new(manager)),
            reader: SessionEventReader::new(),
        })
    }

    /// 配置仓库。
    pub fn store(&self) -> &AppStore {
        &self.store
    }

    /// 会话事件读取器。
    pub fn reader(&self) -> &SessionEventReader {
        &self.reader
    }

    /// 当前捕获配置的快照。
    pub fn capture_config(&self) -> CaptureConfig {
        let guard = match self.capture.read() {
            Ok(guard) => guard,
            Err(poisoned) => poisoned.into_inner(),
        };
        guard.clone()
    }

    /// 当前会话管理器（克隆 `Arc`，调用方在锁外使用）。
    pub fn manager(&self) -> Arc<SessionManager> {
        let guard = match self.manager.read() {
            Ok(guard) => guard,
            Err(poisoned) => poisoned.into_inner(),
        };
        Arc::clone(&guard)
    }

    /// 替换捕获配置并重建管理器。
    ///
    /// 重建会丢弃管理器的内存会话表，因此**有运行中会话时拒绝**：
    /// 那些进程仍在跑，丢掉表项会让它们既停不掉也查不到。
    /// 想改配置请先停掉所有会话——这是刻意的保守选择，不是缺陷。
    pub fn replace_capture_config(
        &self,
        capture: CaptureConfig,
        running: usize,
    ) -> Result<(), CommandError> {
        if running > 0 {
            return Err(CommandError::new(
                "conflict",
                format!("还有 {running} 个会话在运行，请先停止它们再修改捕获配置"),
            ));
        }
        let manager = build_manager(&capture)?;
        {
            let mut guard = match self.manager.write() {
                Ok(guard) => guard,
                Err(poisoned) => poisoned.into_inner(),
            };
            *guard = Arc::new(manager);
        }
        {
            let mut guard = match self.capture.write() {
                Ok(guard) => guard,
                Err(poisoned) => poisoned.into_inner(),
            };
            *guard = capture;
        }
        Ok(())
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
    pub async fn running_pid_for_app(&self, app_id: &str) -> Option<u32> {
        let manager = self.manager();
        manager
            .list_active()
            .await
            .into_iter()
            .filter(|snapshot| snapshot.app_id.as_str() == app_id)
            .filter(|snapshot| snapshot.state == launcher_core::RunState::Running)
            .map(|snapshot| snapshot.pid)
            .next_back()
    }
}

/// 用给定捕获配置构造会话管理器。
fn build_manager(capture: &CaptureConfig) -> Result<SessionManager, CommandError> {
    SessionManager::from_home(Arc::new(default_process_backend()), capture.clone())
        .map_err(CommandError::from)
}

/// 把前端传来的字符串解析成 [`AppId`]，失败时保留中文原因。
pub fn parse_app_id(raw: &str) -> Result<AppId, CommandError> {
    AppId::new(raw.trim()).map_err(CommandError::from)
}

#[cfg(test)]
mod tests {
    use super::*;

    /// 构造一个不依赖真实布局的测试态（不读磁盘、不建目录）。
    fn test_state(name: &str) -> AppState {
        let root = std::env::temp_dir().join(name);
        AppState {
            store: AppStore::new(root.join("apps")),
            capture: RwLock::new(CaptureConfig::default()),
            manager: RwLock::new(Arc::new(SessionManager::new(
                Arc::new(default_process_backend()),
                CaptureConfig::default(),
                root.join("sessions"),
            ))),
            reader: SessionEventReader::new(),
        }
    }

    #[test]
    fn parse_app_id_rejects_invalid_and_accepts_valid() {
        assert!(parse_app_id("demo-app").is_ok());
        assert!(parse_app_id("  demo_1  ").is_ok(), "应自动去首尾空白");

        let err = parse_app_id("a b").expect_err("含空格应失败");
        assert_eq!(err.kind, "invalid_id");
        assert!(err.message.contains("非法标识符"), "文案: {}", err.message);
    }

    #[test]
    fn replace_capture_config_refuses_while_sessions_running() {
        let state = test_state("app-catch-test-refuse");
        let err = state
            .replace_capture_config(CaptureConfig::default(), 2)
            .expect_err("有运行中会话时应拒绝");
        assert_eq!(err.kind, "conflict");
        assert!(err.message.contains("2 个会话"), "文案: {}", err.message);
    }

    #[test]
    fn replace_capture_config_swaps_when_idle() {
        let state = test_state("app-catch-test-swap");
        let mut capture = CaptureConfig::default();
        capture.max_status_events = Some(7);
        state
            .replace_capture_config(capture, 0)
            .expect("空闲时应允许替换");
        assert_eq!(state.capture_config().max_status_events, Some(7));
    }
}
