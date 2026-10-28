//! Unix（macOS / Linux）进程后端。
//!
//! 与 Windows 后端（Job Object）对应的设计：
//!
//! 1. **进程组即"容器"**：子进程以 `process_group(0)` 启动，成为新进程组的组长（pgid == pid）。
//!    `pnpm → node → vite / electron` 这类默认继承进程组的子孙都在组里，`killpg` 一次覆盖。
//! 2. **杀树 = 先礼后兵**：先对整组发 SIGTERM，给 Electron / Vite 机会自行收尾（关窗口、释放端口），
//!    宽限期后仍有存活再 SIGKILL。脚本里用 `detached` / `setsid` 脱离进程组的孙进程，
//!    靠终止前拍下的父子关系快照逐个补发信号——父进程一死它们会被 launchd/init 收养，事后就找不到了。
//! 3. **进程身份 = pid + 启动时间**：macOS 读 `proc_pidinfo(PROC_PIDTBSDINFO)` 的
//!    `pbi_start_tvsec/usec`，Linux 读 `/proc/<pid>/stat` 第 22 列；防 PID 复用误判 / 误杀。
//! 4. **子进程缓存**：本进程 spawn 的子进程按 start_time 缓存 tokio `Child`，`wait` 由它回收僵尸并拿退出码；
//!    启动器重启后重关联的进程不是本进程的子进程，只能轮询到消失，退出码未知（`None`）。
//! 5. **PATH**：从 Finder / Dock 启动的 .app 只有 `/usr/bin:/bin:/usr/sbin:/sbin`，
//!    `pnpm` / `node` / Homebrew 工具全找不到。读一次用户登录 shell 的 PATH 并缓存，
//!    合并进子进程环境（配置里显式写了 PATH 时以配置为准）。
//!    交互式 shell 带 oh-my-zsh / conda 时要 ~3 秒，所以后端一构造就在后台线程预热，
//!    用户第一次点「启动」时通常已经读好了。
//!
//! 本模块是 crate 里允许 `unsafe` 的模块之一：只用于 `kill` / `killpg` / `proc_pidinfo` 三个 libc 调用。

use std::collections::{HashMap, HashSet, VecDeque};
use std::io::Read;
use std::sync::{Arc, Mutex, OnceLock};
use std::time::Duration;

use tokio::process::{Child, Command};
use tokio::sync::Mutex as AsyncMutex;

use crate::error::{AppCatchError, Result};
use crate::platform::{
    BackendFuture, ExitStatusInfo, ProcessBackend, ProcessHandle, ProcessStartTime, SpawnRequest,
    SpawnedProcess,
};

/// SIGTERM 之后等待整棵树自行退出的宽限期。
const TERM_GRACE: Duration = Duration::from_secs(5);
/// SIGKILL 之后等待内核回收的上限。
const KILL_GRACE: Duration = Duration::from_secs(5);
/// 杀树期间复查存活的间隔。
const KILL_POLL: Duration = Duration::from_millis(25);
/// 重关联进程（非本进程子进程）等待退出时的轮询间隔。
const ATTACHED_POLL: Duration = Duration::from_millis(500);
/// 读取登录 shell PATH 的超时：rc 文件里有慢命令时宁可放弃，也不能卡住启动。
const SHELL_PATH_TIMEOUT: Duration = Duration::from_secs(5);

// ---------------------------------------------------------------------------
// 进程身份
// ---------------------------------------------------------------------------

/// 读到的进程身份信息。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
struct ProcessIdentity {
    /// 平台原始启动时间（macOS：微秒级 epoch；Linux：开机后的 clock ticks）。
    start: u64,
    /// 是否已是僵尸（已退出、尚未被父进程回收）。
    zombie: bool,
}

/// macOS：`proc_pidinfo(PROC_PIDTBSDINFO)`。
#[cfg(target_os = "macos")]
fn read_identity(pid: u32) -> Option<ProcessIdentity> {
    /// `<sys/proc.h>` 的 SZOMB：libc crate 未在 apple 目标上导出它。
    const SZOMB: u32 = 5;
    let pid = libc::c_int::try_from(pid).ok()?;
    let size = std::mem::size_of::<libc::proc_bsdinfo>() as libc::c_int;
    // SAFETY: proc_bsdinfo 是纯数据结构，全零是合法初值。
    let mut info: libc::proc_bsdinfo = unsafe { std::mem::zeroed() };
    // SAFETY: 缓冲区指针与长度都来自上面的栈变量，内核只按 size 写入。
    let written = unsafe {
        libc::proc_pidinfo(
            pid,
            libc::PROC_PIDTBSDINFO,
            0,
            (&mut info as *mut libc::proc_bsdinfo).cast::<libc::c_void>(),
            size,
        )
    };
    if written != size {
        return None;
    }
    Some(ProcessIdentity {
        start: info
            .pbi_start_tvsec
            .saturating_mul(1_000_000)
            .saturating_add(info.pbi_start_tvusec),
        zombie: info.pbi_status == SZOMB,
    })
}

/// Linux 等：解析 `/proc/<pid>/stat`。
#[cfg(not(target_os = "macos"))]
fn read_identity(pid: u32) -> Option<ProcessIdentity> {
    let stat = std::fs::read_to_string(format!("/proc/{pid}/stat")).ok()?;
    parse_proc_stat(&stat)
}

/// 解析 `/proc/<pid>/stat`（纯函数，可在任意平台单测）。
///
/// 第 2 列 comm 用括号包裹且可能含空格与括号，必须从**最后一个** `)` 之后再按空白切分：
/// 之后第 0 项是第 3 列 state，第 19 项是第 22 列 starttime。
#[cfg_attr(target_os = "macos", allow(dead_code))]
fn parse_proc_stat(stat: &str) -> Option<ProcessIdentity> {
    let rest = stat.get(stat.rfind(')')? + 1..)?;
    let fields: Vec<&str> = rest.split_whitespace().collect();
    let state = *fields.first()?;
    let start = fields.get(19)?.parse().ok()?;
    Some(ProcessIdentity {
        start,
        zombie: state == "Z" || state == "X",
    })
}

/// 进程是否以该身份**活着**（僵尸视为已退出）。
fn is_alive_matching(handle: &ProcessHandle) -> bool {
    matches!(
        read_identity(handle.pid),
        Some(identity) if !identity.zombie && identity.start == handle.start_time.as_raw()
    )
}

// ---------------------------------------------------------------------------
// 信号
// ---------------------------------------------------------------------------

/// 向整个进程组发信号；返回组是否存在（ESRCH = 组内已无进程）。
fn signal_group(pgid: u32, signal: libc::c_int) -> bool {
    let Ok(pgid) = libc::pid_t::try_from(pgid) else {
        return false;
    };
    // SAFETY: killpg 只读取两个整数参数；pgid > 0 由调用方保证（来自真实 pid）。
    unsafe { libc::killpg(pgid, signal) == 0 }
}

/// 向单个进程发信号；返回进程是否存在。
fn signal_pid(pid: u32, signal: libc::c_int) -> bool {
    let Ok(pid) = libc::pid_t::try_from(pid) else {
        return false;
    };
    if pid <= 0 {
        // 0 / 负数会被解释成"整个进程组"，绝不能误传
        return false;
    }
    // SAFETY: kill 只读取两个整数参数，pid 已保证为正。
    unsafe { libc::kill(pid, signal) == 0 }
}

/// 进程组是否还有成员（信号 0 只做存在性检查）。
fn group_alive(pgid: u32) -> bool {
    signal_group(pgid, 0)
}

/// 单个进程是否还在（含僵尸：僵尸会很快被收养者回收，等它消失即可）。
fn pid_alive(pid: u32) -> bool {
    signal_pid(pid, 0) && !matches!(read_identity(pid), Some(identity) if identity.zombie)
}

// ---------------------------------------------------------------------------
// 进程树快照
// ---------------------------------------------------------------------------

/// 解析 `ps -A -o pid=,ppid=` 的输出为 (pid, ppid) 对（纯函数）。
fn parse_pid_ppid(text: &str) -> Vec<(u32, u32)> {
    text.lines()
        .filter_map(|line| {
            let mut parts = line.split_whitespace();
            let pid = parts.next()?.parse().ok()?;
            let ppid = parts.next()?.parse().ok()?;
            Some((pid, ppid))
        })
        .collect()
}

/// 从 (pid, ppid) 表里 BFS 出 `root` 的全部子孙（不含 root，纯函数）。
fn descendants_from(pairs: &[(u32, u32)], root: u32) -> Vec<u32> {
    let mut children: HashMap<u32, Vec<u32>> = HashMap::new();
    for &(pid, ppid) in pairs {
        if pid != ppid {
            children.entry(ppid).or_default().push(pid);
        }
    }
    let mut seen = HashSet::new();
    let mut out = Vec::new();
    let mut queue = VecDeque::from([root]);
    while let Some(current) = queue.pop_front() {
        for &child in children.get(&current).map(Vec::as_slice).unwrap_or(&[]) {
            if child != root && seen.insert(child) {
                out.push(child);
                queue.push_back(child);
            }
        }
    }
    out
}

/// 拍一张全系统父子关系快照，返回 `root` 的子孙 pid；`ps` 不可用时返回空。
async fn snapshot_descendants(root: u32) -> Vec<u32> {
    let output = Command::new("/bin/ps")
        .args(["-A", "-o", "pid=,ppid="])
        .stdin(std::process::Stdio::null())
        .stderr(std::process::Stdio::null())
        .output()
        .await;
    match output {
        Ok(out) if out.status.success() => {
            descendants_from(&parse_pid_ppid(&String::from_utf8_lossy(&out.stdout)), root)
        }
        _ => Vec::new(),
    }
}

// ---------------------------------------------------------------------------
// 登录 shell 的 PATH
// ---------------------------------------------------------------------------

/// 输出标记：rc 文件可能往 stdout 打印欢迎语，只取标记之间的内容。
const PATH_BEGIN: &str = "__APP_CATCH_PATH_BEGIN__";
const PATH_END: &str = "__APP_CATCH_PATH_END__";

/// 从 shell 输出里抠出 PATH（纯函数）。
fn extract_marked_path(output: &str) -> Option<String> {
    let start = output.rfind(PATH_BEGIN)? + PATH_BEGIN.len();
    let end = start + output.get(start..)?.find(PATH_END)?;
    let path = output.get(start..end)?.trim();
    (!path.is_empty()).then(|| path.to_string())
}

/// 合并 PATH：`preferred` 在前，`fallback` 中未出现的条目追加在后，去重保序（纯函数）。
fn merge_path(preferred: &str, fallback: &str) -> String {
    let mut seen = HashSet::new();
    preferred
        .split(':')
        .chain(fallback.split(':'))
        .filter(|entry| !entry.is_empty() && seen.insert(*entry))
        .collect::<Vec<_>>()
        .join(":")
}

/// 用户登录 shell 的 PATH（进程内只读一次；失败缓存为 `None`，不重试）。
///
/// 同步实现：既能在后台线程预热，也能在 `spawn_blocking` 里取用。
/// `OnceLock` 保证并发调用只会真正执行一次，后到者阻塞等待同一个结果。
fn login_shell_path() -> Option<&'static str> {
    static CACHE: OnceLock<Option<String>> = OnceLock::new();
    CACHE.get_or_init(read_login_shell_path).as_deref()
}

fn read_login_shell_path() -> Option<String> {
    let shell = std::env::var("SHELL").unwrap_or_else(|_| "/bin/zsh".to_string());
    let script = format!("printf '{PATH_BEGIN}%s{PATH_END}' \"$PATH\"");
    // -i：PATH 常写在 .zshrc / .bashrc（nvm、pnpm 的安装脚本都往那里写）；-l：读 profile
    let mut child = std::process::Command::new(shell)
        .args(["-ilc", &script])
        .stdin(std::process::Stdio::null())
        .stdout(std::process::Stdio::piped())
        .stderr(std::process::Stdio::null())
        .spawn()
        .ok()?;
    let deadline = std::time::Instant::now() + SHELL_PATH_TIMEOUT;
    loop {
        match child.try_wait() {
            Ok(Some(_)) => break,
            Ok(None) if std::time::Instant::now() < deadline => {
                std::thread::sleep(Duration::from_millis(20));
            }
            _ => {
                // 超时或查询失败：rc 文件里有慢命令时宁可放弃，也不能卡住启动
                let _ = child.kill();
                let _ = child.wait();
                return None;
            }
        }
    }
    let mut output = String::new();
    child.stdout.take()?.read_to_string(&mut output).ok()?;
    extract_marked_path(&output)
}

// ---------------------------------------------------------------------------
// 后端
// ---------------------------------------------------------------------------

fn now_micros() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| u64::try_from(d.as_micros()).unwrap_or(u64::MAX))
        .unwrap_or(0)
}

/// Unix 进程后端。
///
/// 构造不失败；内部维护「start_time -> 子进程」的缓存，语义与 Windows 后端一致。
pub struct UnixProcessBackend {
    children: Mutex<HashMap<u64, Arc<AsyncMutex<Child>>>>,
}

impl std::fmt::Debug for UnixProcessBackend {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        let count = self.children.lock().map(|g| g.len()).unwrap_or(0);
        f.debug_struct("UnixProcessBackend")
            .field("tracked", &count)
            .finish()
    }
}

impl Default for UnixProcessBackend {
    fn default() -> Self {
        Self::new()
    }
}

impl UnixProcessBackend {
    /// 构造后端实例，并在后台线程预热登录 shell 的 PATH。
    pub fn new() -> Self {
        static PREWARM: std::sync::Once = std::sync::Once::new();
        PREWARM.call_once(|| {
            // 线程起不来也无妨：首次 spawn 时会同步读取
            let _ = std::thread::Builder::new()
                .name("app-catch-shell-path".into())
                .spawn(|| {
                    login_shell_path();
                });
        });
        Self {
            children: Mutex::new(HashMap::new()),
        }
    }

    fn peek_child(&self, handle: &ProcessHandle) -> Option<Arc<AsyncMutex<Child>>> {
        let guard = self.children.lock().unwrap_or_else(|e| e.into_inner());
        guard.get(&handle.start_time.as_raw()).cloned()
    }

    fn remove_tracked(&self, handle: &ProcessHandle) {
        let mut guard = self.children.lock().unwrap_or_else(|e| e.into_inner());
        guard.remove(&handle.start_time.as_raw());
    }
}

/// 整棵树（进程组 + 脱组的子孙）是否都已消失。
fn tree_gone(pgid: u32, strays: &[u32]) -> bool {
    !group_alive(pgid) && strays.iter().all(|pid| !pid_alive(*pid))
}

/// 对整棵树发同一个信号。
fn signal_tree(pgid: u32, strays: &[u32], signal: libc::c_int) {
    signal_group(pgid, signal);
    for pid in strays {
        signal_pid(*pid, signal);
    }
}

/// 在 `timeout` 内等整棵树消失。
async fn wait_tree_gone(pgid: u32, strays: &[u32], timeout: Duration) -> bool {
    let deadline = tokio::time::Instant::now() + timeout;
    loop {
        if tree_gone(pgid, strays) {
            return true;
        }
        if tokio::time::Instant::now() >= deadline {
            return false;
        }
        tokio::time::sleep(KILL_POLL).await;
    }
}

impl ProcessBackend for UnixProcessBackend {
    fn spawn<'a>(&'a self, request: &'a SpawnRequest) -> BackendFuture<'a, Result<SpawnedProcess>> {
        Box::pin(async move {
            let mut command = Command::new(&request.program);
            command.args(&request.args);
            command.stdout(std::process::Stdio::piped());
            command.stderr(std::process::Stdio::piped());
            // 不喂 stdin：交互式程序读 stdin 会立刻拿到 EOF，而不是挂住
            command.stdin(std::process::Stdio::null());
            // 新进程组：pgid == pid，杀树时一次 killpg 覆盖整组
            command.process_group(0);
            if let Some(cwd) = &request.cwd {
                command.current_dir(cwd);
            }
            // PATH 必须在 envs 之前设置：配置里显式写的 PATH 要能覆盖它。
            // std 在 Unix 上用"子进程的 PATH"查找无斜杠的程序名，所以这里同时修好了 `pnpm` 找不到的问题。
            if !request.env.contains_key("PATH") {
                let shell_path = tokio::task::spawn_blocking(login_shell_path)
                    .await
                    .ok()
                    .flatten();
                if let Some(shell_path) = shell_path {
                    let inherited = std::env::var("PATH").unwrap_or_default();
                    command.env("PATH", merge_path(shell_path, &inherited));
                }
            }
            command.envs(&request.env);

            let mut child = command.spawn().map_err(|e| AppCatchError::Io {
                path: request
                    .cwd
                    .clone()
                    .unwrap_or_else(|| std::path::PathBuf::from(&request.program)),
                source: e,
            })?;

            let Some(pid) = child.id() else {
                let _ = child.kill().await;
                return Err(AppCatchError::Unsupported(
                    "子进程启动后立即退出，拿不到 pid".to_string(),
                ));
            };

            // 子进程尚未被回收，即使已退出也还是僵尸、身份可读；
            // 极少数平台读不到僵尸信息时用当前时间兜底——该进程已结束，身份只用于本次会话内部。
            let start_time = read_identity(pid).map_or_else(now_micros, |identity| identity.start);

            let stdout = child
                .stdout
                .take()
                .map(|s| Box::new(s) as Box<dyn tokio::io::AsyncRead + Send + Unpin>);
            let stderr = child
                .stderr
                .take()
                .map(|s| Box::new(s) as Box<dyn tokio::io::AsyncRead + Send + Unpin>);

            {
                let mut guard = self.children.lock().unwrap_or_else(|e| e.into_inner());
                guard.insert(start_time, Arc::new(AsyncMutex::new(child)));
            }

            Ok(SpawnedProcess {
                handle: ProcessHandle::new(pid, ProcessStartTime::from_raw(start_time)),
                stdout,
                stderr,
            })
        })
    }

    fn kill_tree<'a>(&'a self, handle: &'a ProcessHandle) -> BackendFuture<'a, Result<()>> {
        Box::pin(async move {
            let owned = self.peek_child(handle).is_some();
            // 不是本进程 spawn 的：先校验身份，pid 已被复用时绝不误杀
            if !owned && !is_alive_matching(handle) {
                return Ok(());
            }

            let pgid = handle.pid;
            // 必须在发信号前拍快照：父进程一死，脱组的子孙会被重新挂到 launchd/init 下
            let strays: Vec<u32> = snapshot_descendants(handle.pid).await;

            signal_tree(pgid, &strays, libc::SIGTERM);
            if wait_tree_gone(pgid, &strays, TERM_GRACE).await {
                return Ok(());
            }

            signal_tree(pgid, &strays, libc::SIGKILL);
            if wait_tree_gone(pgid, &strays, KILL_GRACE).await {
                return Ok(());
            }

            let survivors: Vec<u32> = strays
                .iter()
                .copied()
                .filter(|pid| pid_alive(*pid))
                .collect();
            Err(AppCatchError::Unsupported(format!(
                "终止进程树后仍有进程存活: pgid={pgid} 组内存活={} 子孙={survivors:?}",
                group_alive(pgid)
            )))
        })
    }

    fn attach<'a>(
        &'a self,
        pid: u32,
        start_time: ProcessStartTime,
    ) -> BackendFuture<'a, Result<Option<ProcessHandle>>> {
        Box::pin(async move {
            let handle = ProcessHandle::new(pid, start_time);
            Ok(is_alive_matching(&handle).then_some(handle))
        })
    }

    fn is_running<'a>(&'a self, handle: &'a ProcessHandle) -> BackendFuture<'a, Result<bool>> {
        Box::pin(async move { Ok(is_alive_matching(handle)) })
    }

    fn wait<'a>(&'a self, handle: &'a ProcessHandle) -> BackendFuture<'a, Result<ExitStatusInfo>> {
        Box::pin(async move {
            if let Some(child) = self.peek_child(handle) {
                let status = {
                    let mut guard = child.lock().await;
                    guard
                        .wait()
                        .await
                        .map_err(|e| AppCatchError::Unsupported(format!("等待子进程失败: {e}")))?
                };
                self.remove_tracked(handle);
                // 被信号终止时 code() 为 None，与 Windows 侧"被杀"的语义由上层按 stop 标记区分
                return Ok(ExitStatusInfo {
                    code: status.code(),
                });
            }

            // 重关联的进程不是我们的子进程：waitpid 不可用，只能轮询到它消失
            while is_alive_matching(handle) {
                tokio::time::sleep(ATTACHED_POLL).await;
            }
            Ok(ExitStatusInfo { code: None })
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn proc_stat_parses_start_time_even_with_parens_in_comm() {
        let stat = "4242 (weird ) name) S 1 4242 4242 0 -1 4194560 100 0 0 0 1 2 0 0 20 0 3 0 987654 123 456";
        let identity = parse_proc_stat(stat).expect("应能解析");
        assert_eq!(identity.start, 987_654);
        assert!(!identity.zombie);

        let zombie = "7 (sh) Z 1 7 7 0 -1 0 0 0 0 0 0 0 0 0 20 0 1 0 55 0 0";
        assert!(parse_proc_stat(zombie).expect("应能解析").zombie);
        assert_eq!(parse_proc_stat("garbage"), None);
    }

    #[test]
    fn descendants_follow_parent_links_and_skip_unrelated() {
        let pairs = parse_pid_ppid(
            "  1     0\n 100   1\n 101 100\n 102 101\n 200   1\n 103 100\nbad line\n",
        );
        let mut got = descendants_from(&pairs, 100);
        got.sort_unstable();
        assert_eq!(got, vec![101, 102, 103]);
        assert!(descendants_from(&pairs, 999).is_empty());
    }

    #[test]
    fn descendants_tolerate_cycles() {
        // ps 快照不是原子的：pid 复用可能造成环，BFS 不能死循环
        let pairs = vec![(2, 1), (1, 2), (3, 2)];
        let mut got = descendants_from(&pairs, 1);
        got.sort_unstable();
        assert_eq!(got, vec![2, 3]);
    }

    #[test]
    fn shell_path_is_extracted_between_markers() {
        let noisy = format!("Welcome!\n{PATH_BEGIN}/opt/homebrew/bin:/usr/bin{PATH_END}");
        assert_eq!(
            extract_marked_path(&noisy).as_deref(),
            Some("/opt/homebrew/bin:/usr/bin")
        );
        assert_eq!(extract_marked_path("no markers"), None);
        assert_eq!(
            extract_marked_path(&format!("{PATH_BEGIN}{PATH_END}")),
            None
        );
    }

    #[test]
    fn merged_path_prefers_shell_entries_and_dedupes() {
        assert_eq!(
            merge_path("/opt/homebrew/bin:/usr/bin", "/usr/bin:/bin::/usr/sbin"),
            "/opt/homebrew/bin:/usr/bin:/bin:/usr/sbin"
        );
    }

    #[test]
    fn own_process_identity_is_readable_and_alive() {
        let pid = std::process::id();
        let identity = read_identity(pid).expect("应能读取自身身份");
        assert!(!identity.zombie);
        let handle = ProcessHandle::new(pid, ProcessStartTime::from_raw(identity.start));
        assert!(is_alive_matching(&handle));
        // 启动时间不符 = 视为 pid 被复用
        let wrong = ProcessHandle::new(pid, ProcessStartTime::from_raw(identity.start + 1));
        assert!(!is_alive_matching(&wrong));
    }

    #[tokio::test]
    async fn spawn_captures_identity_and_wait_reports_exit_code() {
        let backend = UnixProcessBackend::new();
        let mut request = SpawnRequest::new(
            crate::session::SessionId::new_v4(),
            crate::config::AppId::new("unix-test").expect("合法 id"),
            "sh",
        );
        request.args = vec!["-c".into(), "exit 3".into()];
        let spawned = backend.spawn(&request).await.expect("spawn 失败");
        let exit = backend.wait(&spawned.handle).await.expect("wait 失败");
        assert_eq!(exit.code, Some(3));
        assert!(!backend.is_running(&spawned.handle).await.expect("查询失败"));
    }

    #[tokio::test]
    async fn kill_tree_reaps_group_and_detached_grandchild() {
        let backend = UnixProcessBackend::new();
        let mut request = SpawnRequest::new(
            crate::session::SessionId::new_v4(),
            crate::config::AppId::new("unix-tree").expect("合法 id"),
            "sh",
        );
        // 组内孙进程 + 一个 setsid 脱组的孙进程（模拟 detached 子进程），父进程忽略 SIGTERM
        request.args = vec![
            "-c".into(),
            "trap '' TERM; sleep 300 & (command -v setsid >/dev/null && setsid sleep 300 || perl -e 'setpgrp(0,0); exec \"sleep\", 300') & wait".into(),
        ];
        let spawned = backend.spawn(&request).await.expect("spawn 失败");
        tokio::time::sleep(Duration::from_millis(300)).await;
        let before = snapshot_descendants(spawned.handle.pid).await;
        assert!(before.len() >= 2, "应至少有两个子孙进程: {before:?}");

        let waiter = {
            let handle = spawned.handle;
            let backend = &backend;
            async move { backend.wait(&handle).await }
        };
        let (killed, exit) = tokio::join!(backend.kill_tree(&spawned.handle), waiter);
        killed.expect("杀树失败");
        exit.expect("wait 失败");

        assert!(!group_alive(spawned.handle.pid), "进程组应已清空");
        for pid in before {
            assert!(!pid_alive(pid), "子孙 {pid} 应已被回收");
        }
    }

    #[tokio::test]
    async fn kill_tree_ignores_reused_pid() {
        let backend = UnixProcessBackend::new();
        // 自身 pid + 错误的启动时间：模拟 pid 已被别的进程复用，绝不能发信号
        let handle = ProcessHandle::new(std::process::id(), ProcessStartTime::from_raw(1));
        backend.kill_tree(&handle).await.expect("应直接返回 Ok");
        assert!(
            read_identity(std::process::id()).is_some(),
            "测试进程自己必须还活着"
        );
    }
}
