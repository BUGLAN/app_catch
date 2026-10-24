//! Windows 平台进程后端：Job Object 杀树 + 启动时间校验的 attach。
//!
//! 关键设计：
//!
//! 1. **进程容器**：每次 spawn 都新建一个 Job Object，设置 JOB_OBJECT_LIMIT_KILL_ON_JOB_CLOSE，
//!    并把子进程分配给该 Job。Windows 8+ 支持嵌套 Job，子进程再自行创建 Job 也不受影响；
//!    Job 句柄关闭（含启动器崩溃）时内核会回收整棵树，不会留下孤儿进程。
//! 2. **句柄生命周期**：所有 HANDLE 都由 RAII 封装（OwnedHandle / OwnedWinHandle），
//!    Drop 时调用 CloseHandle；不存在需要手工释放的分支，错误路径上也不会泄漏。
//! 3. **进程缓存**：wait 需要等待「已 spawn 的子进程」，而 tokio 的 Child 一旦被回收就无法由
//!    pid 重建（HANDLE 不跨进程有效）。因此后端内部按 start_time 缓存 Child，
//!    wait / kill_tree 优先取缓存；缓存缺失（重启后重关联）再回落到 attach 打开的查询句柄。
//! 4. **PID 复用判定**：attach 先 OpenProcess，再比对 GetProcessTimes 的创建时间与传入值。
//!    不一致说明 pid 已被别的进程占用，返回 Ok(None) 而不是误杀。

use std::collections::HashMap;
use std::os::windows::io::{AsRawHandle, FromRawHandle, OwnedHandle, RawHandle};
use std::sync::{Arc, Mutex};

use tokio::process::{Child, Command};
use tokio::sync::Mutex as AsyncMutex;
use windows_sys::Win32::Foundation::{
    FILETIME, GetLastError, HANDLE, STILL_ACTIVE, WAIT_FAILED, WAIT_OBJECT_0, WAIT_TIMEOUT,
};
use windows_sys::Win32::Security::SECURITY_ATTRIBUTES;
use windows_sys::Win32::Storage::FileSystem::SYNCHRONIZE;
use windows_sys::Win32::System::Diagnostics::ToolHelp::{
    CreateToolhelp32Snapshot, Process32FirstW, Process32NextW, PROCESSENTRY32W,
    TH32CS_SNAPPROCESS,
};
use windows_sys::Win32::System::JobObjects::{
    AssignProcessToJobObject, CreateJobObjectW, JOB_OBJECT_LIMIT_KILL_ON_JOB_CLOSE,
    JOBOBJECT_EXTENDED_LIMIT_INFORMATION, JobObjectExtendedLimitInformation,
    SetInformationJobObject, TerminateJobObject,
};
use windows_sys::Win32::System::Threading::{
    GetExitCodeProcess, GetProcessTimes, OpenProcess, PROCESS_QUERY_LIMITED_INFORMATION,
    PROCESS_SET_QUOTA, PROCESS_TERMINATE, TerminateProcess, WaitForSingleObject,
};

use crate::error::{AppCatchError, Result};
use crate::platform::{
    BackendFuture, ExitStatusInfo, IconCapture, IconTarget, ProcessBackend, ProcessHandle,
    ProcessStartTime, SpawnRequest, SpawnedProcess,
};

/// OpenProcess 需要的访问权限：等待退出 + 查询有限信息（兼容受保护进程）。
const PROCESS_ACCESS: u32 = SYNCHRONIZE | PROCESS_QUERY_LIMITED_INFORMATION;

/// 需要终止进程时追加的访问权限。
const PROCESS_ACCESS_TERMINATE: u32 = PROCESS_ACCESS | PROCESS_TERMINATE;

/// 把进程分配给 Job Object 所需的权限（`AssignProcessToJobObject` 要求 SET_QUOTA + TERMINATE）。
const PROCESS_ACCESS_ASSIGN: u32 = PROCESS_ACCESS | PROCESS_SET_QUOTA | PROCESS_TERMINATE;

/// 结束进程树时使用的退出码（非 0，便于在事件流里区分「被停止」）。
const KILLED_EXIT_CODE: u32 = 1;

/// 组装一条带 Win32 操作名的错误。
fn win32_error(op: &str) -> AppCatchError {
    // SAFETY: GetLastError 无副作用，任意线程、任意时刻可调用。
    let code = unsafe { GetLastError() };
    AppCatchError::Unsupported(format!(
        "{op} 失败（Win32 错误码 {code}）: {}",
        std::io::Error::from_raw_os_error(code as i32)
    ))
}

/// HANDLE 的 RAII 封装：离开作用域时由 OwnedHandle 调用 CloseHandle。
struct OwnedWinHandle(OwnedHandle);

impl OwnedWinHandle {
    /// 接管一个裸句柄；空句柄视为失败（返回 None）。
    fn from_raw(raw: HANDLE) -> Option<Self> {
        if raw.is_null() {
            return None;
        }
        // SAFETY: 句柄由 Win32 调用返回且非空，所有权自此转移给 OwnedHandle，
        // 它会在 Drop 时调用 CloseHandle 恰好一次，不会重复释放。
        unsafe { Some(Self(OwnedHandle::from_raw_handle(raw as RawHandle))) }
    }

    /// 取出裸句柄用于继续传给 Win32 调用（不转移所有权）。
    fn raw(&self) -> HANDLE {
        self.0.as_raw_handle() as HANDLE
    }
}

/// Job Object 句柄，Drop 时关闭。
///
/// 因为设置了 KILL_ON_JOB_CLOSE，关闭句柄等价于「回收整棵进程树」，
/// 这是本模块不留孤儿进程的根本保证。
struct JobHandle(OwnedWinHandle);

impl JobHandle {
    /// 新建匿名 Job 并开启 KILL_ON_JOB_CLOSE。
    fn create() -> Result<Self> {
        // SAFETY: 传空指针表示使用默认安全属性、匿名 Job；返回空句柄表示失败。
        let raw = unsafe {
            CreateJobObjectW(std::ptr::null::<SECURITY_ATTRIBUTES>(), std::ptr::null())
        };
        let owned = OwnedWinHandle::from_raw(raw).ok_or_else(|| win32_error("CreateJobObjectW"))?;

        let mut info = JOBOBJECT_EXTENDED_LIMIT_INFORMATION::default();
        info.BasicLimitInformation.LimitFlags = JOB_OBJECT_LIMIT_KILL_ON_JOB_CLOSE;
        // SAFETY: info 是合法的 JOBOBJECT_EXTENDED_LIMIT_INFORMATION，
        // 传入的 size 与结构体实际大小一致。
        let ok = unsafe {
            SetInformationJobObject(
                owned.raw(),
                JobObjectExtendedLimitInformation,
                &info as *const _ as *const core::ffi::c_void,
                std::mem::size_of::<JOBOBJECT_EXTENDED_LIMIT_INFORMATION>() as u32,
            )
        };
        if ok == 0 {
            return Err(win32_error("SetInformationJobObject"));
        }
        Ok(Self(owned))
    }

    /// 把已存在的进程加入本 Job。
    fn assign(&self, process: HANDLE) -> Result<()> {
        // SAFETY: Job 句柄由 self 持有；进程句柄由调用方保证有效。
        let ok = unsafe { AssignProcessToJobObject(self.0.raw(), process) };
        if ok == 0 {
            return Err(win32_error("AssignProcessToJobObject"));
        }
        Ok(())
    }

    /// 终止 Job 内的全部进程（含孙进程）。
    fn terminate(&self) -> Result<()> {
        // SAFETY: Job 句柄有效。
        let ok = unsafe { TerminateJobObject(self.0.raw(), KILLED_EXIT_CODE) };
        if ok == 0 {
            return Err(win32_error("TerminateJobObject"));
        }
        Ok(())
    }
}

/// 由 FILETIME 组合出 100ns 粒度的 u64 原始值。
fn filetime_to_u64(ft: &FILETIME) -> u64 {
    ((ft.dwHighDateTime as u64) << 32) | ft.dwLowDateTime as u64
}

/// 打开进程并读取其创建时间；进程不存在或无权限时返回 Ok(None)。
fn open_with_start_time(pid: u32, access: u32) -> Result<Option<(OwnedWinHandle, u64)>> {
    if pid == 0 {
        return Ok(None);
    }
    // SAFETY: OpenProcess 只读取系统状态；失败返回空句柄，由 from_raw 转成 None。
    let raw = unsafe { OpenProcess(access, 0, pid) };
    let Some(handle) = OwnedWinHandle::from_raw(raw) else {
        return Ok(None);
    };

    let zero = FILETIME {
        dwLowDateTime: 0,
        dwHighDateTime: 0,
    };
    let mut creation = zero;
    let mut exit = zero;
    let mut kernel = zero;
    let mut user = zero;
    // SAFETY: 四个输出指针都指向本栈上的有效 FILETIME。
    let ok = unsafe {
        GetProcessTimes(
            handle.raw(),
            &mut creation,
            &mut exit,
            &mut kernel,
            &mut user,
        )
    };
    if ok == 0 {
        return Ok(None);
    }
    Ok(Some((handle, filetime_to_u64(&creation))))
}

/// 用 ToolHelp 快照收集 pid 的全部子孙进程（自底向上顺序，便于先杀叶子）。
///
/// 仅用于「重启后重关联」场景：那时拿不到原 Job 句柄，只能靠父子关系枚举。
fn collect_descendants(root_pid: u32) -> Vec<u32> {
    // SAFETY: 快照 API 只读；返回 INVALID_HANDLE_VALUE 时由 from_raw 转成 None。
    let snapshot = unsafe { CreateToolhelp32Snapshot(TH32CS_SNAPPROCESS, 0) };
    let Some(snapshot) = OwnedWinHandle::from_raw(snapshot) else {
        return Vec::new();
    };

    let mut entries: Vec<(u32, u32)> = Vec::new();
    let mut entry: PROCESSENTRY32W = unsafe { std::mem::zeroed() };
    entry.dwSize = std::mem::size_of::<PROCESSENTRY32W>() as u32;

    // SAFETY: entry 已按约定设置 dwSize，是有效的输出缓冲区。
    let mut has_entry = unsafe { Process32FirstW(snapshot.raw(), &mut entry) } != 0;
    while has_entry {
        entries.push((entry.th32ProcessID, entry.th32ParentProcessID));
        // SAFETY: 同上，继续用同一个有效缓冲区。
        has_entry = unsafe { Process32NextW(snapshot.raw(), &mut entry) } != 0;
    }

    // 从根 pid 出发做广度优先生成后代集合
    let mut result = Vec::new();
    let mut frontier = vec![root_pid];
    while let Some(parent) = frontier.pop() {
        for (pid, ppid) in &entries {
            if *ppid == parent && *pid != root_pid && !result.contains(pid) {
                result.push(*pid);
                frontier.push(*pid);
            }
        }
    }
    // 先杀后代再杀父进程
    result.reverse();
    result
}

/// 列出某进程的全部子孙 pid（祖先在前、叶子在后）。
///
/// 供 kill_tree 的「重关联后无 Job 句柄」分支与验证测试使用。
pub fn descendant_pids(root_pid: u32) -> Vec<u32> {
    collect_descendants(root_pid)
}

/// 进程是否「仍在运行」且身份一致（pid + 启动时间都匹配）。
///
/// 关键区别：Windows 的进程对象在**还有句柄被打开**时会继续存在，即使进程已经终止。
/// 因此「能 OpenProcess 成功」不代表进程活着，必须看 WaitForSingleObject 的信号状态：
/// 返回 WAIT_TIMEOUT 才是还在运行，WAIT_OBJECT_0 表示已终止但对象尚未释放。
fn is_alive_matching(handle: &ProcessHandle) -> bool {
    match open_with_start_time(handle.pid, PROCESS_ACCESS) {
        Ok(Some((owned, start))) => {
            if start != handle.start_time.as_raw() {
                // pid 已被复用成别的进程：原进程已消失
                return false;
            }
            // SAFETY: 句柄有效；超时 0 只做一次状态查询。
            let waited = unsafe { WaitForSingleObject(owned.raw(), 0) };
            waited == WAIT_TIMEOUT
        }
        _ => false,
    }
}

/// 该 pid 是否出现在系统进程快照里（纯读取、无阻塞，且不依赖句柄打开）。
///
/// 与 [`is_process_alive`] 是两条独立证据：前者看「系统还认不认这个 pid」，
/// 后者看「进程对象的信号状态」。两者都为假才可断言进程确实消失。
pub fn pid_exists_in_snapshot(pid: u32) -> bool {
    if pid == 0 {
        return false;
    }
    // SAFETY: 快照 API 只读；返回 INVALID_HANDLE_VALUE 时由 from_raw 转成 None。
    let snapshot = unsafe { CreateToolhelp32Snapshot(TH32CS_SNAPPROCESS, 0) };
    let Some(snapshot) = OwnedWinHandle::from_raw(snapshot) else {
        return false;
    };

    let mut entry: PROCESSENTRY32W = unsafe { std::mem::zeroed() };
    entry.dwSize = std::mem::size_of::<PROCESSENTRY32W>() as u32;

    // SAFETY: entry 已按约定设置 dwSize，是有效的输出缓冲区。
    let mut has_entry = unsafe { Process32FirstW(snapshot.raw(), &mut entry) } != 0;
    while has_entry {
        if entry.th32ProcessID == pid {
            return true;
        }
        // SAFETY: 同上，继续用同一个有效缓冲区。
        has_entry = unsafe { Process32NextW(snapshot.raw(), &mut entry) } != 0;
    }
    false
}

/// 某 pid 当前是否存活。
///
/// 只做「打不开即不存在」的判定，不校验启动时间，因此只适合瞬时观察；
/// 需要防 PID 复用时请用 [`ProcessBackend::attach`]。
pub fn is_process_alive(pid: u32) -> bool {
    match open_with_start_time(pid, PROCESS_ACCESS) {
        Ok(Some((owned, _))) => {
            // SAFETY: 句柄有效；超时 0 只做一次状态查询。
            let waited = unsafe { WaitForSingleObject(owned.raw(), 0) };
            waited == WAIT_TIMEOUT
        }
        _ => false,
    }
}

/// 终止单个进程并等待其真正退出；进程已不存在视为成功。
fn terminate_and_wait(pid: u32) -> bool {
    match open_with_start_time(pid, PROCESS_ACCESS_TERMINATE) {
        Ok(Some((owned, _))) => {
            // SAFETY: 句柄以 PROCESS_TERMINATE 打开且有效。
            let killed = unsafe { TerminateProcess(owned.raw(), KILLED_EXIT_CODE) };
            // SAFETY: 句柄有效；无限等待到进程真正退出，保证调用方复查时 pid 已消失。
            let waited = unsafe { WaitForSingleObject(owned.raw(), u32::MAX) };
            killed != 0 || waited == WAIT_OBJECT_0
        }
        _ => true,
    }
}

/// 已 spawn 子进程的托管状态。
///
/// child 与 job 都用 `Arc` 共享：`wait` 与 `kill_tree` 会分别在两个任务上持有同一份记录，
/// 谁都不独占所有权，因此「一个任务在等退出、另一个任务在杀树」不会互相饿死。
struct TrackedChild {
    /// tokio 的子进程对象：持有进程句柄，等待退出走它最省事。
    child: Arc<AsyncMutex<Child>>,
    /// 该子进程所属的 Job；必须活到进程退出，随后随条目一起释放。
    job: Arc<JobHandle>,
}

/// Windows 进程后端。
///
/// 构造不失败；内部维护「start_time -> 子进程」的缓存。
pub struct WindowsProcessBackend {
    /// 按 start_time（进程身份的唯一部分）索引的在册子进程。
    children: Mutex<HashMap<u64, TrackedChild>>,
}

impl std::fmt::Debug for WindowsProcessBackend {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        let count = self
            .children
            .lock()
            .map(|g| g.len())
            .unwrap_or(0);
        f.debug_struct("WindowsProcessBackend")
            .field("tracked", &count)
            .finish()
    }
}

impl Default for WindowsProcessBackend {
    fn default() -> Self {
        Self::new()
    }
}

impl WindowsProcessBackend {
    /// 构造后端实例。
    pub fn new() -> Self {
        Self {
            children: Mutex::new(HashMap::new()),
        }
    }

    /// 取出缓存的子进程与 Job（不拥有，只是克隆共享句柄）。
    fn peek_child(&self, handle: &ProcessHandle) -> Option<(Arc<AsyncMutex<Child>>, Arc<JobHandle>)> {
        let guard = self.children.lock().unwrap_or_else(|e| e.into_inner());
        guard
            .get(&handle.start_time.as_raw())
            .map(|t| (Arc::clone(&t.child), Arc::clone(&t.job)))
    }

    /// 从缓存中移除并返回整条记录（Job 句柄随之转移）。
    fn remove_tracked(&self, handle: &ProcessHandle) -> Option<TrackedChild> {
        let mut guard = self.children.lock().unwrap_or_else(|e| e.into_inner());
        guard.remove(&handle.start_time.as_raw())
    }

    /// 查询进程是否仍在运行：系统调用路径已足够，且天然校验 start_time。
    ///
    /// 这里不走缓存里的 `Child::try_wait`：那需要 `&mut Child`，与等待退出任务争锁，
    /// 会让一次状态查询变成排队等待。
    fn running_state(&self, handle: &ProcessHandle) -> Result<bool> {
        match open_with_start_time(handle.pid, PROCESS_ACCESS)? {
            None => Ok(false),
            Some((owned, start)) => {
                if start != handle.start_time.as_raw() {
                    // pid 已复用成别的进程
                    return Ok(false);
                }
                // SAFETY: 句柄有效；超时 0 表示只做一次状态查询。
                let waited = unsafe { WaitForSingleObject(owned.raw(), 0) };
                Ok(waited == WAIT_TIMEOUT)
            }
        }
    }
}

impl ProcessBackend for WindowsProcessBackend {
    fn spawn<'a>(&'a self, request: &'a SpawnRequest) -> BackendFuture<'a, Result<SpawnedProcess>> {
        Box::pin(async move {
            let mut command = Command::new(&request.program);
            command.args(&request.args);
            command.stdout(std::process::Stdio::piped());
            command.stderr(std::process::Stdio::piped());
            // 不喂 stdin：交互式程序读 stdin 会立刻拿到 EOF，而不是挂住
            command.stdin(std::process::Stdio::null());
            if let Some(cwd) = &request.cwd {
                command.current_dir(cwd);
            }
            for (k, v) in &request.env {
                command.env(k, v);
            }

            let mut child = command.spawn().map_err(|e| AppCatchError::Io {
                path: request
                    .cwd
                    .clone()
                    .unwrap_or_else(|| std::path::PathBuf::from(format!("{}", request.program))),
                source: e,
            })?;

            let pid = match child.id() {
                Some(pid) => pid,
                None => {
                    let _ = child.kill().await;
                    return Err(AppCatchError::Unsupported(
                        "子进程启动后立即退出，拿不到 pid".to_string(),
                    ));
                }
            };

            // 先建 Job 再分配：任一步失败都要杀掉已启动的子进程，避免留下孤儿
            let job = match JobHandle::create() {
                Ok(job) => job,
                Err(e) => {
                    let _ = child.kill().await;
                    return Err(e);
                }
            };
            // 分配 Job 需要一个有 PROCESS_SET_QUOTA | PROCESS_TERMINATE 权限的进程句柄。
            // tokio 的 Child 不实现 std 的 AsRawHandle（它用的是 crate 私有的同名 trait），
            // 因此这里按 pid 重新打开一个句柄来完成分配，句柄由 OwnedWinHandle 持有并在下个语句后关闭。
            let assign_handle = match open_with_start_time(pid, PROCESS_ACCESS_ASSIGN) {
                Ok(Some((owned, _))) => owned,
                Ok(None) => {
                    let _ = child.kill().await;
                    return Err(AppCatchError::Unsupported(format!(
                        "无法打开子进程 {pid} 的句柄以分配 Job Object"
                    )));
                }
                Err(e) => {
                    let _ = child.kill().await;
                    return Err(e);
                }
            };
            if let Err(e) = job.assign(assign_handle.raw()) {
                let _ = child.kill().await;
                return Err(e);
            }
            // 分配完成后立即释放该句柄，Job 已经记住了子进程
            drop(assign_handle);

            // 读取创建时间作为进程身份；拿不到视为致命错误（无法做 PID 复用判定）
            let start_time = match open_with_start_time(pid, PROCESS_ACCESS) {
                Ok(Some((_, start))) => start,
                Ok(None) => {
                    let _ = child.kill().await;
                    return Err(AppCatchError::Unsupported(format!(
                        "无法读取子进程 {pid} 的启动时间"
                    )));
                }
                Err(e) => {
                    let _ = child.kill().await;
                    return Err(e);
                }
            };

            let stdout = child
                .stdout
                .take()
                .map(|s| Box::new(s) as Box<dyn tokio::io::AsyncRead + Send + Unpin>);
            let stderr = child
                .stderr
                .take()
                .map(|s| Box::new(s) as Box<dyn tokio::io::AsyncRead + Send + Unpin>);

            let handle = ProcessHandle::new(pid, ProcessStartTime::from_raw(start_time));
            {
                let mut guard = self.children.lock().unwrap_or_else(|e| e.into_inner());
                guard.insert(
                    start_time,
                    TrackedChild {
                        child: Arc::new(AsyncMutex::new(child)),
                        job: Arc::new(job),
                    },
                );
            }

            Ok(SpawnedProcess {
                handle,
                stdout,
                stderr,
            })
        })
    }

    fn kill_tree<'a>(&'a self, handle: &'a ProcessHandle) -> BackendFuture<'a, Result<()>> {
        Box::pin(async move {
            // 有缓存（本进程 spawn 的）：Job 一次性回收整棵树
            if let Some((_child, job)) = self.peek_child(handle) {
                // 必须在终止前记录子孙 pid：父进程一死，它们会被系统重新挂到别的父进程上，
                // 之后再按父子关系枚举就找不到它们了，也就无法确认是否真的被回收。
                let descendants = collect_descendants(handle.pid);
                let terminate_result = job.terminate();

                // 不在这里 Child::wait：那会和等待退出的任务抢同一把锁。
                // 改为按 pid 复查直到父与全部子孙消失，既确认终止生效又不争锁。
                let gone = wait_tree_gone(handle, &descendants, std::time::Duration::from_secs(15));

                return match terminate_result {
                    // Job 终止成功：进程树已回收（若仍有残留则等待超时，报错更诚实）
                    Ok(()) if gone => Ok(()),
                    Ok(()) => Err(AppCatchError::Unsupported(format!(
                        "终止进程树后仍有进程存活: pid={} 子孙={:?}",
                        handle.pid,
                        descendants
                            .iter()
                            .copied()
                            .filter(|p| is_process_alive(*p))
                            .collect::<Vec<_>>()
                    ))),
                    // Job 终止失败但进程树确实没了，也视为成功（可能已自行退出）
                    Err(_) if gone => Ok(()),
                    Err(e) => Err(e),
                };
            }

            // 无缓存（重启后重关联）：先校验身份，再按快照枚举子孙进程逐个终止
            match open_with_start_time(handle.pid, PROCESS_ACCESS)? {
                None => Ok(()),
                Some((_owned, start)) => {
                    if start != handle.start_time.as_raw() {
                        // pid 已被复用，绝不误杀
                        return Ok(());
                    }
                    for pid in collect_descendants(handle.pid) {
                        terminate_and_wait(pid);
                    }
                    terminate_and_wait(handle.pid);
                    Ok(())
                }
            }
        })
    }

    fn attach<'a>(
        &'a self,
        pid: u32,
        start_time: ProcessStartTime,
    ) -> BackendFuture<'a, Result<Option<ProcessHandle>>> {
        Box::pin(async move {
            match open_with_start_time(pid, PROCESS_ACCESS)? {
                None => Ok(None),
                Some((_owned, actual)) => {
                    if actual != start_time.as_raw() {
                        // 创建时间不一致：pid 已被复用，绝不能认作同一个进程
                        return Ok(None);
                    }
                    Ok(Some(ProcessHandle::new(pid, start_time)))
                }
            }
        })
    }

    fn is_running<'a>(&'a self, handle: &'a ProcessHandle) -> BackendFuture<'a, Result<bool>> {
        Box::pin(async move { self.running_state(handle) })
    }

    fn wait<'a>(&'a self, handle: &'a ProcessHandle) -> BackendFuture<'a, Result<ExitStatusInfo>> {
        Box::pin(async move {
            if let Some((child, _job)) = self.peek_child(handle) {
                let status = {
                    let mut guard = child.lock().await;
                    guard
                        .wait()
                        .await
                        .map_err(|e| AppCatchError::Unsupported(format!("等待子进程失败: {e}")))?
                };
                // 进程已结束：把记录（含 Job 句柄）摘下，Job/KILL_ON_JOB_CLOSE 随之回收残树
                self.remove_tracked(handle);
                return Ok(ExitStatusInfo {
                    code: status.code(),
                });
            }

            // 无缓存：观察已存在的进程，等它结束后取退出码
            match open_with_start_time(handle.pid, PROCESS_ACCESS)? {
                None => Ok(ExitStatusInfo { code: None }),
                Some((owned, start)) => {
                    if start != handle.start_time.as_raw() {
                        return Ok(ExitStatusInfo { code: None });
                    }
                    // SAFETY: 句柄有效；无限等待进程结束。
                    let waited = unsafe { WaitForSingleObject(owned.raw(), u32::MAX) };
                    if waited == WAIT_FAILED {
                        return Err(win32_error("WaitForSingleObject"));
                    }
                    observe_exit_code(owned.raw())
                }
            }
        })
    }
}

/// 等待「父进程 + 全部子孙」都消失，返回是否在超时前全部消失。
///
/// `TerminateJobObject` 是异步生效的：调用返回时进程可能还在收尾，
/// 因此必须复查，否则调用方紧接着检查 pid 会看到「还以为活着」的假象。
fn wait_tree_gone(handle: &ProcessHandle, descendants: &[u32], timeout: std::time::Duration) -> bool {
    let deadline = std::time::Instant::now() + timeout;
    loop {
        let parent_gone = !is_alive_matching(handle);
        let children_gone = descendants.iter().all(|pid| !is_process_alive(*pid));
        if parent_gone && children_gone {
            return true;
        }
        if std::time::Instant::now() >= deadline {
            return false;
        }
        std::thread::sleep(std::time::Duration::from_millis(5));
    }
}


/// 读取退出码；进程仍在运行（STILL_ACTIVE）时返回 None。
fn observe_exit_code(process: HANDLE) -> Result<ExitStatusInfo> {
    let mut code: u32 = 0;
    // SAFETY: 句柄有效，输出指针指向本栈变量。
    let ok = unsafe { GetExitCodeProcess(process, &mut code) };
    if ok == 0 {
        return Err(win32_error("GetExitCodeProcess"));
    }
    if code == STILL_ACTIVE as u32 {
        return Ok(ExitStatusInfo { code: None });
    }
    Ok(ExitStatusInfo {
        code: Some(code as i32),
    })
}

/// Windows 图标捕获（M3 落地真实抓取，本阶段恒返回 None 走兜底图）。
#[derive(Debug, Default, Clone, Copy)]
pub struct WindowsIconCapture;

impl WindowsIconCapture {
    /// 构造图标捕获实现。
    pub fn new() -> Self {
        Self
    }
}

impl IconCapture for WindowsIconCapture {
    fn capture(&self, _target: &IconTarget) -> Option<std::path::PathBuf> {
        // 尽力而为的接口约定：抓不到就返回 None，由调用方走内置兜底图
        None
    }
}
