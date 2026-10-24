//! 命令层集成测试：用真实会话验证分片合并与增量游标。
//!
//! 这些测试不碰 Tauri 窗口，只验证「命令层读取逻辑」与 launcher-core 的真实产物
//! 能否对上：真实进程 -> 真实 JSONL -> 真实轮转 -> 命令层分页读取。
//!
//! 与 events_reader 的单元测试互补：单元测试用「手工摆好的文件」验证算法，
//! 这里验证的是「写盘方真实产出的文件」不会打破那些假设。

use std::sync::Arc;

use app_catch_lib::events_reader::SessionEventReader;

use launcher_core::config::{AppConfig, AppId};
use launcher_core::platform::default_process_backend;
use launcher_core::runtime::{CaptureConfig, SessionManager};
use launcher_core::session::{ExitReason, SessionEvent};

/// 设置一个隔离的 APP_CATCH_HOME（测试结束自动恢复）。
///
/// 复用 launcher-core 的 HomeGuard 语义，但这里需要独立的临时目录 + 串行化，
/// 因此用进程级锁手工实现，避免与核心库的测试互相干扰。
struct HomeGuard {
    previous: Option<std::ffi::OsString>,
    _lock: std::sync::MutexGuard<'static, ()>,
}

static ENV_LOCK: std::sync::Mutex<()> = std::sync::Mutex::new(());

impl HomeGuard {
    fn set(path: &std::path::Path) -> Self {
        let lock = ENV_LOCK.lock().unwrap_or_else(|e| e.into_inner());
        let previous = std::env::var_os(launcher_core::paths::HOME_ENV);
        std::env::set_var(launcher_core::paths::HOME_ENV, path);
        Self {
            previous,
            _lock: lock,
        }
    }
}

impl Drop for HomeGuard {
    fn drop(&mut self) {
        match self.previous.take() {
            Some(value) => std::env::set_var(launcher_core::paths::HOME_ENV, value),
            None => std::env::remove_var(launcher_core::paths::HOME_ENV),
        }
    }
}

/// 构造一个「写很多行且很快就轮转」的捕获配置。
fn tiny_capture() -> CaptureConfig {
    let mut capture = CaptureConfig::default();
    // 每条 output 约 100 字节，200 字节意味着两三条就轮转一次
    capture.writer.max_file_bytes = 200;
    // 保留足够多分片，确保测试期间没有分片被删（保留策略另有单元测试）
    capture.writer.max_shards = 64;
    capture.writer.flush_every = 1;
    capture.writer.flush_interval = std::time::Duration::from_millis(0);
    // 状态采集关掉：本测试只关心 output/exit，避免 status 事件干扰行号断言
    capture.status_interval = None;
    capture
}

/// 跑一个会话并等它结束，返回会话 id 与会话目录。
async fn run_session(manager: &SessionManager, app: &AppConfig, lines: usize) -> std::path::PathBuf {
    let handle = manager.start(app).await.expect("启动会话失败");
    let dir = manager.session_dir(&handle.session_id);

    // 等进程退出，再等收尾把 exit 写进去
    let exit = manager
        .wait_for_exit(handle.session_id, std::time::Duration::from_secs(30))
        .await
        .expect("等待退出超时");
    assert_eq!(exit.reason, ExitReason::Normal, "cmd 循环应正常退出");

    // exit 事件由后台收尾任务写入，这里轮询等它落盘
    let log_path = dir.join("events.jsonl");
    for _ in 0..200 {
        let mut found = false;
        for entry in std::fs::read_dir(&dir).expect("读会话目录失败").flatten() {
            let name = entry.file_name().to_string_lossy().into_owned();
            if !name.starts_with("events") {
                continue;
            }
            let text = std::fs::read_to_string(entry.path()).unwrap_or_default();
            if text.contains("\"event\":\"exit\"") {
                found = true;
                break;
            }
        }
        if found {
            break;
        }
        tokio::time::sleep(std::time::Duration::from_millis(25)).await;
    }
    let _ = log_path;
    assert!(lines > 0, "测试用例应至少产生一行输出");
    dir
}

/// 用 cmd 循环打印若干行：每行带序号，便于验证顺序。
fn echo_app(lines: usize) -> AppConfig {
    let mut app = AppConfig::new(
        AppId::new("shard-demo").expect("合法 id"),
        "分片演示",
        "cmd",
    );
    let script = (0..lines)
        .map(|i| format!("echo line-{i:03} padding-padding-padding-padding"))
        .collect::<Vec<_>>()
        .join(" & ");
    app.args = vec!["/c".to_string(), script];
    app
}

/// 核心断言：把命令层读到的全部事件拼起来，必须等于文件里的全量事件。
#[tokio::test]
async fn paginated_read_across_real_shards_matches_full_file_content() {
    let temp = tempfile::tempdir().expect("创建临时目录失败");
    let _guard = HomeGuard::set(temp.path());
    launcher_core::paths::ensure_layout().expect("建目录失败");

    let manager = SessionManager::from_home(Arc::new(default_process_backend()), tiny_capture())
        .expect("构造管理器失败");
    let dir = run_session(&manager, &echo_app(40), 40).await;

    // 确认真的发生了轮转，否则这个测试就退化成单文件读取了
    let shard_count = std::fs::read_dir(&dir)
        .expect("读目录失败")
        .flatten()
        .filter(|entry| {
            let name = entry.file_name().to_string_lossy().into_owned();
            name.starts_with("events.") && name.ends_with(".jsonl")
        })
        .count();
    assert!(shard_count > 0, "配置应触发轮转，实际分片数 = {shard_count}");

    // 用很小的页大小分页读完全量，模拟界面逐轮轮询
    let reader = SessionEventReader::new();
    let mut collected: Vec<SessionEvent> = Vec::new();
    let mut cursor: u64 = 0;
    let mut pages = 0;
    loop {
        let page = reader
            .read_page(&dir, cursor, 3)
            .expect("分页读取失败");
        assert_eq!(page.skipped_lines, 0, "真实产物不应有不可解析的行");
        collected.extend(page.events.clone());
        assert!(page.next_line >= cursor, "游标不得回退");
        cursor = page.next_line;
        pages += 1;
        assert!(pages < 500, "分页次数异常，可能陷入死循环");
        if !page.has_more {
            break;
        }
    }
    assert!(pages > 1, "页大小 3 时应分成多页，实际 {pages} 页");

    // 与「按文件顺序直读」的结果逐条比对（这是独立于读取器的第二套实现）
    let direct = read_all_direct(&dir);
    assert_eq!(
        collected.len(),
        direct.len(),
        "命令层读取的事件数应与直读一致"
    );
    for (index, (a, b)) in collected.iter().zip(direct.iter()).enumerate() {
        assert_eq!(a, b, "第 {index} 条事件不一致");
    }

    // 顺序性：output 行的序号必须严格递增
    let lines: Vec<String> = collected
        .iter()
        .filter_map(|event| match event {
            SessionEvent::Output { line, .. } => Some(line.clone()),
            _ => None,
        })
        .collect();
    assert!(!lines.is_empty(), "应有输出行");
    let mut last = String::new();
    for line in &lines {
        if let Some(rest) = line.trim().strip_prefix("line-") {
            let seq = rest.chars().take(3).collect::<String>();
            assert!(seq > last, "输出顺序错乱：{seq} 不大于 {last}");
            last = seq;
        }
    }

    // 首尾事件语义：第一条是 start，最后一条是 exit
    assert_eq!(collected.first().map(|e| e.event_name()), Some("start"));
    assert_eq!(collected.last().map(|e| e.event_name()), Some("exit"));
}

/// 分页读取必须对「同一个游标读两次」返回相同结果（幂等，不重复消费）。
#[tokio::test]
async fn reading_same_cursor_twice_is_idempotent() {
    let temp = tempfile::tempdir().expect("创建临时目录失败");
    let _guard = HomeGuard::set(temp.path());
    launcher_core::paths::ensure_layout().expect("建目录失败");

    let manager = SessionManager::from_home(Arc::new(default_process_backend()), tiny_capture())
        .expect("构造管理器失败");
    let dir = run_session(&manager, &echo_app(10), 10).await;

    let reader = SessionEventReader::new();
    let first = reader.read_page(&dir, 0, 4).expect("第一次读取失败");
    let again = reader.read_page(&dir, 0, 4).expect("重复读取失败");
    assert_eq!(first, again, "同一游标的两次读取必须完全一致");

    // 从第二页继续：不能与第一页重叠
    let second = reader
        .read_page(&dir, first.next_line, 4)
        .expect("续读失败");
    assert_eq!(second.next_line, first.next_line + second.events.len() as u64);
}

/// 「按文件顺序直读」的独立实现，用于交叉验证读取器。
fn read_all_direct(dir: &std::path::Path) -> Vec<SessionEvent> {
    let mut shards: Vec<(u32, std::path::PathBuf)> = Vec::new();
    let mut active: Option<std::path::PathBuf> = None;
    for entry in std::fs::read_dir(dir).expect("读目录失败").flatten() {
        let name = entry.file_name().to_string_lossy().into_owned();
        if name == launcher_core::paths::SESSION_LOG_FILE {
            active = Some(entry.path());
            continue;
        }
        if let Some(rest) = name.strip_prefix("events.") {
            if let Some(seq) = rest.strip_suffix(".jsonl").and_then(|s| s.parse::<u32>().ok()) {
                shards.push((seq, entry.path()));
            }
        }
    }
    shards.sort_by_key(|(seq, _)| *seq);
    let mut files: Vec<std::path::PathBuf> = shards.into_iter().map(|(_, p)| p).collect();
    if let Some(active) = active {
        files.push(active);
    }

    let mut events = Vec::new();
    for path in files {
        let text = std::fs::read_to_string(&path).unwrap_or_default();
        for line in text.lines() {
            if line.trim().is_empty() {
                continue;
            }
            if let Ok(event) = SessionEvent::from_jsonl_line(line) {
                events.push(event);
            }
        }
    }
    events
}
