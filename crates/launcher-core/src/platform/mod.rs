//! 平台抽象层：进程后端与图标捕获。
//!
//! 需求基线要求「平台相关代码走抽象层，不锁死单一平台」：
//! - [`ProcessBackend`]：spawn / kill_tree / attach（按 pid + 进程启动时间重关联，防 PID 复用）；
//! - [`IconCapture`]：从可执行文件或运行中窗口尽力抓取图标；
//! - 具体平台实现按 `cfg` 选择：Windows 见 [`windows`]，其它平台见 [`fallback`]。
//!
//! 运行时语义（M1b）：
//! - 所有方法都是 **async**：等待子进程与读写管道都在 tokio 运行时内进行；
//! - [`SpawnedProcess`] 把 stdout/stderr 的读取端直接交给调用方，
//!   类型是跨平台的 [`tokio::io::AsyncRead`] 对象，**不泄漏任何平台句柄类型**；
//! - 子进程句柄由后端在内部持有（RAII），调用方只需要 [`ProcessHandle`] 这个稳定身份。
//!
//! 注意 trait 对象安全：为了让 [`SessionManager`](crate::runtime::SessionManager) 能持有
//! `Arc<dyn ProcessBackend>`，这里的 async 方法统一返回装箱 future（`Pin<Box<dyn Future>>`），
//! 而不是使用 `async fn in trait`（后者在保持 dyn 兼容前需要额外胶水）。

use std::collections::BTreeMap;
use std::future::Future;
use std::path::PathBuf;
use std::pin::Pin;

use crate::config::AppId;
use crate::error::Result;
use crate::session::SessionId;

#[cfg(not(windows))]
pub mod fallback;
/// 图标核心：位图规范化、PNG 编解码、缓存判定（**不含平台 API**，任何平台都能单测）。
pub mod icon;
/// Windows 图标抓取：窗口 HICON / exe 资源图标 -> PNG 缓存。
#[cfg(windows)]
#[allow(unsafe_code)]
pub mod icon_windows;
/// Windows 实现直接调用 Win32 API，是本 crate 允许 `unsafe` 的模块之一。
#[cfg(windows)]
#[allow(unsafe_code)]
pub mod windows;

/// Windows 监听端口采集（GetExtendedTcpTable），尽力而为、失败静默。
#[cfg(windows)]
#[allow(unsafe_code)]
pub mod ports;

/// Windows 窗口标题采集（EnumWindows / GetWindowTextW），尽力而为、失败静默。
#[cfg(windows)]
#[allow(unsafe_code)]
pub mod window;

pub use icon::{cache_path, CacheState, RgbaImage, ICON_MAX_SIZE};
#[cfg(windows)]
pub use icon_windows::{file_icon, window_icon_for_pids, WindowsIconCapture};
#[cfg(windows)]
pub use ports::{listening_ports_for_pid, listening_ports_for_pids};
#[cfg(windows)]
pub use window::{main_window_for_pids, window_title_for_pid, window_title_for_pids};
#[cfg(windows)]
pub use windows::{descendant_pids, is_process_alive, pid_exists_in_snapshot, WindowsProcessBackend};
#[cfg(not(windows))]
pub use fallback::{FallbackIconCapture, FallbackProcessBackend};

/// 收集一个会话进程树的全部 pid（根 pid 在前，子孙在后）。
///
/// 状态采集的两个 probe 都需要「根 + 子孙」这个集合：
/// GUI 应用常见「启动器拉起的是 shim，真正的窗口/端口属于它的子进程」。
/// 非 Windows 平台没有进程树枚举能力，只返回根 pid 本身。
#[cfg(windows)]
pub fn collect_tree_pids(root_pid: u32) -> Vec<u32> {
    if root_pid == 0 {
        return Vec::new();
    }
    let mut pids = Vec::with_capacity(8);
    pids.push(root_pid);
    pids.extend(descendant_pids(root_pid));
    pids
}

/// 非 Windows 平台：没有进程树枚举能力，只返回根 pid 本身。
#[cfg(not(windows))]
pub fn collect_tree_pids(root_pid: u32) -> Vec<u32> {
    if root_pid == 0 {
        Vec::new()
    } else {
        vec![root_pid]
    }
}

/// 平台后端方法返回的装箱 future（保证 trait 对象安全）。
pub type BackendFuture<'a, T> = Pin<Box<dyn Future<Output = T> + Send + 'a>>;

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
/// 平台语义：
/// - Windows：进程创建时间的 FILETIME，单位为 100ns，自 1601-01-01 UTC 起算；
/// - macOS：`proc_pidinfo` 返回的 `pbi_start_tvsec/nsec` 组合。
///
/// 存储为原始 `u64`，与 [`SessionEvent::Start`](crate::session::SessionEvent::Start)
/// 中记录的 pid 配合使用即可判定身份。跨平台比较没有意义。
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct ProcessStartTime(u64);

impl ProcessStartTime {
    /// 用平台的原始值构造；具体含义由平台决定。
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

    /// 命令行参数不含程序名的完整向量（含 argv[0]），用于写进 start 事件。
    pub fn argv(&self) -> Vec<String> {
        let mut argv = Vec::with_capacity(self.args.len() + 1);
        argv.push(self.program.clone());
        argv.extend(self.args.iter().cloned());
        argv
    }
}

/// 启动结果：进程身份 + 两个输出读取端。
///
/// 读取端是 [`tokio::io::AsyncRead`] trait 对象，调用方（捕获管线）只依赖标准异步 IO，
/// 不需要知道底下是 Windows 的匿名管道还是别的平台实现。
pub struct SpawnedProcess {
    /// 进程身份（pid + 启动时间）。
    pub handle: ProcessHandle,
    /// 子进程 stdout 读取端；后端未接管道时为 `None`。
    pub stdout: Option<Box<dyn tokio::io::AsyncRead + Send + Unpin>>,
    /// 子进程 stderr 读取端；后端未接管道时为 `None`。
    pub stderr: Option<Box<dyn tokio::io::AsyncRead + Send + Unpin>>,
}

impl std::fmt::Debug for SpawnedProcess {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("SpawnedProcess")
            .field("handle", &self.handle)
            .field("has_stdout", &self.stdout.is_some())
            .field("has_stderr", &self.stderr.is_some())
            .finish()
    }
}

impl SpawnedProcess {
    /// 只带进程身份构造（无输出管道），供不接管道的调用方与测试使用。
    pub fn without_pipes(handle: ProcessHandle) -> Self {
        Self {
            handle,
            stdout: None,
            stderr: None,
        }
    }
}

/// 进程退出信息。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ExitStatusInfo {
    /// 退出码；被信号杀死时通常为 `None`。
    pub code: Option<i32>,
}

/// 平台进程后端：启动、回收进程树、重启后重新关联。
///
/// 实现要求：
/// - `spawn` 必须把子进程放进可整体回收的容器（Windows Job Object / macOS 进程组）；
/// - `kill_tree` 回收整棵进程树，不能只杀父进程；
/// - `attach` 必须同时校验 pid 与启动时间，避免 PID 复用导致误判。
///
/// 所有方法都是 async，返回装箱 future 以保持 trait 对象安全。
pub trait ProcessBackend: Send + Sync {
    /// 启动子进程，并把 stdout/stderr 读取端交出来。
    fn spawn<'a>(&'a self, request: &'a SpawnRequest) -> BackendFuture<'a, Result<SpawnedProcess>>;

    /// 回收整棵进程树（含孙进程），随后等待父进程结束。
    fn kill_tree<'a>(&'a self, handle: &'a ProcessHandle) -> BackendFuture<'a, Result<()>>;

    /// 按 `pid` + 进程启动时间重新关联一个已存在的进程。
    ///
    /// 返回 `Ok(None)` 表示进程已不存在或启动时间不匹配（即 PID 已被复用），
    /// 返回 `Err` 表示查询本身失败（权限、系统调用错误）。
    fn attach<'a>(
        &'a self,
        pid: u32,
        start_time: ProcessStartTime,
    ) -> BackendFuture<'a, Result<Option<ProcessHandle>>>;

    /// 判断进程是否仍然存活（同样校验启动时间）。
    fn is_running<'a>(&'a self, handle: &'a ProcessHandle) -> BackendFuture<'a, Result<bool>>;

    /// 等待进程退出并返回退出信息。
    fn wait<'a>(&'a self, handle: &'a ProcessHandle) -> BackendFuture<'a, Result<ExitStatusInfo>>;

    /// 列出该会话进程树中除根进程外的全部子孙 pid。
    ///
    /// 这是**可选能力**：返回值语义是「能给出时才给」，因此用 `Option` 而不是空 `Vec`
    /// 区分「确实没有子孙」和「本后端不支持查询」——前者是有效的采集结果，
    /// 后者需要调用方回退到别的途径（如平台无关的快照枚举）。
    ///
    /// 为什么放在后端 trait 上：只有后端知道「这棵树的权威成员是谁」。
    /// Windows 实现给每个会话建了 Job Object，成员表由内核维护，
    /// 一次查询实测 0.3 微秒；而通用的「扫全系统进程表 + 按父子关系 BFS」
    /// 在同一台机器上要 330 毫秒，且中间进程退出后会断链。
    ///
    /// 默认实现返回 `None`（不支持），保持向后兼容：
    /// 非 Windows 后端与测试用的假后端无需实现它。
    fn descendant_pids(&self, _handle: &ProcessHandle) -> Option<Vec<u32>> {
        None
    }

}

/// 图标捕获目标：应用身份 + 可执行文件 + （可能的）运行中窗口信息。
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct IconTarget {
    /// 应用 id（M3 新增）：决定缓存文件名 `<home>/icons/<app_id>.png`。
    ///
    /// 没有它就无法按应用维度的缓存，因此捕获入口必须有 app_id；
    /// 缺失时 [`IconCapture::capture`] 直接返回 `None`（走前端内置兜底图）。
    pub app_id: Option<AppId>,
    /// 用户在配置里指定的自定义图标（`AppConfig.icon`），优先级最高。
    pub custom_icon: Option<PathBuf>,
    /// 可执行文件路径（Windows 从 exe 资源取图标）。
    pub executable: Option<PathBuf>,
    /// 关联进程 pid（用于按窗口取图标）。
    pub pid: Option<u32>,
    /// 窗口标题（辅助定位目标窗口）。
    pub window_title: Option<String>,
}

impl IconTarget {
    /// 只按可执行文件抓取（无应用 id 时不会落缓存）。
    pub fn from_executable(path: impl Into<PathBuf>) -> Self {
        Self {
            executable: Some(path.into()),
            ..Self::default()
        }
    }

    /// 按应用身份构造：缓存路径与优先级链都需要它。
    pub fn for_app(app_id: AppId) -> Self {
        Self {
            app_id: Some(app_id),
            ..Self::default()
        }
    }

    /// 补上自定义图标（builder 风格，便于命令层链式构造）。
    #[must_use]
    pub fn with_custom_icon(mut self, icon: Option<PathBuf>) -> Self {
        self.custom_icon = icon;
        self
    }

    /// 补上运行中会话的根 pid。
    #[must_use]
    pub fn with_pid(mut self, pid: Option<u32>) -> Self {
        self.pid = pid;
        self
    }

    /// 补上可执行文件路径。
    #[must_use]
    pub fn with_executable(mut self, path: Option<PathBuf>) -> Self {
        self.executable = path;
        self
    }
}

/// 图标捕获：尽力而为，失败（含无窗口服务类应用）返回 `None`，由调用方走内置兜底图。
///
/// 实现约定（M3）：
/// - **绝不返回错误**：抓不到就是 `None`，不写日志、不影响会话启动；
/// - **必须能重入**：命令层在会话启动后会再调一次拿「运行时图标」；
/// - **可能较慢**：内部会调 Shell API（`PrivateExtractIconsW`/`SHGetFileInfoW`），
///   调用方应放进 `spawn_blocking`（见 `src-tauri` 的 `get_app_icon` 命令）。
pub trait IconCapture: Send + Sync {
    /// 抓取图标并返回缓存后的 PNG 路径；抓不到返回 `None`。
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
    fn spawn_request_argv_includes_program_first() {
        let mut request = SpawnRequest::new(
            SessionId::new_v4(),
            AppId::new("demo").expect("合法 id"),
            "python",
        );
        request.args.push("-u".to_string());
        request.args.push("main.py".to_string());

        // argv 必须能精确还原启动方式：argv[0] 是程序本身
        assert_eq!(request.argv(), vec!["python", "-u", "main.py"]);
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
    fn spawned_process_debug_does_not_leak_pipe_details() {
        let spawned = SpawnedProcess::without_pipes(ProcessHandle::new(
            7,
            ProcessStartTime::from_raw(9),
        ));
        let text = format!("{spawned:?}");
        assert!(text.contains("has_stdout: false"), "{text}");
        assert!(text.contains("has_stderr: false"), "{text}");
    }

    #[test]
    fn icon_target_from_executable_has_no_pid() {
        let target = IconTarget::from_executable("C:/Windows/notepad.exe");
        assert_eq!(target.executable, Some(PathBuf::from("C:/Windows/notepad.exe")));
        assert_eq!(target.pid, None);
        assert_eq!(target.app_id, None, "未指定应用时不该有缓存身份");
        assert_eq!(
            IconTarget::default(),
            IconTarget {
                app_id: None,
                custom_icon: None,
                executable: None,
                pid: None,
                window_title: None,
            }
        );
    }

    /// builder 链：命令层按「应用 id + 自定义图标 + exe + pid」逐项补齐目标。
    #[test]
    fn icon_target_builder_chain_fills_all_slots() {
        let target = IconTarget::for_app(AppId::new("demo").expect("合法 id"))
            .with_custom_icon(Some(PathBuf::from("C:/x/icon.png")))
            .with_executable(Some(PathBuf::from("C:/x/app.exe")))
            .with_pid(Some(4242));

        assert_eq!(target.app_id.as_ref().map(AppId::as_str), Some("demo"));
        assert_eq!(target.custom_icon, Some(PathBuf::from("C:/x/icon.png")));
        assert_eq!(target.executable, Some(PathBuf::from("C:/x/app.exe")));
        assert_eq!(target.pid, Some(4242));
    }

    /// 缺少 app_id 时图标捕获必须安静地返回 None（没有缓存身份）。
    #[test]
    fn capture_without_app_id_returns_none() {
        let capture = default_icon_capture();
        assert_eq!(capture.capture(&IconTarget::default()), None);
        assert_eq!(
            capture.capture(&IconTarget::from_executable("C:/x/app.exe")),
            None
        );
    }

    /// 占位实现必须返回明确错误而不是 panic。
    ///
    /// M1b 之后 Windows 后端已是真实实现，这条「未实现」契约由非 Windows 的 fallback 后端继续保证；
    /// 测试本身在所有平台编译，只是运行条件随平台切换。
    #[cfg(not(windows))]
    #[test]
    fn placeholder_backend_reports_unsupported() {
        let backend = default_process_backend();
        let request = SpawnRequest::new(
            SessionId::new_v4(),
            AppId::new("demo").expect("合法 id"),
            "cmd.exe",
        );

        assert!(matches!(
            backend.spawn(&request).await.expect_err("占位实现应报错"),
            crate::error::AppCatchError::Unsupported(_)
        ));
    }

    /// Windows 后端必须真的能起进程：这是 M1b 之后替代占位测试的冒烟检查。
    #[cfg(windows)]
    #[test]
    fn windows_backend_spawns_real_process() {
        use crate::platform::ProcessBackend;

        let runtime = tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()
            .expect("构建 tokio 运行时失败");

        runtime.block_on(async {
            let backend = default_process_backend();
            let request = SpawnRequest::new(
                SessionId::new_v4(),
                AppId::new("demo").expect("合法 id"),
                "cmd.exe",
            );
            let spawned = backend.spawn(&request).await.expect("启动 cmd.exe 应成功");
            assert!(spawned.handle.pid > 0, "应拿到真实 pid");
            assert!(spawned.stdout.is_some(), "应交出 stdout 读取端");
            assert!(spawned.stderr.is_some(), "应交出 stderr 读取端");

            let exit = backend.wait(&spawned.handle).await.expect("等待退出失败");
            assert_eq!(exit.code, Some(0), "cmd.exe 无参数应正常退出");
            assert!(!backend.is_running(&spawned.handle).await.expect("查询状态失败"));
        });
    }
}