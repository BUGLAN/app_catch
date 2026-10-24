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

use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::Arc;

use tokio::sync::Mutex;

use crate::config::{AppConfig, AppId};
use crate::error::{AppCatchError, Result};
use crate::paths;
use crate::platform::{ProcessBackend, ProcessHandle, ProcessStartTime, SpawnRequest};
use crate::runtime::capture::{CaptureConfig, CapturePipeline};
use crate::runtime::log_writer::LogWriter;
use crate::session::{now_rfc3339, ExitReason, RunState, SessionEvent, SessionId};

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

/// 会话的共享状态；start / stop / 守护任务三处都会写它。
#[derive(Debug, Clone)]
struct SessionState {
    app_id: AppId,
    pid: u32,
    started_at: String,
    state: RunState,
    last_exit: Option<ExitInfo>,
}

/// 运行中会话的完整记录（进程身份 + 状态 + 主动停止标记）。
struct ActiveSession {
    handle: ProcessHandle,
    /// 是否收到过主动停止请求；决定 Exit 事件的 reason 是 killed 还是 normal/crashed。
    stop_requested: bool,
    state: SessionState,
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
        }
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

        // 先开日志再 spawn：start 事件一定是首行，且 spawn 失败也能留下痕迹
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
                // 启动失败也要留下可读痕迹，便于 M2 直接展示原因
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
                    },
                },
            );
        }

        // 守护任务：等进程退出 -> 收尾捕获管线 -> 写 exit 事件 -> 回写状态
        let backend = Arc::clone(&self.backend);
        let active = Arc::clone(&self.active);
        let handle = spawned.handle;
        let dir_for_task = dir.clone();
        let writer_config = self.capture.writer.clone();
        tokio::spawn(async move {
            // 先等进程结束（这一步决定退出码），再收尾管线（把剩余输出读完写完）
            let exit = backend.wait(&handle).await;
            let code = match &exit {
                Ok(info) => info.code,
                Err(_) => None,
            };
            let _ = pipeline.finish().await;

            // 读出「是否主动停止」，据此判定退出原因
            let stop_requested = {
                let guard = active.lock().await;
                guard
                    .get(&session_id)
                    .map(|s| s.stop_requested)
                    .unwrap_or(false)
            };
            let reason = classify_exit(stop_requested, code);

            // exit 事件必须落在日志末尾：重新打开 writer 追加写
            if let Ok(mut writer) = LogWriter::open(&dir_for_task, writer_config).await {
                let _ = writer.write_event(&SessionEvent::exit(code, reason)).await;
                let _ = writer.close().await;
            }

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
            },
        })
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

        let mut writer = LogWriter::open(dir, self.capture.writer.clone()).await?;

        // 旧事件没记 start_time（值为 0）时无法判定身份，按已消失处理
        let attached = if start_time.as_raw() == 0 {
            None
        } else {
            self.backend.attach(pid, start_time).await?
        };

        let Some(handle) = attached else {
            // 进程不存在或 pid 已被复用：补 exit，把会话收尾
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
                    },
                },
            );
            return Ok(Some(SessionSnapshot {
                session_id,
                app_id,
                pid,
                state: RunState::Crashed,
                started_at,
                last_exit: Some(info),
            }));
        };

        // 进程还活着：标记 Running 并补写 status 事件
        writer
            .write_event(&SessionEvent::status(RunState::Running, None, None))
            .await?;
        writer.close().await?;

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
                    },
                },
            );
        }

        Ok(Some(SessionSnapshot {
            session_id,
            app_id,
            pid: handle.pid,
            state: RunState::Running,
            started_at,
            last_exit: None,
        }))
    }
}

/// 把内部记录投影成对外快照。
fn snapshot_of(session_id: SessionId, session: &ActiveSession) -> SessionSnapshot {
    SessionSnapshot {
        session_id,
        app_id: session.state.app_id.clone(),
        pid: session.state.pid,
        state: session.state.state,
        started_at: session.state.started_at.clone(),
        last_exit: session.state.last_exit,
    }
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
