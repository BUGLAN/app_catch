//! 平台抽象层：进程后端与图标捕获。
//!
//! 需求基线要求「平台相关代码走抽象层，不锁死单一平台」。M1a 只定义接口与占位实现：
//! - [`ProcessBackend`]：spawn / kill_tree / attach（按 pid + 进程启动时间重关联，防 PID 复用）；
//! - [`IconCapture`]：从可执行文件或运行中窗口尽力抓取图标；
//! - 具体平台实现按 `cfg` 选择：Windows 见 [`windows`]，其它平台见 [`fallback`]。
//!
//! 本阶段**不包含任何真实进程逻辑**：占位实现统一返回 [`AppCatchError::Unsupported`]，
//! 而不是 `todo!()` / `panic!`，这样 M1b 填充前调用方也能拿到可读错误而不是让进程崩掉。

use std::collections::BTreeMap;
use std::path::PathBuf;

use crate::config::AppId;
use crate::error::Result;
use crate::session::SessionId;

#[cfg(not(windows))]
pub mod fallback;
#[cfg(windows)]
pub mod windows;

#[cfg(windows)]
pub use windows::{WindowsIconCapture, WindowsProcessBackend};
#[cfg(not(windows))]
pub use fallback::{FallbackIconCapture, FallbackProcessBackend};

/// 当前平台默认的进程后端类型。
#[cfg(windows)]
pub type PlatformProcessBackend = WindowsProcessBackend;
/// 当前平台默认的进程后端类型。
#[cfg(not(windows))]
pub type PlatformProcessBackend = FallbackProcessBackend;

/// 当前平台默认的图标捕获实现类型。
#[cfg(windows)]
pub type PlatformIconCapture = WindowsIconCapture;
/// 当前平台默认的图标捕获实现类型。
#[cfg(not(windows))]
pub type PlatformIconCapture = FallbackIconCapture;

/// 构造当前平台的默认进程后端。
pub fn default_process_backend() -> PlatformProcessBackend {
    PlatformProcessBackend::new()
}

/// 构造当前平台的默认图标捕获实现。
pub fn default_icon_capture() -> PlatformIconCapture {
    PlatformIconCapture::new()
}

/// 进程启动时间：用于区分「同一个 pid 但已是另一个进程」。
///
/// 平台语义（M1b 填充）：
/// - Windows：进程创建时间的 FILETIME，单位为 100ns，自 1601-01-01 UTC 起算；
/// - macOS：`proc_pidinfo` 返回的 `pbi_start_tvsec/nsec` 组合。
///
/// 存储为原始 `u64`，与 [`SessionEvent::Start`] 中记录的 pid 配合使用即可判定身份。
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct ProcessStartTime(u64);

impl ProcessStartTime {
    /// 用平台的原始值构造；具体含义由平台决定，跨平台比较没有意义。
    pub fn from_raw(raw: u64) -> Self {
        Self(raw)
    }

    /// 取出原始值（写盘 / 落库用）。
    pub fn as_raw(&self) -> u64 {
        self.0
    }
}

impl std::fmt::Display for ProcessStartTime {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}", self.0)
    }
}

/// 一个已启动（或已重新关联）进程的稳定身份。
///
/// `pid` 会随时间被复用，必须与 [`ProcessStartTime`] 一起使用才有身份意义。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ProcessHandle {
    /// 操作系统进程 id。
    pub pid: u32,
    /// 进程启动时间，防 PID 复用。
    pub start_time: ProcessStartTime,
}

impl ProcessHandle {
    /// 构造一个进程句柄。
    pub fn new(pid: u32, start_time: ProcessStartTime) -> Self {
        Self { pid, start_time }
    }
}

/// 启动请求：把 `AppConfig` 展开成后端可直接消费的形式。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SpawnRequest {
    /// 关联的会话 id（一次启动一个会话）。
    pub session_id: SessionId,
    /// 关联的应用配置 id。
    pub app_id: AppId,
    /// 可执行文件或命令。
    pub program: String,
    /// 命令行参数。
    pub args: Vec<String>,
    /// 工作目录；`None` 表示继承启动器当前目录。
    pub cwd: Option<PathBuf>,
    /// 环境变量增量（在父进程环境基础上追加）。
    pub env: BTreeMap<String, String>,
}

impl SpawnRequest {
    /// 以最小信息构造启动请求。
    pub fn new(session_id: SessionId, app_id: AppId, program: impl Into<String>) -> Self {
        Self {
            session_id,
            app_id,
            program: program.into(),
            args: Vec::new(),
            cwd: None,
            env: BTreeMap::new(),
        }
    }
}

/// 启动结果。
///
/// M1a 只携带进程身份；stdout/stderr 管道、等待退出等运行时能力在 M1b 随具体后端一并落地
/// （各后端用自身的方法扩出管道读取，避免把 tokio / std 的具体类型泄漏进对象安全的 trait）。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct SpawnedProcess {
    /// 进程身份（pid + 启动时间）。
    pub handle: ProcessHandle,
}

/// 进程退出信息。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ExitStatusInfo {
    /// 退出码；被信号杀死时通常为 `None`。
    pub code: Option<i32>,
}

/// 平台进程后端：启动、回收进程树、重启后重新关联。
///
/// 实现要求（M1b）：
/// - `spawn` 必须把子进程放进可整体回收的容器（Windows Job Object / macOS 进程组）；
/// - `kill_tree` 回收整棵进程树，不能只杀父进程；
/// - `attach` 必须同时校验 pid 与启动时间，避免 PID 复用导致误判。
pub trait ProcessBackend: Send + Sync {
    /// 启动子进程。
    ///
    /// M1a：占位实现返回 [`AppCatchError::Unsupported`]。
    fn spawn(&self, request: &SpawnRequest) -> Result<SpawnedProcess>;

    /// 回收整棵进程树。
    ///
    /// M1a：占位实现返回 [`AppCatchError::Unsupported`]。
    fn kill_tree(&self, handle: &ProcessHandle) -> Result<()>;

    /// 按 `pid` + 进程启动时间重新关联一个已存在的进程。
    ///
    /// 返回 `Ok(None)` 表示进程已不存在或启动时间不匹配（即 PID 已被复用），
    /// 返回 `Err` 表示查询本身失败（权限、系统调用错误）。
    fn attach(&self, pid: u32, start_time: ProcessStartTime) -> Result<Option<ProcessHandle>>;

    /// 判断进程是否仍然存活（同样校验启动时间）。
    fn is_running(&self, handle: &ProcessHandle) -> Result<bool>;

    /// 阻塞等待进程退出并返回退出信息（M1b：在独立任务中调用）。
    ///
    /// M1a：占位实现返回 [`AppCatchError::Unsupported`]。
    fn wait(&self, handle: &ProcessHandle) -> Result<ExitStatusInfo>;
}

/// 图标捕获目标：可执行文件 + （可能的）运行中窗口信息。
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct IconTarget {
    /// 可执行文件路径（Windows 从 exe 资源取图标）。
    pub executable: Option<PathBuf>,
    /// 关联进程 pid（用于按窗口取图标）。
    pub pid: Option<u32>,
    /// 窗口标题（辅助定位目标窗口）。
    pub window_title: Option<String>,
}

impl IconTarget {
    /// 只按可执行文件抓取。
    pub fn from_executable(path: impl Into<PathBuf>) -> Self {
        Self {
            executable: Some(path.into()),
            pid: None,
            window_title: None,
        }
    }
}

/// 图标捕获：尽力而为，失败（含无窗口服务类应用）返回 `None`，由调用方走内置兜底图。
pub trait IconCapture: Send + Sync {
    /// 抓取图标并返回缓存后的 PNG 路径；抓不到返回 `None`。
    ///
    /// M1a：占位实现恒返回 `None`。
    fn capture(&self, target: &IconTarget) -> Option<PathBuf>;
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::session::SessionId;

    #[test]
    fn spawn_request_carries_session_and_app_identity() {
        let session = SessionId::new_v4();
        let app = AppId::new("demo").expect("合法 id");
        let mut request = SpawnRequest::new(session, app.clone(), "python");
        request.args.push("main.py".to_string());
        request.env.insert("PYTHONUNBUFFERED".to_string(), "1".to_string());

        assert_eq!(request.session_id, session);
        assert_eq!(request.app_id, app);
        assert_eq!(request.program, "python");
        assert_eq!(request.args, vec!["main.py".to_string()]);
        assert_eq!(request.cwd, None);
        assert_eq!(request.env.len(), 1);
    }

    #[test]
    fn process_start_time_raw_round_trip_and_display() {
        let t = ProcessStartTime::from_raw(133_000_000_000_000_000);
        assert_eq!(t.as_raw(), 133_000_000_000_000_000);
        assert_eq!(t.to_string(), "133000000000000000");

        let handle = ProcessHandle::new(4321, t);
        assert_eq!(handle.pid, 4321);
        assert_eq!(handle.start_time, t);
    }

    #[test]
    fn icon_target_from_executable_has_no_pid() {
        let target = IconTarget::from_executable("C:/Windows/notepad.exe");
        assert_eq!(target.executable, Some(PathBuf::from("C:/Windows/notepad.exe")));
        assert_eq!(target.pid, None);
        assert_eq!(IconTarget::default(), IconTarget {
            executable: None,
            pid: None,
            window_title: None,
        });
    }

    /// 占位实现必须返回明确错误而不是 panic，M1b 填充前调用方也能安全试调用。
    #[cfg(windows)]
    #[test]
    fn placeholder_backend_reports_unsupported() {
        let backend = default_process_backend();
        let request = SpawnRequest::new(
            SessionId::new_v4(),
            AppId::new("demo").expect("合法 id"),
            "cmd.exe",
        );

        assert!(matches!(
            backend.spawn(&request).expect_err("占位实现应报错"),
            crate::error::AppCatchError::Unsupported(_)
        ));

        let handle = ProcessHandle::new(1, ProcessStartTime::from_raw(0));
        assert!(matches!(
            backend.kill_tree(&handle).expect_err("占位实现应报错"),
            crate::error::AppCatchError::Unsupported(_)
        ));
        assert!(matches!(
            backend
                .attach(1, ProcessStartTime::from_raw(0))
                .expect_err("占位实现应报错"),
            crate::error::AppCatchError::Unsupported(_)
        ));
        assert!(matches!(
            backend.is_running(&handle).expect_err("占位实现应报错"),
            crate::error::AppCatchError::Unsupported(_)
        ));
        assert!(matches!(
            backend.wait(&handle).expect_err("占位实现应报错"),
            crate::error::AppCatchError::Unsupported(_)
        ));

        let icons = default_icon_capture();
        assert_eq!(icons.capture(&IconTarget::default()), None);
    }
}
