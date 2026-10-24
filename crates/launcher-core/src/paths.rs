//! 路径布局：`APP_CATCH_HOME` 下的目录与文件位置。
//!
//! 约定（需求基线 F1/F3）：
//! - 根目录：环境变量 `APP_CATCH_HOME` 优先，否则 `<user home>/.app_catch`；
//!   Windows 与 macOS 都使用 `~/.app_catch`，保持两平台一致的跨平台基线；
//! - `<home>/apps/<app_id>.toml`：每个应用一个配置文件；
//! - `<home>/sessions/<session_id>/`：每次启动一个会话目录，事件流写在其中的 JSONL 文件；
//! - `<home>/icons/`：图标缓存。
//!
//! 所有函数只做路径计算或目录创建，不 panic、不读配置文件；返回值统一为 [`crate::error::Result`]。
//! 环境变量 `APP_CATCH_HOME` 是测试隔离的关键：单元测试一律指向 `tempfile` 临时目录，
//! 绝不触碰真实用户主目录。

use std::path::PathBuf;

use crate::error::{AppCatchError, Result};
use crate::session::SessionId;

/// 环境变量名：覆盖 app_catch 根目录（测试隔离与便携部署用）。
pub const HOME_ENV: &str = "APP_CATCH_HOME";

/// 会话事件流文件名（位于会话目录内）。
pub const SESSION_LOG_FILE: &str = "events.jsonl";

/// 应用配置文件的扩展名。
pub const APP_CONFIG_EXT: &str = "toml";

/// 返回 app_catch 根目录的路径。
///
/// 优先级：环境变量 [`HOME_ENV`] > `dirs::home_dir()` 下的 `.app_catch`。
/// 两者都取不到时返回 [`AppCatchError::HomeDirUnavailable`]。
///
/// 注意：本函数不创建目录，也不校验路径是否可写；创建目录请用 [`ensure_layout`]。
pub fn app_catch_home() -> Result<PathBuf> {
    if let Some(raw) = std::env::var_os(HOME_ENV) {
        let trimmed = raw.to_string_lossy().trim().to_string();
        if !trimmed.is_empty() {
            return Ok(PathBuf::from(trimmed));
        }
    }
    let home = dirs::home_dir().ok_or(AppCatchError::HomeDirUnavailable)?;
    Ok(home.join(".app_catch"))
}

/// `<home>/apps`：应用配置目录。
pub fn apps_dir() -> Result<PathBuf> {
    Ok(app_catch_home()?.join("apps"))
}

/// `<home>/sessions`：会话目录。
pub fn sessions_dir() -> Result<PathBuf> {
    Ok(app_catch_home()?.join("sessions"))
}

/// `<home>/icons`：图标缓存目录。
pub fn icons_dir() -> Result<PathBuf> {
    Ok(app_catch_home()?.join("icons"))
}

/// `<home>/sessions/<session_id>`：单个会话目录。
pub fn session_dir(session_id: &SessionId) -> Result<PathBuf> {
    Ok(sessions_dir()?.join(session_id.to_string()))
}

/// `<home>/sessions/<session_id>/events.jsonl`：会话事件流文件。
pub fn session_log_path(session_id: &SessionId) -> Result<PathBuf> {
    Ok(session_dir(session_id)?.join(SESSION_LOG_FILE))
}

/// 幂等地创建运行所需目录：根目录、apps、sessions、icons。
///
/// 已存在时不报错；权限不足等错误以 [`AppCatchError::Io`] 返回。
pub fn ensure_layout() -> Result<()> {
    for dir in [app_catch_home()?, apps_dir()?, sessions_dir()?, icons_dir()?] {
        std::fs::create_dir_all(&dir).map_err(|e| AppCatchError::io(dir, e))?;
    }
    Ok(())
}

/// 幂等地创建单个会话目录，返回其路径。
///
/// M1b 写事件流前调用；M1a 只提供目录语义，不写文件。
pub fn ensure_session_dir(session_id: &SessionId) -> Result<PathBuf> {
    let dir = session_dir(session_id)?;
    std::fs::create_dir_all(&dir).map_err(|e| AppCatchError::io(&dir, e))?;
    Ok(dir)
}

#[cfg(test)]
pub(crate) mod tests {
    use super::*;
    use std::ffi::OsString;

    /// 进程级互斥锁：`APP_CATCH_HOME` 是全局状态，cargo test 默认多线程并行，
    /// 必须让所有读写该变量的测试串行执行，否则会互相看到别人的临时目录（真实踩过的 flaky）。
    static ENV_LOCK: std::sync::Mutex<()> = std::sync::Mutex::new(());

    /// 在测试期间设置 `APP_CATCH_HOME`，离开作用域时恢复原值。
    ///
    /// 构造时持有 [`ENV_LOCK`]，因此同一时刻只有一个测试在操作该环境变量。
    pub(crate) struct HomeGuard {
        previous: Option<OsString>,
        // 仅用于持有锁；字段本身不被读取
        _lock: std::sync::MutexGuard<'static, ()>,
    }

    impl HomeGuard {
        /// 把 `APP_CATCH_HOME` 指向 `path`。
        pub(crate) fn set(path: &std::path::Path) -> Self {
            let lock = ENV_LOCK.lock().unwrap_or_else(|e| e.into_inner());
            let previous = std::env::var_os(HOME_ENV);
            std::env::set_var(HOME_ENV, path);
            Self {
                previous,
                _lock: lock,
            }
        }

        /// 移除 `APP_CATCH_HOME`（验证回落到用户主目录的分支）。
        pub(crate) fn unset() -> Self {
            let lock = ENV_LOCK.lock().unwrap_or_else(|e| e.into_inner());
            let previous = std::env::var_os(HOME_ENV);
            std::env::remove_var(HOME_ENV);
            Self {
                previous,
                _lock: lock,
            }
        }
    }

    impl Drop for HomeGuard {
        fn drop(&mut self) {
            match self.previous.take() {
                Some(v) => std::env::set_var(HOME_ENV, v),
                None => std::env::remove_var(HOME_ENV),
            }
        }
    }

    #[test]
    fn home_respects_env_override() {
        let dir = tempfile::tempdir().expect("创建临时目录失败");
        let _guard = HomeGuard::set(dir.path());

        assert_eq!(app_catch_home().expect("取 home 失败"), dir.path());
        assert_eq!(apps_dir().expect("取 apps 失败"), dir.path().join("apps"));
        assert_eq!(
            sessions_dir().expect("取 sessions 失败"),
            dir.path().join("sessions")
        );
        assert_eq!(icons_dir().expect("取 icons 失败"), dir.path().join("icons"));
    }

    #[test]
    fn session_paths_are_nested_under_sessions() {
        let dir = tempfile::tempdir().expect("创建临时目录失败");
        let _guard = HomeGuard::set(dir.path());
        let id = SessionId::new_v4();

        let sdir = session_dir(&id).expect("取会话目录失败");
        assert_eq!(sdir, dir.path().join("sessions").join(id.to_string()));

        let log = session_log_path(&id).expect("取日志路径失败");
        assert_eq!(log, sdir.join(SESSION_LOG_FILE));
        assert!(log.to_string_lossy().ends_with("events.jsonl"));
    }

    #[test]
    fn ensure_layout_is_idempotent() {
        let dir = tempfile::tempdir().expect("创建临时目录失败");
        let _guard = HomeGuard::set(dir.path());

        ensure_layout().expect("首次创建布局失败");
        ensure_layout().expect("二次创建布局应幂等");

        for sub in ["", "apps", "sessions", "icons"] {
            let p = dir.path().join(sub);
            assert!(p.is_dir(), "目录应存在: {}", p.display());
        }
    }

    #[test]
    fn ensure_session_dir_creates_and_is_idempotent() {
        let dir = tempfile::tempdir().expect("创建临时目录失败");
        let _guard = HomeGuard::set(dir.path());
        let id = SessionId::new_v4();

        let created = ensure_session_dir(&id).expect("创建会话目录失败");
        assert!(created.is_dir());
        ensure_session_dir(&id).expect("重复创建应幂等");
    }

    #[test]
    fn home_falls_back_to_user_home_without_env() {
        // 未设置 APP_CATCH_HOME 时回落到 <home>/.app_catch；仅断言后缀，避免依赖真实 home 内容
        let _guard = HomeGuard::unset();

        let home = app_catch_home().expect("回落取 home 失败");
        assert!(
            home.ends_with(".app_catch"),
            "应回落到 <home>/.app_catch，实际: {}",
            home.display()
        );
    }
}