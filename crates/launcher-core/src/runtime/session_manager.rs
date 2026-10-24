//! 会话运行时门面：启动、停止、重启、状态查询、重启后重关联。
//!
//! 这是 M2（Tauri command 层）唯一需要对接的入口，全部 API 都是 async。
//!
//! 一次会话的完整生命周期：
//!
//! ```text
//! start() --> Running --(自行退出, code==0)----> Exited  (reason=normal)
//!                   --(自行退出, code!=0/取不到)--> Crashed (reason=crashed)
//!                   --(调用 stop())-------------> Exited  (reason=killed)
//! ```
//!
//! 退出分类（需求基线 F4）：
//! - 本进程主动调用 stop/kill -> ExitReason::Killed；
//! - 未主动停止且退出码为 0 -> ExitReason::Normal；
//! - 未主动停止且退出码非 0（含被外部杀死） -> ExitReason::Crashed。

use std::collections::{HashMap, VecDeque};
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::{Duration, Instant};

use tokio::sync::Mutex;
use tokio::task::JoinHandle;

use crate::config::{AppConfig, AppId};
use crate::error::{AppCatchError, Result};
use crate::paths;
use crate::platform::{ProcessBackend, ProcessHandle, ProcessStartTime, SpawnRequest};
use crate::runtime::capture::{CaptureConfig, CapturePipeline, CaptureSink};
use crate::runtime::log_writer::LogWriter;
use crate::runtime::status::{default_status_probe, ProbeContext, StatusProbe, StatusSnapshot};
use crate::session::{now_rfc3339, ExitReason, RunState, SessionEvent, SessionId};

/// 状态事件限流窗口：一分钟。
const STATUS_RATE_WINDOW: Duration = Duration::from_secs(60);

/// 收尾时等待状态采集循环结束的预算。
///
/// 循环下一轮开头会重新检查会话状态，因此在写入 exit 之前先把它等到结束；
/// 这个上限只兜住「探针卡在系统调用里」的极端情况：超时即 abort，绝不泄漏 task。
const DURATION_STATUS_SHUTDOWN_BUDGET: Duration = Duration::from_secs(5);

/// 会话的退出信息。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ExitInfo {
    /// 退出码；被信号杀死或取不到时为 None。
    pub code: Option<i32>,
    /// 退出原因。
    pub reason: ExitReason,
}

/// 会话快照：给 GUI 展示与状态查询用的只读视图。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SessionSnapshot {
    /// 会话 id（= 会话目录名）。
    pub session_id: SessionId,
    /// 关联的应用 id。
    pub app_id: AppId,
    /// 子进程 pid。
    pub pid: u32,
    /// 当前状态。
    pub state: RunState,
    /// 会话开始时间（RFC3339）。
    pub started_at: String,
    /// 已确定的退出信息；仍在运行时为 None。
    pub last_exit: Option<ExitInfo>,
    /// 最近一次成功采集到的状态（窗口标题 / 监听端口）。
    ///
    /// -`None`：状态采集被禁用（`CaptureConfig::status_interval == None`），
    ///   或尚未完成第一次采集；
    /// - `Some`：已采集过至少一次。字段本身仍可能是空值——那表示「尽力而为但没采到」，
    ///   例如无窗口的 Python 服务：`window_title` 为 `None`、`ports` 照常填。
    ///
    /// 与日志的关系：`status` 事件只在**状态变化**时写，受 `max_status_events` 限流；
    /// 但这里是**实时最新值**，不受限流影响。M2 直接读它展示即可。
    pub latest_status: Option<StatusSnapshot>,
}

/// 一次会话的句柄：标识 + 进程身份 + 启动时快照。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SessionHandle {
    /// 会话 id。
    pub session_id: SessionId,
    /// 进程身份（pid + 启动时间）。
    pub handle: ProcessHandle,
    /// 启动时生成的快照（state 恒为 Running）。
    pub snapshot: SessionSnapshot,
}

/// 会话的共享状态；start / stop / 守护任务 / 状态循环四处都会写它。
#[derive(Debug, Clone)]
struct SessionState {
    app_id: AppId,
    pid: u32,
    started_at: String,
    state: RunState,
    last_exit: Option<ExitInfo>,
    /// 最近一次采集到的状态；未采集过为 None。
    latest_status: Option<StatusSnapshot>,
}

/// 运行中会话的完整记录（进程身份 + 状态 + 主动停止标记 + 后台任务句柄）。
struct ActiveSession {
    handle: ProcessHandle,
    /// 是否收到过主动停止请求；决定 Exit 事件的 reason 是 killed 还是 normal/crashed。
    stop_requested: bool,
    state: SessionState,
    /// 状态采集循环的句柄；`CaptureConfig::status_interval` 为 None 时为 None。
    ///
    /// 由本记录持有并在会话收尾时 await，保证结束后没有残留 task。
    status_task: Option<JoinHandle<()>>,
}

/// status 事件限流器：滑动 60 秒窗口内已写条数。
///
/// 只保存窗口内的写入时刻（`Instant`）；条数上限远小于窗口秒数，
/// 因此队列的实际长度是个位数，开销可忽略。
struct StatusRateLimiter {
    /// 每分钟上限；None 表示不限制。
    limit: Option<u32>,
    /// 窗口内已写事件的时刻。
    recent: VecDeque<Instant>,
}

impl StatusRateLimiter {
    /// 用配置构造限流器。
    fn new(limit: Option<u32>) -> Self {
        Self {
            limit,
            recent: VecDeque::new(),
        }
    }

    /// 判断现在能否再写一条；能写则同时记账并返回 true。
    fn try_acquire(&mut self) -> bool {
        let now = Instant::now();
        // 先淘汰已经滑出窗口的记录
        while let Some(front) = self.recent.front() {
            if now.duration_since(*front) >= STATUS_RATE_WINDOW {
                self.recent.pop_front();
            } else {
                break;
            }
        }

        match self.limit {
            // 不限流：直接放行，不记账（省一次 push）
            None => true,
            Some(limit) => {
                if (self.recent.len() as u32) >= limit {
                    // 达到上限：本窗口内不再写日志，但调用方仍会更新快照
                    false
                } else {
                    self.recent.push_back(now);
                    true
                }
            }
        }
    }
}

/// 会话管理器：M2 的唯一入口。
///
/// 内部持有平台后端（trait 对象，便于换实现）与运行中会话表。
pub struct SessionManager {
    backend: Arc<dyn ProcessBackend>,
    capture: CaptureConfig,
    sessions_root: PathBuf,
    /// session_id -> 会话记录
    active: Arc<Mutex<HashMap<SessionId, ActiveSession>>>,
    /// 状态采集探针（可替换，测试用假 probe 驱动）。
    status_probe: Arc<dyn StatusProbe>,
}

impl std::fmt::Debug for SessionManager {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("SessionManager")
            .field("sessions_root", &self.sessions_root)
            .finish_non_exhaustive()
    }
}

impl SessionManager {
    /// 用默认布局（APP_CATCH_HOME 优先）构造。
    pub fn from_home(backend: Arc<dyn ProcessBackend>, capture: CaptureConfig) -> Result<Self> {
        Ok(Self::new(backend, capture, paths::sessions_dir()?))
    }

    /// 用显式会话根目录构造（测试用）。
    pub fn new(
        backend: Arc<dyn ProcessBackend>,
        capture: CaptureConfig,
        sessions_root: impl Into<PathBuf>,
    ) -> Self {
        Self {
            backend,
            capture,
            sessions_root: sessions_root.into(),
            active: Arc::new(Mutex::new(HashMap::new())),
            status_probe: default_status_probe(),
        }
    }

    /// 替换状态采集探针（测试注入假 probe；M2 想加状态项也可整体替换）。
    ///
    /// 典型用法见 `tests/status_probe.rs`：注入固定值 probe 验证「变化才写事件」。
    #[must_use]
    pub fn with_status_probe(mut self, probe: Arc<dyn StatusProbe>) -> Self {
        self.status_probe = probe;
        self
    }

    /// 会话根目录。
    pub fn sessions_root(&self) -> &Path {
        &self.sessions_root
    }

    /// 单个会话目录（不创建）。
    pub fn session_dir(&self, session_id: &SessionId) -> PathBuf {
        self.sessions_root.join(session_id.to_string())
    }

    /// 启动一个应用，返回会话句柄。
    ///
    /// 步骤：生成 session_id -> 建会话目录 -> 写 start 事件 -> spawn -> 拉起捕获管线。
    /// 管线与退出收尾在后台任务中完成，调用方无需再管理生命周期。
    pub async fn start(&self, app: &AppConfig) -> Result<SessionHandle> {
        let session_id = SessionId::new_v4();
        let dir = self.session_dir(&session_id);
        tokio::fs::create_dir_all(&dir)
            .await
            .map_err(|e| AppCatchError::io(&dir, e))?;

        // 先开日志再 spawn：start 事件一定是首行，且 spawn 失败也能留下痕迹。
        // 这里的 writer 是**本会话唯一的** LogWriter，spawn 成功后立刻移交给捕获管线，
        // 之后的 status / exit 事件都通过管线的 sink 排队，绝不再新开第二个。
        let mut writer = LogWriter::open(&dir, self.capture.writer.clone()).await?;

        let mut request = SpawnRequest::new(session_id, app.id.clone(), app.command.clone());
        request.args = app.args.clone();
        request.env = app.env.clone();
        if !app.cwd.as_os_str().is_empty() {
            request.cwd = Some(app.cwd.clone());
        }

        let spawned = match self.backend.spawn(&request).await {
            Ok(spawned) => spawned,
            Err(e) => {
                // 启动失败也要留下可读痕迹，便于 M2 直接展示原因。
                // 这条路径上捕获管线还没建立，writer 仍是唯一持有者，写完 exit 即关闭。
                let _ = writer
                    .write_event(&SessionEvent::exit(None, ExitReason::Crashed))
                    .await;
                let _ = writer.close().await;
                return Err(e);
            }
        };

        let started_at = now_rfc3339();
        let cwd = request
            .cwd
            .as_ref()
            .map(|p| p.to_string_lossy().into_owned())
            .unwrap_or_else(current_dir_string);
        let start_event = SessionEvent::start(
            session_id,
            app.id.to_string(),
            render_command(&request.argv()),
            cwd,
            request.argv(),
            spawned.handle.pid,
            std::process::id(),
            spawned.handle.start_time.as_raw(),
        );
        writer.write_event(&start_event).await?;
        writer.flush().await?;

        let pipeline = CapturePipeline::start(
            spawned.stdout,
            spawned.stderr,
            writer,
            self.capture.clone(),
        );

        // 写盘权柄：状态循环与退出收尾共用它，谁都不再自己开 LogWriter
        let sink = pipeline.sink();

        // 状态采集循环：在 start 返回前就挂上
        // （句柄先存进 ActiveSession，收尾任务会 await 它，保证不泄漏 task）
        let status_task = self.spawn_status_loop(session_id, spawned.handle.pid, sink.clone());

        {
            let mut guard = self.active.lock().await;
            guard.insert(
                session_id,
                ActiveSession {
                    handle: spawned.handle,
                    stop_requested: false,
                    state: SessionState {
                        app_id: app.id.clone(),
                        pid: spawned.handle.pid,
                        started_at: started_at.clone(),
                        state: RunState::Running,
                        last_exit: None,
                        latest_status: None,
                    },
                    status_task,
                },
            );
        }

        // 守护任务：等进程退出 -> 等状态循环收尾 -> 收尾管线（写 exit 并关闭） -> 回写状态
        //
        // 顺序很关键：exit 事件必须落在日志**末尾**，因此状态循环必须先结束，
        // 否则它可能在写完 exit 之后再排入一条 status。
        let backend = Arc::clone(&self.backend);
        let active = Arc::clone(&self.active);
        let handle = spawned.handle;
        tokio::spawn(async move {
            // 先等进程结束（这一步决定退出码）
            let exit = backend.wait(&handle).await;
            let code = match &exit {
                Ok(info) => info.code,
                Err(_) => None,
            };

            // 取出状态循环句柄，顺带读出「是否主动停止」用于归类
            let (status_task, stop_requested) = {
                let mut guard = active.lock().await;
                match guard.get_mut(&session_id) {
                    Some(session) => (session.status_task.take(), session.stop_requested),
                    None => (None, false),
                }
            };

            if let Some(task) = status_task {
                // 正常路径毫秒级返回；预算只兜「探针卡在系统调用里」的极端情况
                let mut task = task;
                if tokio::time::timeout(DURATION_STATUS_SHUTDOWN_BUDGET, &mut task)
                    .await
                    .is_err()
                {
                    task.abort();
                    let _ = task.await;
                }
            }

            let reason = classify_exit(stop_requested, code);

            // 收尾管线：读完剩余输出 -> overflow 补记 -> 写 exit -> flush+close。
            // 这是会话日志唯一的收尾入口，exit 必然落在末尾。
            let _ = pipeline.close_with(Some(SessionEvent::exit(code, reason))).await;

            let mut guard = active.lock().await;
            if let Some(session) = guard.get_mut(&session_id) {
                session.state.state = state_for_reason(reason);
                session.state.last_exit = Some(ExitInfo { code, reason });
            }
        });

        Ok(SessionHandle {
            session_id,
            handle: spawned.handle,
            snapshot: SessionSnapshot {
                session_id,
                app_id: app.id.clone(),
                pid: spawned.handle.pid,
                state: RunState::Running,
                started_at,
                last_exit: None,
                // start 刚返回，第一次采样还没跑：留空而不是伪造一个空快照
                latest_status: None,
            },
        })
    }

    /// 启动会话的状态采集循环，返回任务句柄。
    ///
    /// 循环行为（对应 docs/RUNTIME.md「状态采集」）：
    ///
    /// 1. 每 `status_interval` 采样一次；采样本身在 `spawn_blocking` 里跑，
    ///    不占用 tokio worker；
    /// 2. 与上一次采样结果比较，**只有变化时才追加 `status` 事件**（避免日志刷屏）；
    /// 3. 无论是否写事件，都把最新值写进 `SessionState::latest_status` 供 M2 直接读；
    /// 4. 写事件前先过 `max_status_events` 限流（滑动 60 秒窗口）；
    /// 5. 事件一律经 `sink` 排队，绝不自己开 `LogWriter`（单 writer 所有权）；
    /// 6. 每轮开头检查进程是否还活着，已退出就结束循环。
    ///
    /// `status_interval` 为 `None` 或零时直接返回 `None`（不启动任务）。
    fn spawn_status_loop(
        &self,
        session_id: SessionId,
        root_pid: u32,
        sink: CaptureSink,
    ) -> Option<JoinHandle<()>> {
        let interval = self.capture.status_interval?;
        if interval.is_zero() {
            return None;
        }

        let probe = Arc::clone(&self.status_probe);
        // 组合 probe 里只要有一个需要进程树，就枚举一次；都不需要则整条路径省掉
        let needs_tree = probe.needs_process_tree();
        let active = Arc::clone(&self.active);
        let mut limiter = StatusRateLimiter::new(self.capture.max_status_events);

        Some(tokio::spawn(async move {
            // 上一轮的状态；None 表示还没采过（第一轮一定写事件，给日志一个起点）
            let mut last: Option<StatusSnapshot> = None;
            // 会话 id 只在 ProbeContext 里用到，转成字符串一次即可
            let session_id_text = session_id.to_string();

            loop {
                // 进程已退出（含被 kill）就收尾：先退一轮再查，让最后一刻的状态有机会落盘
                let running = {
                    let guard = active.lock().await;
                    guard
                        .get(&session_id)
                        .map(|s| s.state.state == RunState::Running)
                        .unwrap_or(false)
                };
                if !running {
                    return;
                }

                // 探针是同步 Win32 调用：扔到阻塞线程池，避免占住 tokio worker。
                // 只有确实需要进程树的 probe 才枚举整机进程表——
                // ToolHelp 快照是这条路径上最贵的一步（开发机实测约 250ms）。
                let ctx = {
                    let descendants = if needs_tree {
                        descendant_pids_blocking(root_pid).await
                    } else {
                        Vec::new()
                    };
                    ProbeContext::with_session(root_pid, session_id_text.clone())
                        .with_descendants(descendants)
                };
                let probe_for_call = Arc::clone(&probe);
                let sampled = tokio::task::spawn_blocking(move || probe_for_call.probe(&ctx))
                    .await
                    // 阻塞任务本身崩了（panic / 取消）：按「这一轮没采到」处理，循环继续
                    .unwrap_or_else(|_| StatusSnapshot::empty());

                let changed = last.as_ref() != Some(&sampled);
                last = Some(sampled.clone());

                // 快照始终更新为最新值（限流只影响日志，不影响 GUI 读到的状态）
                {
                    let mut guard = active.lock().await;
                    if let Some(session) = guard.get_mut(&session_id) {
                        session.state.latest_status = Some(sampled.clone());
                    }
                }

                // 只有变化时才写事件；再叠加限流保护。
                // 走 sink 而不是新开 writer：分片序号与保留策略由那个唯一的 writer 统一维护。
                if changed && limiter.try_acquire() {
                    let event = SessionEvent::status(
                        RunState::Running,
                        sampled.window_title.clone(),
                        sampled.ports_field(),
                    );
                    // 背压发送：状态事件数量极少，丢掉才是真的丢信息；
                    // 返回 false 说明写盘任务已收尾（会话正在退出），循环随之结束。
                    if !sink.write(event).await {
                        return;
                    }
                }

                // 等到下一轮：循环下一轮开头会重新检查会话状态
                tokio::time::sleep(interval).await;
            }
        }))
    }

    /// 主动停止一个会话：回收整棵进程树，并把退出原因标记为 killed。
    ///
    /// 返回时进程树已被回收（kill_tree 内部会等到进程真正消失）。
    pub async fn stop(&self, session_id: SessionId) -> Result<()> {
        let handle = {
            let mut guard = self.active.lock().await;
            let session = guard.get_mut(&session_id).ok_or_else(|| {
                AppCatchError::NotFound(format!("运行中的会话 {session_id}"))
            })?;
            // 先置标记再杀：守护任务的判定一定能看到「这是主动停止」
            session.stop_requested = true;
            session.handle
        };

        self.backend.kill_tree(&handle).await?;
        Ok(())
    }

    /// 重启：停止旧会话并以新 session_id 重新启动。
    pub async fn restart(&self, session_id: SessionId, app: &AppConfig) -> Result<SessionHandle> {
        // 会话可能已自行退出，NotFound 不应阻断重启
        match self.stop(session_id).await {
            Ok(()) | Err(AppCatchError::NotFound(_)) => {}
            Err(e) => return Err(e),
        }
        self.start(app).await
    }

    /// 列出仍在运行的会话快照（按 session_id 稳定排序）。
    pub async fn list_active(&self) -> Vec<SessionSnapshot> {
        let guard = self.active.lock().await;
        let mut list: Vec<SessionSnapshot> = guard
            .iter()
            .filter(|(_, s)| s.state.state == RunState::Running)
            .map(|(id, s)| snapshot_of(*id, s))
            .collect();
        list.sort_by_key(|s| s.session_id);
        list
    }

    /// 查询单个会话状态；未知会话返回 NotFound。
    pub async fn status(&self, session_id: SessionId) -> Result<SessionSnapshot> {
        let guard = self.active.lock().await;
        let session = guard
            .get(&session_id)
            .ok_or_else(|| AppCatchError::NotFound(format!("会话 {session_id}")))?;
        Ok(snapshot_of(session_id, session))
    }

    /// 等待会话退出并返回退出信息（测试与脚本用），超时返回错误。
    pub async fn wait_for_exit(
        &self,
        session_id: SessionId,
        timeout: std::time::Duration,
    ) -> Result<ExitInfo> {
        let deadline = tokio::time::Instant::now() + timeout;
        loop {
            {
                let guard = self.active.lock().await;
                match guard.get(&session_id) {
                    Some(session) => {
                        if let Some(exit) = session.state.last_exit {
                            return Ok(exit);
                        }
                    }
                    None => {
                        return Err(AppCatchError::NotFound(format!("会话 {session_id}")));
                    }
                }
            }
            if tokio::time::Instant::now() >= deadline {
                return Err(AppCatchError::Unsupported(format!(
                    "等待会话 {session_id} 退出超时"
                )));
            }
            tokio::time::sleep(std::time::Duration::from_millis(5)).await;
        }
    }

    /// 扫描历史会话目录，找出「有 start 无 exit」的会话并重新关联进程。
    ///
    /// 判定规则：
    /// - attach 成功（pid 存活且启动时间一致） -> 标记 Running，并追加一条 status(running) 事件；
    /// - 进程不存在或启动时间不匹配（PID 已被复用） -> 追加 exit(code=null, reason=crashed)。
    ///
    /// 已知限制：重启启动器后**无法重新接管**旧进程的 stdout/stderr（管道已随父进程消失），
    /// 详见 docs/RUNTIME.md。
    pub async fn recover_sessions(&self) -> Result<Vec<SessionSnapshot>> {
        tokio::fs::create_dir_all(&self.sessions_root)
            .await
            .map_err(|e| AppCatchError::io(&self.sessions_root, e))?;

        let mut entries = tokio::fs::read_dir(&self.sessions_root)
            .await
            .map_err(|e| AppCatchError::io(&self.sessions_root, e))?;

        let mut recovered = Vec::new();
        while let Some(entry) = entries
            .next_entry()
            .await
            .map_err(|e| AppCatchError::io(&self.sessions_root, e))?
        {
            let path = entry.path();
            if !path.is_dir() {
                continue;
            }
            let Some(name) = path.file_name().and_then(|n| n.to_str()) else {
                continue;
            };
            let Ok(session_id) = name.parse::<SessionId>() else {
                // 目录名不是 uuid：不是会话目录，跳过
                continue;
            };
            if !path.join(paths::SESSION_LOG_FILE).is_file() {
                continue;
            }

            if let Some(snapshot) = self.recover_one(session_id, &path).await? {
                recovered.push(snapshot);
            }
        }

        recovered.sort_by(|a, b| a.started_at.cmp(&b.started_at));
        Ok(recovered)
    }

    /// 处理单个会话目录；Some 表示这是一个需要接管或收尾的未完成会话。
    async fn recover_one(
        &self,
        session_id: SessionId,
        dir: &Path,
    ) -> Result<Option<SessionSnapshot>> {
        let log_path = dir.join(paths::SESSION_LOG_FILE);
        // 已轮转走的会话：主文件可能为空，此时回落到最后一个分片读取 start
        let mut text = tokio::fs::read_to_string(&log_path)
            .await
            .map_err(|e| AppCatchError::io(&log_path, e))?;
        if !text.contains("\"event\":\"start\"") {
            if let Some(shard_text) = read_last_shard_with_start(dir).await {
                text.push_str(&shard_text);
            }
        }

        let mut start_event: Option<SessionEvent> = None;
        let mut has_exit = false;
        for line in text.lines() {
            if line.trim().is_empty() {
                continue;
            }
            // 解析失败的行（启动器被强杀留下的半行）跳过，不影响其余事件
            let Ok(event) = SessionEvent::from_jsonl_line(line) else {
                continue;
            };
            match &event {
                SessionEvent::Start { .. } => start_event = Some(event),
                SessionEvent::Exit { .. } => has_exit = true,
                _ => {}
            }
        }

        // 没有 start：不是完整会话；已有 exit：已正常收尾。两种情况都不需要接管
        let Some(start) = start_event else {
            return Ok(None);
        };
        if has_exit {
            return Ok(None);
        }

        let (pid, start_time, app_id_raw) = match &start {
            SessionEvent::Start {
                pid,
                start_time,
                app_id,
                ..
            } => (*pid, ProcessStartTime::from_raw(*start_time), app_id.clone()),
            _ => return Ok(None),
        };
        let started_at = start.timestamp().to_string();
        // 配置里已删除的应用仍然接管进程，只是应用 id 用占位符
        let app_id = AppId::new(app_id_raw)
            .or_else(|_| AppId::new("unknown"))
            .unwrap_or_else(|_| unreachable!("unknown 是合法的 AppId"));

        // 旧事件没记 start_time（值为 0）时无法判定身份，按已消失处理
        let attached = if start_time.as_raw() == 0 {
            None
        } else {
            self.backend.attach(pid, start_time).await?
        };

        let Some(handle) = attached else {
            // 进程不存在或 pid 已被复用：补 exit，把会话收尾。
            // 这条路径不开捕获管线（没有进程可采），单独开一个短命 writer 写完后立刻关闭；
            // 随后会话也不会再进入 active 表，不存在第二个 writer 与它共存。
            let mut writer = LogWriter::open(dir, self.capture.writer.clone()).await?;
            writer
                .write_event(&SessionEvent::exit(None, ExitReason::Crashed))
                .await?;
            writer.close().await?;
            let info = ExitInfo {
                code: None,
                reason: ExitReason::Crashed,
            };
            let mut guard = self.active.lock().await;
            guard.insert(
                session_id,
                ActiveSession {
                    handle: ProcessHandle::new(pid, start_time),
                    stop_requested: false,
                    state: SessionState {
                        app_id: app_id.clone(),
                        pid,
                        started_at: started_at.clone(),
                        state: RunState::Crashed,
                        last_exit: Some(info),
                        latest_status: None,
                    },
                    // 已崩溃的会话没有进程可采，不启动状态循环。
                    // sink 用一个通道已关闭的占位：它永远不会被写，也写不进去。
                    status_task: None,
                },
            );
            return Ok(Some(SessionSnapshot {
                session_id,
                app_id,
                pid,
                state: RunState::Crashed,
                started_at,
                last_exit: Some(info),
                latest_status: None,
            }));
        };

        // 进程还活着：接管它。
        //
        // 接管后**没有** stdout/stderr 管道（管道随上一个父进程消失），
        // 因此这里开一条两侧都没有 reader 的管线：它唯一的 writer 从此独占该会话目录，
        // 后续 status 事件、以及将来的 exit 事件都经它的 sink 排队，
        // 不会再有第二个 writer 与它争抢分片序号。
        let pipeline = CapturePipeline::start(
            None,
            None,
            LogWriter::open(dir, self.capture.writer.clone()).await?,
            self.capture.clone(),
        );
        let sink = pipeline.sink();

        // 补写 status(running) 事件，与后续事件走同一条写入路径
        let _ = sink
            .write(SessionEvent::status(RunState::Running, None, None))
            .await;

        // 接管的活进程同样需要状态采集：否则重关联后的会话会永远没有窗口标题/端口
        let status_task = self.spawn_status_loop(session_id, handle.pid, sink.clone());

        {
            let mut guard = self.active.lock().await;
            guard.insert(
                session_id,
                ActiveSession {
                    handle,
                    stop_requested: false,
                    state: SessionState {
                        app_id: app_id.clone(),
                        pid: handle.pid,
                        started_at: started_at.clone(),
                        state: RunState::Running,
                        last_exit: None,
                        latest_status: None,
                    },
                    status_task,
                },
            );
        }

        // 接管会话的守护任务：等它退出后同样补写 exit 并回写状态。
        // 关键点是**复用同一份 pipeline/sink**，绝不新开 writer。
        {
            let backend = Arc::clone(&self.backend);
            let active = Arc::clone(&self.active);
            let handle_for_wait = handle;
            tokio::spawn(async move {
                let exit = backend.wait(&handle_for_wait).await;
                let code = match &exit {
                    Ok(info) => info.code,
                    Err(_) => None,
                };

                let (status_task, stop_requested) = {
                    let mut guard = active.lock().await;
                    match guard.get_mut(&session_id) {
                        Some(session) => (session.status_task.take(), session.stop_requested),
                        None => (None, false),
                    }
                };

                if let Some(task) = status_task {
                    let mut task = task;
                    if tokio::time::timeout(DURATION_STATUS_SHUTDOWN_BUDGET, &mut task)
                        .await
                        .is_err()
                    {
                        task.abort();
                        let _ = task.await;
                    }
                }

                let reason = classify_exit(stop_requested, code);
                let _ = pipeline
                    .close_with(Some(SessionEvent::exit(code, reason)))
                    .await;

                let mut guard = active.lock().await;
                if let Some(session) = guard.get_mut(&session_id) {
                    session.state.state = state_for_reason(reason);
                    session.state.last_exit = Some(ExitInfo { code, reason });
                }
            });
        }

        Ok(Some(SessionSnapshot {
            session_id,
            app_id,
            pid: handle.pid,
            state: RunState::Running,
            started_at,
            last_exit: None,
            latest_status: None,
        }))
    }
}

/// 把内部记录投影成对外快照。
///
/// §latest_status§ 是**克隆**出来的：GUI 拿到的是采集时刻的稳定值，
/// 不会因为在别处继续刷新而出现「读了一半」的撕裂状态。
fn snapshot_of(session_id: SessionId, session: &ActiveSession) -> SessionSnapshot {
    SessionSnapshot {
        session_id,
        app_id: session.state.app_id.clone(),
        pid: session.state.pid,
        state: session.state.state,
        started_at: session.state.started_at.clone(),
        last_exit: session.state.last_exit,
        latest_status: session.state.latest_status.clone(),
    }
}

/// 在阻塞线程池里枚举子孙进程 pid。
///
/// ToolHelp 快照是同步系统调用（毫秒级），不能在 tokio worker 上直接调用；
/// 拿不到就返回空列表——状态采集是尽力而为，绝不因此让循环出错。
#[cfg(windows)]
async fn descendant_pids_blocking(root_pid: u32) -> Vec<u32> {
    tokio::task::spawn_blocking(move || crate::platform::descendant_pids(root_pid))
        .await
        .unwrap_or_default()
}

/// 非 Windows 平台没有进程树枚举能力，返回空列表。
#[cfg(not(windows))]
async fn descendant_pids_blocking(_root_pid: u32) -> Vec<u32> {
    Vec::new()
}

/// 读取目录下最后一个含 start 事件的分片（会话已轮转时用）。
async fn read_last_shard_with_start(dir: &Path) -> Option<String> {
    let mut entries = tokio::fs::read_dir(dir).await.ok()?;
    let mut shards: Vec<(u32, PathBuf)> = Vec::new();
    while let Ok(Some(entry)) = entries.next_entry().await {
        let name = entry.file_name().to_string_lossy().into_owned();
        let Some(rest) = name.strip_prefix("events.") else {
            continue;
        };
        let Some(seq_text) = rest.strip_suffix(".jsonl") else {
            continue;
        };
        let Ok(seq) = seq_text.parse::<u32>() else {
            continue;
        };
        shards.push((seq, entry.path()));
    }
    // seq 最小的分片必然包含 start（轮转从 1 开始递增）
    shards.sort_by_key(|(seq, _)| *seq);
    let (_, path) = shards.into_iter().next()?;
    tokio::fs::read_to_string(path).await.ok()
}

/// 把 argv 渲染成人类可读的命令行（仅用于 start 事件的 cmd 字段）。
pub fn render_command(argv: &[String]) -> String {
    let mut out = String::new();
    for (i, part) in argv.iter().enumerate() {
        if i > 0 {
            out.push(' ');
        }
        if part.contains(' ') || part.contains('"') {
            out.push('"');
            out.push_str(&part.replace('"', "\\\""));
            out.push('"');
        } else {
            out.push_str(part);
        }
    }
    out
}

/// 当前进程工作目录的字符串形式（取不到时返回空串）。
fn current_dir_string() -> String {
    std::env::current_dir()
        .map(|p| p.to_string_lossy().into_owned())
        .unwrap_or_default()
}

/// 依据「是否主动停止」与退出码判定退出原因。
pub fn classify_exit(stop_requested: bool, code: Option<i32>) -> ExitReason {
    if stop_requested {
        // 主动停止优先级最高：哪怕进程恰好以 0 退出，语义上也是「被我们停掉的」
        return ExitReason::Killed;
    }
    match code {
        Some(0) => ExitReason::Normal,
        // 非 0 退出码、或被外部杀死拿不到码，统一归为 crashed
        _ => ExitReason::Crashed,
    }
}

/// 退出原因对应的运行状态。
pub fn state_for_reason(reason: ExitReason) -> RunState {
    match reason {
        ExitReason::Crashed => RunState::Crashed,
        ExitReason::Normal | ExitReason::Killed => RunState::Exited,
    }
}
