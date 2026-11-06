//! 启动 hook：在主进程启动前 / 启动后执行用户配置的 shell 命令。
//!
//! 典型用途：启动本地开发应用前先 `git pull --ff-only`，启动后打开浏览器等。
//!
//! 执行时机（由 [`crate::runtime::SessionManager::start`] 驱动，restart 同样经过 start）：
//!
//! | 阶段 | 时机 | 失败时 |
//! | --- | --- | --- |
//! | `before` | 生成 session_id 之后、创建会话目录与 spawn 主进程**之前**，按顺序逐条执行 | 立即中止：后续 hook 不再执行、主进程不启动、不创建会话目录，`start` 返回 [`AppCatchError::HookFailed`] |
//! | `after` | 主进程 spawn 成功、`start` 事件落盘、捕获管线就绪**之后**，在后台按顺序执行，不阻塞 `start` 返回 | 只记日志：后续 after hook 不再执行，主进程不受影响 |
//!
//! 执行方式：
//! - 每条命令交给系统 shell（Unix `/bin/sh -c`，Windows `cmd /D /S /C`），因此可写管道、`&&` 等；
//! - 复用 [`ProcessBackend`] 启动：与主进程一样补齐登录 shell 的 PATH（macOS 从 Finder 启动时
//!   `git` / `pnpm` 才找得到）、放进可整体回收的进程组 / Job，超时时整棵树一起杀掉；
//! - 工作目录、环境变量与应用配置一致，另外注入 `APP_CATCH_APP_ID` / `APP_CATCH_SESSION_ID` /
//!   `APP_CATCH_HOOK_STAGE`，after 阶段额外注入主进程 `APP_CATCH_PID`；
//! - 单条命令超过 `hooks.timeout_secs`（缺省 [`DEFAULT_HOOK_TIMEOUT_SECS`]）视为失败并杀树。
//!
//! 日志：hook 输出以 `output` 事件写入会话日志，行首带 `[before]` / `[after]` 前缀。
//! before 阶段会话日志尚未创建（`start` 必须是首行），输出先在内存缓冲（只保留末尾
//! [`MAX_BUFFERED_BEFORE_LINES`] 行），主进程启动后紧跟在 `start` 事件之后补写。

use std::collections::{BTreeMap, VecDeque};
use std::path::PathBuf;
use std::sync::Arc;
use std::time::{Duration, Instant};

use tokio::io::{AsyncBufReadExt, AsyncRead, BufReader};
use tokio::sync::mpsc;

use crate::config::{AppConfig, AppId};
use crate::error::{AppCatchError, Result};
use crate::platform::{ProcessBackend, SpawnRequest};
use crate::runtime::capture::{decode_line_lossless, CaptureSink};
use crate::session::{SessionEvent, SessionId, StreamKind};

/// 单条 hook 命令的缺省超时（秒）。
pub const DEFAULT_HOOK_TIMEOUT_SECS: u64 = 600;

/// before 阶段在内存里最多缓冲的输出行数（超出部分丢最旧的，并补一行说明）。
pub const MAX_BUFFERED_BEFORE_LINES: usize = 2000;

/// 失败时错误信息里附带的输出尾部行数。
const ERROR_TAIL_LINES: usize = 8;

/// 超时杀树后，继续收集残余输出的上限时长。
const DRAIN_AFTER_KILL: Duration = Duration::from_secs(2);

/// hook 阶段。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum HookStage {
    /// 主进程启动前。
    Before,
    /// 主进程启动后。
    After,
}

impl HookStage {
    /// 小写名称，用于日志前缀与环境变量。
    pub fn as_str(self) -> &'static str {
        match self {
            HookStage::Before => "before",
            HookStage::After => "after",
        }
    }

    /// 中文名称，用于错误文案。
    fn label(self) -> &'static str {
        match self {
            HookStage::Before => "启动前",
            HookStage::After => "启动后",
        }
    }
}

/// 一次 hook 执行所需的上下文（从应用配置展开）。
#[derive(Debug, Clone)]
pub struct HookContext {
    session_id: SessionId,
    app_id: AppId,
    cwd: Option<PathBuf>,
    env: BTreeMap<String, String>,
    timeout: Duration,
}

impl HookContext {
    /// 按应用配置构造上下文。
    pub fn new(app: &AppConfig, session_id: SessionId) -> Self {
        let cwd = (!app.cwd.as_os_str().is_empty()).then(|| app.cwd.clone());
        let mut env = app.env.clone();
        env.insert("APP_CATCH_APP_ID".to_string(), app.id.to_string());
        env.insert("APP_CATCH_SESSION_ID".to_string(), session_id.to_string());
        Self {
            session_id,
            app_id: app.id.clone(),
            cwd,
            env,
            timeout: Duration::from_secs(app.hooks.effective_timeout_secs()),
        }
    }

    /// 注入主进程 pid（after 阶段用）。
    #[must_use]
    pub fn with_pid(mut self, pid: u32) -> Self {
        self.env
            .insert("APP_CATCH_PID".to_string(), pid.to_string());
        self
    }

    fn spawn_request(&self, stage: HookStage, command: &str) -> SpawnRequest {
        let (program, args) = shell_invocation(command);
        let mut request = SpawnRequest::new(self.session_id, self.app_id.clone(), program);
        request.args = args;
        request.cwd = self.cwd.clone();
        request.env = self.env.clone();
        request.env.insert(
            "APP_CATCH_HOOK_STAGE".to_string(),
            stage.as_str().to_string(),
        );
        request
    }
}

/// 把一条命令包装成系统 shell 调用。
fn shell_invocation(command: &str) -> (String, Vec<String>) {
    if cfg!(windows) {
        (
            "cmd".to_string(),
            vec!["/D".into(), "/S".into(), "/C".into(), command.to_string()],
        )
    } else {
        (
            "/bin/sh".to_string(),
            vec!["-c".into(), command.to_string()],
        )
    }
}

/// 单条 hook 的执行结果。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct HookOutcome {
    /// 原始命令。
    pub command: String,
    /// 退出码；被杀 / 取不到时为 None。
    pub code: Option<i32>,
    /// 是否因超时被杀。
    pub timed_out: bool,
    /// 启动失败原因（命令无法 spawn）。
    pub spawn_error: Option<String>,
    /// 耗时。
    pub elapsed: Duration,
    /// 输出末尾若干行（不含前缀），用于错误信息。
    pub tail: Vec<String>,
}

impl HookOutcome {
    /// 是否成功（成功启动、未超时、退出码为 0）。
    pub fn success(&self) -> bool {
        self.spawn_error.is_none() && !self.timed_out && self.code == Some(0)
    }

    /// 一句话说明结果（成功或失败原因）。
    fn summary(&self, timeout: Duration) -> String {
        let secs = self.elapsed.as_secs_f64();
        if let Some(err) = &self.spawn_error {
            format!("无法启动：{err}")
        } else if self.timed_out {
            format!("超时（{} 秒）已终止", timeout.as_secs())
        } else {
            match self.code {
                Some(0) => format!("完成（退出码 0，耗时 {secs:.1}s）"),
                Some(code) => format!("失败（退出码 {code}，耗时 {secs:.1}s）"),
                None => format!("失败（无退出码，可能被信号终止，耗时 {secs:.1}s）"),
            }
        }
    }
}

/// 执行单条 hook：输出逐行交给 `emit`（已带阶段前缀），返回执行结果。
///
/// 本函数自身不返回错误：spawn 失败、超时、非零退出都折叠进 [`HookOutcome`]，
/// 由调用方按阶段决定是中止还是只记日志。
pub async fn run_hook(
    backend: &Arc<dyn ProcessBackend>,
    stage: HookStage,
    command: &str,
    ctx: &HookContext,
    emit: &(dyn Fn(SessionEvent) + Send + Sync),
) -> HookOutcome {
    let prefix = format!("[{}] ", stage.as_str());
    let started = Instant::now();
    emit(SessionEvent::output(
        StreamKind::Stdout,
        format!("{prefix}$ {command}"),
    ));

    let mut outcome = HookOutcome {
        command: command.to_string(),
        code: None,
        timed_out: false,
        spawn_error: None,
        elapsed: Duration::ZERO,
        tail: Vec::new(),
    };

    let request = ctx.spawn_request(stage, command);
    let spawned = match backend.spawn(&request).await {
        Ok(spawned) => spawned,
        Err(e) => {
            outcome.spawn_error = Some(e.to_string());
            outcome.elapsed = started.elapsed();
            emit(SessionEvent::output(
                StreamKind::Stderr,
                format!("{prefix}{}", outcome.summary(ctx.timeout)),
            ));
            return outcome;
        }
    };

    let (tx, mut rx) = mpsc::unbounded_channel::<(SessionEvent, String)>();
    let mut pumps = Vec::new();
    if let Some(stdout) = spawned.stdout {
        pumps.push(tokio::spawn(pump(
            stdout,
            StreamKind::Stdout,
            prefix.clone(),
            tx.clone(),
        )));
    }
    if let Some(stderr) = spawned.stderr {
        pumps.push(tokio::spawn(pump(
            stderr,
            StreamKind::Stderr,
            prefix.clone(),
            tx.clone(),
        )));
    }
    drop(tx);

    let handle = spawned.handle;
    let mut tail: VecDeque<String> = VecDeque::with_capacity(ERROR_TAIL_LINES);
    let mut push_line = |event: SessionEvent, text: String| {
        if tail.len() == ERROR_TAIL_LINES {
            tail.pop_front();
        }
        tail.push_back(text);
        emit(event);
    };

    let finished = tokio::time::timeout(ctx.timeout, async {
        // 输出与退出并发等待：先读完管道（两条流都 EOF），再拿退出码
        let drain = async {
            while let Some((event, text)) = rx.recv().await {
                push_line(event, text);
            }
        };
        let (_, exit) = tokio::join!(drain, backend.wait(&handle));
        exit
    })
    .await;

    match finished {
        Ok(exit) => outcome.code = exit.ok().and_then(|info| info.code),
        Err(_) => {
            outcome.timed_out = true;
            let _ = backend.kill_tree(&handle).await;
            // 杀树后管道会关闭；限时收完残余输出，避免孙进程占着管道把这里拖住
            let _ = tokio::time::timeout(DRAIN_AFTER_KILL, async {
                while let Some((event, text)) = rx.recv().await {
                    push_line(event, text);
                }
            })
            .await;
        }
    }
    for pump in pumps {
        pump.abort();
    }

    outcome.elapsed = started.elapsed();
    outcome.tail = tail.into_iter().collect();
    let stream = if outcome.success() {
        StreamKind::Stdout
    } else {
        StreamKind::Stderr
    };
    emit(SessionEvent::output(
        stream,
        format!("{prefix}{}", outcome.summary(ctx.timeout)),
    ));
    outcome
}

/// 逐行读取一条输出流并发往 channel（与捕获管线同样的无损解码）。
async fn pump(
    reader: Box<dyn AsyncRead + Send + Unpin>,
    stream: StreamKind,
    prefix: String,
    tx: mpsc::UnboundedSender<(SessionEvent, String)>,
) {
    let mut reader = BufReader::new(reader);
    let mut buf = Vec::new();
    loop {
        buf.clear();
        match reader.read_until(b'\n', &mut buf).await {
            Ok(0) | Err(_) => break,
            Ok(_) => {
                let (text, encoding) = decode_line_lossless(&buf);
                let event =
                    SessionEvent::output_with_encoding(stream, format!("{prefix}{text}"), encoding);
                if tx.send((event, text)).is_err() {
                    break;
                }
            }
        }
    }
}

/// 执行全部 before hook；任一失败立即返回 [`AppCatchError::HookFailed`]。
///
/// 成功时返回需要补写进会话日志的输出事件（已按 [`MAX_BUFFERED_BEFORE_LINES`] 截尾）。
pub async fn run_before_hooks(
    backend: &Arc<dyn ProcessBackend>,
    app: &AppConfig,
    session_id: SessionId,
) -> Result<Vec<SessionEvent>> {
    if app.hooks.before.is_empty() {
        return Ok(Vec::new());
    }
    let ctx = HookContext::new(app, session_id);
    let buffer = std::sync::Mutex::new((VecDeque::<SessionEvent>::new(), 0u64));
    let emit = |event: SessionEvent| {
        let mut guard = buffer.lock().unwrap_or_else(|e| e.into_inner());
        let (lines, dropped) = &mut *guard;
        if lines.len() == MAX_BUFFERED_BEFORE_LINES {
            lines.pop_front();
            *dropped += 1;
        }
        lines.push_back(event);
    };

    for command in &app.hooks.before {
        let outcome = run_hook(backend, HookStage::Before, command, &ctx, &emit).await;
        if !outcome.success() {
            return Err(hook_error(HookStage::Before, &outcome, ctx.timeout));
        }
    }

    let (lines, dropped) = buffer.into_inner().unwrap_or_else(|e| e.into_inner());
    let mut events = Vec::with_capacity(lines.len() + 1);
    if dropped > 0 {
        events.push(SessionEvent::output(
            StreamKind::Stdout,
            format!("[before] （输出过长，已省略较早的 {dropped} 行）"),
        ));
    }
    events.extend(lines);
    Ok(events)
}

/// 在后台执行全部 after hook，输出经 `sink` 写进会话日志。
///
/// 某条失败后不再执行后续 after hook；主进程不受任何影响。
/// 会话已收尾（sink 关闭）时输出写入会静默失败，hook 本身仍会执行完。
pub fn spawn_after_hooks(
    backend: Arc<dyn ProcessBackend>,
    app: &AppConfig,
    session_id: SessionId,
    pid: u32,
    sink: CaptureSink,
) -> Option<tokio::task::JoinHandle<()>> {
    if app.hooks.after.is_empty() {
        return None;
    }
    let ctx = HookContext::new(app, session_id).with_pid(pid);
    let commands = app.hooks.after.clone();
    Some(tokio::spawn(async move {
        let emit = move |event: SessionEvent| {
            // 非阻塞：与输出流同一语义，通道满时计入 overflow 而不是卡住 hook
            let _ = sink.try_write(event);
        };
        for command in &commands {
            let outcome = run_hook(&backend, HookStage::After, command, &ctx, &emit).await;
            if !outcome.success() {
                break;
            }
        }
    }))
}

/// 组装 hook 失败的错误（中文，附输出末尾几行，便于直接在界面上看到原因）。
fn hook_error(stage: HookStage, outcome: &HookOutcome, timeout: Duration) -> AppCatchError {
    let mut message = format!(
        "{} hook `{}` {}",
        stage.label(),
        outcome.command,
        outcome.summary(timeout)
    );
    if !outcome.tail.is_empty() {
        message.push('\n');
        message.push_str(&outcome.tail.join("\n"));
    }
    AppCatchError::HookFailed(message)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn shell_invocation_wraps_command() {
        let (program, args) = shell_invocation("git pull && echo ok");
        if cfg!(windows) {
            assert_eq!(program, "cmd");
            assert_eq!(args.last().map(String::as_str), Some("git pull && echo ok"));
        } else {
            assert_eq!(program, "/bin/sh");
            assert_eq!(
                args,
                vec!["-c".to_string(), "git pull && echo ok".to_string()]
            );
        }
    }

    #[test]
    fn outcome_success_requires_zero_exit() {
        let mut outcome = HookOutcome {
            command: "x".into(),
            code: Some(0),
            timed_out: false,
            spawn_error: None,
            elapsed: Duration::from_millis(10),
            tail: Vec::new(),
        };
        assert!(outcome.success());
        outcome.code = Some(1);
        assert!(!outcome.success());
        outcome.code = Some(0);
        outcome.timed_out = true;
        assert!(!outcome.success());
    }
}
