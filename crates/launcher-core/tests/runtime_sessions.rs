//! 运行时集成测试：真实拉起子进程，验证捕获、退出分类、杀树、轮转与重关联。
//!
//! 隔离原则：每个测试把 `APP_CATCH_HOME` 指向独立 `tempfile` 临时目录，
//! 绝不触碰真实的 `~/.app_catch`。
//!
//! 注意：`APP_CATCH_HOME` 是进程级全局变量，cargo test 默认多线程并行会互相踩。
//! 这里用一把进程内互斥锁串行化所有涉及环境变量或真实进程的测试。

use std::sync::{Arc, Mutex, OnceLock};
use std::time::Duration;

use launcher_core::config::{AppConfig, AppId};
use launcher_core::platform::default_process_backend;
use launcher_core::runtime::capture::CaptureConfig;
use launcher_core::runtime::log_writer::LogWriterConfig;
use launcher_core::runtime::{ExitInfo, SessionManager};
use launcher_core::session::{ExitReason, RunState, SessionEvent, SessionId};

/// 串行化全局环境变量与真实进程测试的互斥锁。
fn env_lock() -> &'static Mutex<()> {
    static LOCK: OnceLock<Mutex<()>> = OnceLock::new();
    LOCK.get_or_init(|| Mutex::new(()))
}

/// 测试用隔离环境：持有临时目录与进程内锁。
struct TestEnv {
    _dir: tempfile::TempDir,
    home: std::path::PathBuf,
    _guard: std::sync::MutexGuard<'static, ()>,
}

impl TestEnv {
    fn new() -> Self {
        let guard = env_lock().lock().unwrap_or_else(|e| e.into_inner());
        let dir = tempfile::tempdir().expect("创建临时目录失败");
        let home = dir.path().to_path_buf();
        std::env::set_var("APP_CATCH_HOME", &home);
        Self {
            _dir: dir,
            home,
            _guard: guard,
        }
    }

    fn manager(&self, capture: CaptureConfig) -> SessionManager {
        SessionManager::from_home(Arc::new(default_process_backend()), capture)
            .expect("构造 SessionManager 失败")
    }

    fn sessions_root(&self) -> std::path::PathBuf {
        self.home.join("sessions")
    }
}

impl Drop for TestEnv {
    fn drop(&mut self) {
        std::env::remove_var("APP_CATCH_HOME");
    }
}

/// 构造一个要执行 `cmd` 的应用配置。
fn cmd_app(args: &[&str]) -> AppConfig {
    let mut app = AppConfig::new(
        AppId::new("test-app").expect("合法 id"),
        "测试应用",
        "cmd",
    );
    app.args = args.iter().map(|s| s.to_string()).collect();
    app
}

/// 读出会话日志的全部事件（主文件 + 分片，按 seq 顺序）。
fn read_all_events(dir: &std::path::Path) -> Vec<SessionEvent> {
    let mut files: Vec<std::path::PathBuf> = Vec::new();
    let mut shards: Vec<(u32, std::path::PathBuf)> = Vec::new();
    for entry in std::fs::read_dir(dir).expect("读会话目录失败").flatten() {
        let name = entry.file_name().to_string_lossy().into_owned();
        if name == "events.jsonl" {
            files.push(entry.path());
        } else if let Some(rest) = name.strip_prefix("events.") {
            if let Some(seq) = rest.strip_suffix(".jsonl") {
                if let Ok(seq) = seq.parse::<u32>() {
                    shards.push((seq, entry.path()));
                }
            }
        }
    }
    shards.sort_by_key(|(seq, _)| *seq);
    let mut ordered: Vec<std::path::PathBuf> = shards.into_iter().map(|(_, p)| p).collect();
    // 主文件是当前活跃片，排在最后
    ordered.extend(files);

    let mut events = Vec::new();
    for path in ordered {
        let text = std::fs::read_to_string(&path).expect("读日志失败");
        for line in text.lines() {
            if line.trim().is_empty() {
                continue;
            }
            let event = SessionEvent::from_jsonl_line(line)
                .unwrap_or_else(|e| panic!("日志行不可解析（{}）: {e}\n{line}", path.display()));
            events.push(event);
        }
    }
    events
}

/// 统计 output 事件数。
fn count_outputs(events: &[SessionEvent]) -> usize {
    events
        .iter()
        .filter(|e| matches!(e, SessionEvent::Output { .. }))
        .count()
}

/// 取最后一条 exit 事件。
fn last_exit(events: &[SessionEvent]) -> Option<(Option<i32>, ExitReason)> {
    events.iter().rev().find_map(|e| match e {
        SessionEvent::Exit { code, reason, .. } => Some((*code, *reason)),
        _ => None,
    })
}

/// 用独立的系统进程快照交叉验证某 pid 是否已消失。
///
/// 与本 crate 内部的 `WaitForSingleObject` 判定是两条独立证据：
/// 前者看「系统快照里还有没有这个 pid」，后者看「进程对象的信号状态」。
/// 不使用外部 `tasklist` 命令——子进程调用在测试环境里可能因句柄继承而阻塞。
fn pid_gone(pid: u32) -> bool {
    !launcher_core::platform::pid_exists_in_snapshot(pid)
        && !launcher_core::platform::is_process_alive(pid)
}

/// 轮询等待条件成立（避免固定 sleep 造成的 flaky）。
async fn wait_until<F: FnMut() -> bool>(mut cond: F, timeout: Duration) -> bool {
    let deadline = tokio::time::Instant::now() + timeout;
    loop {
        if cond() {
            return true;
        }
        if tokio::time::Instant::now() >= deadline {
            return false;
        }
        tokio::time::sleep(Duration::from_millis(10)).await;
    }
}

// ============================ a) 正常短命进程 ============================

#[tokio::test]
async fn short_lived_process_captures_output_and_exits_normally() {
    let env = TestEnv::new();
    let manager = env.manager(CaptureConfig::default());

    // 注意：cmd 里 "echo A &" 中 & 前的空格会进入输出（"A "），故用 "echo A&echo B" 形式
    let app = cmd_app(&["/c", "echo A&echo B"]);
    let session = manager.start(&app).await.expect("启动失败");
    let exit = manager
        .wait_for_exit(session.session_id, Duration::from_secs(30))
        .await
        .expect("等待退出失败");

    assert_eq!(exit.code, Some(0), "短命进程应正常退出");
    assert_eq!(exit.reason, ExitReason::Normal);

    let dir = manager.session_dir(&session.session_id);
    let events = read_all_events(&dir);
    assert_eq!(
        count_outputs(&events),
        2,
        "应捕获到 A/B 两行，实际事件: {events:?}"
    );
    let lines: Vec<&str> = events
        .iter()
        .filter_map(|e| match e {
            SessionEvent::Output { line, .. } => Some(line.as_str()),
            _ => None,
        })
        .collect();
    assert!(lines.contains(&"A"), "应包含 A，实际: {lines:?}");
    assert!(lines.contains(&"B"), "应包含 B，实际: {lines:?}");
}

// ============================ b) 非零退出 ============================

#[tokio::test]
async fn non_zero_exit_is_classified_as_crashed() {
    let env = TestEnv::new();
    let manager = env.manager(CaptureConfig::default());

    let app = cmd_app(&["/c", "exit /b 3"]);
    let session = manager.start(&app).await.expect("启动失败");
    let exit = manager
        .wait_for_exit(session.session_id, Duration::from_secs(30))
        .await
        .expect("等待退出失败");

    assert_eq!(exit.code, Some(3), "应拿到真实退出码 3");
    assert_eq!(exit.reason, ExitReason::Crashed, "非零退出应判定为崩溃");

    let events = read_all_events(&manager.session_dir(&session.session_id));
    assert_eq!(
        last_exit(&events),
        Some((Some(3), ExitReason::Crashed)),
        "exit 事件应与快照一致"
    );
}

// ============================ c) 主动 stop ============================

#[tokio::test]
async fn stop_kills_long_running_process_and_marks_killed() {
    let env = TestEnv::new();
    let manager = env.manager(CaptureConfig::default());

    // ping 会在 Windows 上稳定跑约 60 秒，足够我们从容停止
    let app = cmd_app(&["/c", "ping -n 60 127.0.0.1 > nul"]);
    let session = manager.start(&app).await.expect("启动失败");
    let pid = session.handle.pid;

    // 确认它真的在跑
    assert!(
        wait_until(|| launcher_core::platform::is_process_alive(pid), Duration::from_secs(10)).await,
        "进程应处于运行中"
    );

    let active = manager.list_active().await;
    assert_eq!(active.len(), 1, "应有 1 个运行中会话");

    manager
        .stop(session.session_id)
        .await
        .expect("停止会话失败");

    // stop 返回时进程树应已消失（kill_tree 内部会等待）
    assert!(
        !launcher_core::platform::is_process_alive(pid),
        "stop 返回后 pid {pid} 应已消失"
    );

    let exit = manager
        .wait_for_exit(session.session_id, Duration::from_secs(30))
        .await
        .expect("等待退出失败");
    assert_eq!(exit.reason, ExitReason::Killed, "主动停止应判定为 killed");

    let events = read_all_events(&manager.session_dir(&session.session_id));
    assert_eq!(
        last_exit(&events).map(|(_, reason)| reason),
        Some(ExitReason::Killed)
    );
}

// ============================ d) kill_tree 杀孙进程 ============================

#[tokio::test]
async fn stop_reclaims_grandchild_processes() {
    let env = TestEnv::new();
    let manager = env.manager(CaptureConfig::default());

    // 父 cmd 起一个后台子 cmd（ping 长跑），自己也在跑 ping，形成 父 -> 子 的树。
    // 用 call 让子 cmd 在同一条 cmd 进程树里，便于验证 Job Object 的整树回收。
    let app = cmd_app(&[
        "/c",
        "start /b cmd /c ping -n 60 127.0.0.1 > nul& ping -n 60 127.0.0.1 > nul",
    ]);
    let session = manager.start(&app).await.expect("启动失败");
    let parent_pid = session.handle.pid;

    // 等到确实出现了子孙进程
    let mut descendants: Vec<u32> = Vec::new();
    for _ in 0..200 {
        descendants = launcher_core::platform::descendant_pids(parent_pid);
        if !descendants.is_empty() {
            break;
        }
        tokio::time::sleep(Duration::from_millis(25)).await;
    }
    assert!(
        !descendants.is_empty(),
        "父进程 {parent_pid} 应至少有一个子进程，用于验证杀树"
    );
    assert!(
        descendants.iter().any(|p| launcher_core::platform::is_process_alive(*p)),
        "子进程应处于运行中: {descendants:?}"
    );

    manager
        .stop(session.session_id)
        .await
        .expect("停止会话失败");

    // 父与所有子进程都必须消失
    assert!(
        !launcher_core::platform::is_process_alive(parent_pid),
        "父进程 {parent_pid} 应已消失"
    );
    let survivors: Vec<u32> = descendants
        .iter()
        .copied()
        .filter(|p| launcher_core::platform::is_process_alive(*p))
        .collect();
    assert!(survivors.is_empty(), "子孙进程应被一并回收，仍存活: {survivors:?}");

    // 交叉验证：系统快照 + 进程对象两条独立证据都必须表明 pid 已消失
    println!("父 pid {parent_pid} / 子孙 {:?} 应已全部回收", descendants);
    for pid in std::iter::once(parent_pid).chain(descendants.iter().copied()) {
        let in_snapshot = launcher_core::platform::pid_exists_in_snapshot(pid);
        let obj_alive = launcher_core::platform::is_process_alive(pid);
        println!("pid {pid}: 快照中存在={in_snapshot}, 进程对象仍存活={obj_alive}");
        assert!(
            pid_gone(pid),
            "pid {pid} 未被回收（快照中存在={in_snapshot}, 进程对象存活={obj_alive}）"
        );
    }
}

// ============================ f) 编码保真（GBK / UTF-8 / lossy） ============================

/// 「中文测试」的 GBK(cp936) 字节序列：这是 Windows 中文环境的默认控制台编码。
const GBK_ZHONGWEN_CESHI: &[u8] = &[0xD6, 0xD0, 0xCE, 0xC4, 0xB2, 0xE2, 0xCA, 0xD4];

/// 既不是合法 UTF-8、也不是合法 GBK 的字节对（0x81 0x30 在 GBK 中无映射）。
const UNDECODABLE_BYTES: &[u8] = &[0x81, 0x30];

fn temp_bytes_file(name: &str, bytes: &[u8]) -> (tempfile::TempDir, String) {
    let dir = tempfile::tempdir().expect("创建临时目录失败");
    let path = dir.path().join(name);
    std::fs::write(&path, bytes).expect("写临时字节文件失败");
    let text = path.to_string_lossy().into_owned();
    (dir, text)
}

/// 启动真实子进程，把若干「文件内容」按顺序原样写到 stdout。
///
/// Windows 的 cmd 内置命令 type 按字节透传文件内容（不做编码转换），
/// 因此这是构造**真 GBK 输出流**的最直接办法。
async fn run_type_session(
    env: &TestEnv,
    files: &[&str],
    capture: CaptureConfig,
) -> (std::path::PathBuf, Vec<SessionEvent>) {
    let manager = env.manager(capture);
    let command = files
        .iter()
        .map(|f| format!("type {f}"))
        .collect::<Vec<_>>()
        .join(" & ");
    let app = cmd_app(&["/c", &command]);

    let session = manager.start(&app).await.expect("启动失败");
    manager
        .wait_for_exit(session.session_id, Duration::from_secs(30))
        .await
        .expect("等待退出失败");

    let dir = manager.session_dir(&session.session_id);
    let events = read_all_events(&dir);
    (dir, events)
}

/// 取全部 output 事件里的原始行（不做任何归一化）。
fn output_lines(events: &[SessionEvent]) -> Vec<String> {
    events
        .iter()
        .filter_map(|e| match e {
            SessionEvent::Output { line, .. } => Some(line.clone()),
            _ => None,
        })
        .collect()
}

/// 取输出行对应的编码标记。
fn output_encodings(events: &[SessionEvent]) -> Vec<String> {
    events
        .iter()
        .filter_map(|e| match e {
            SessionEvent::Output { encoding, .. } => Some(encoding.as_str().to_string()),
            _ => None,
        })
        .collect()
}

/// 缺陷回归：GBK 字节「中文测试」必须无损还原，且标记 encoding=gbk。
///
/// 修复前：decode_line 用 from_utf8_lossy，6 个原始字节被替换为 U+FFFD，
/// 落盘为替换字符 + Ĳ 的乱码，原始信息不可还原。
#[tokio::test]
async fn gbk_line_is_decoded_losslessly_and_tagged() {
    let env = TestEnv::new();
    let (_keep, path) = temp_bytes_file("gbk.txt", &[GBK_ZHONGWEN_CESHI, b"\n"].concat());

    let (dir, events) = run_type_session(&env, &[&path], CaptureConfig::default()).await;

    let lines = output_lines(&events);
    assert_eq!(
        lines,
        vec!["中文测试".to_string()],
        "GBK 行应无损还原: {lines:?}"
    );
    assert_eq!(output_encodings(&events), vec!["gbk".to_string()]);
    assert!(
        !lines[0].contains(char::REPLACEMENT_CHARACTER),
        "不得出现替换字符 U+FFFD: {:?}",
        lines[0]
    );

    // 落盘原文同样不得含替换字符，并显式带上 encoding 字段
    let raw = std::fs::read_to_string(dir.join("events.jsonl")).expect("读日志失败");
    let output_line = raw
        .lines()
        .find(|l| l.contains("\"event\":\"output\""))
        .expect("应有 output 行");
    assert!(
        !output_line.contains(char::REPLACEMENT_CHARACTER),
        "JSONL 中不得出现替换字符: {output_line}"
    );
    assert!(
        output_line.contains("\"encoding\":\"gbk\""),
        "JSONL 应标记 encoding=gbk: {output_line}"
    );
    assert!(
        output_line.contains("\"line\":\"中文测试\""),
        "JSONL 行内容应无损: {output_line}"
    );
}

/// UTF-8 正例：中文 UTF-8 输出解析正确并标记 encoding=utf-8。
#[tokio::test]
async fn utf8_line_is_decoded_and_tagged() {
    let env = TestEnv::new();
    let (_keep, path) = temp_bytes_file("u8.txt", "UTF-8 中文输出测试\n".as_bytes());

    let (_dir, events) = run_type_session(&env, &[&path], CaptureConfig::default()).await;

    assert_eq!(
        output_lines(&events),
        vec!["UTF-8 中文输出测试".to_string()]
    );
    assert_eq!(output_encodings(&events), vec!["utf-8".to_string()]);
}

/// 混合流：同一进程先后输出 UTF-8 行与 GBK 行，两行各自按自己的编码解码。
#[tokio::test]
async fn mixed_utf8_and_gbk_lines_are_decoded_independently() {
    let env = TestEnv::new();
    let (_keep_u8, u8_path) = temp_bytes_file("u8.txt", "first-utf8 中文\n".as_bytes());
    let (_keep_gbk, gbk_path) = temp_bytes_file("gbk.txt", &[GBK_ZHONGWEN_CESHI, b"\n"].concat());

    let (_dir, events) =
        run_type_session(&env, &[&u8_path, &gbk_path], CaptureConfig::default()).await;

    assert_eq!(
        output_lines(&events),
        vec!["first-utf8 中文".to_string(), "中文测试".to_string()],
        "UTF-8 与 GBK 行应各自正确解码"
    );
    assert_eq!(
        output_encodings(&events),
        vec!["utf-8".to_string(), "gbk".to_string()]
    );
}

/// 非法字节（既非 UTF-8 也非合法 GBK）：不 panic、不丢行，并明确标记 lossy。
#[tokio::test]
async fn undecodable_bytes_are_kept_and_marked_lossy() {
    let env = TestEnv::new();
    let mut bytes: Vec<u8> = Vec::new();
    bytes.extend_from_slice(b"ok-before\n");
    bytes.extend_from_slice(UNDECODABLE_BYTES);
    bytes.extend_from_slice(b"\nok-after\n");

    let (_keep, path) = temp_bytes_file("bad.bin", &bytes);
    let (dir, events) = run_type_session(&env, &[&path], CaptureConfig::default()).await;

    let lines = output_lines(&events);
    assert_eq!(lines.len(), 3, "非法字节行不得被丢弃: {lines:?}");
    assert_eq!(lines[0], "ok-before");
    assert_eq!(lines[2], "ok-after");
    assert!(
        lines[1].contains(char::REPLACEMENT_CHARACTER),
        "无法解码时应以替换字符表示: {lines:?}"
    );
    assert_eq!(
        output_encodings(&events),
        vec![
            "utf-8".to_string(),
            "lossy".to_string(),
            "utf-8".to_string()
        ],
        "只有无法解码的那一行标记 lossy"
    );

    let raw = std::fs::read_to_string(dir.join("events.jsonl")).expect("读日志失败");
    assert!(
        raw.contains("\"encoding\":\"lossy\""),
        "JSONL 应显式标记 lossy 行: {raw}"
    );
}

// ============================ e) 大量输出不丢行 ============================

#[tokio::test]
async fn high_volume_output_writes_every_line() {
    let env = TestEnv::new();
    let manager = env.manager(CaptureConfig::default());

    // 5000 行 + 一行结束标记。
    // 循环体必须整体放在括号里，否则 "& echo DONE" 会被算进循环体，DONE 也执行 5000 次。
    let app = cmd_app(&[
        "/c",
        "(for /L %i in (1,1,5000) do @echo LINE_%i) & echo DONE",
    ]);
    let session = manager.start(&app).await.expect("启动失败");
    let exit = manager
        .wait_for_exit(session.session_id, Duration::from_secs(120))
        .await
        .expect("等待退出失败");
    assert_eq!(exit.code, Some(0));

    let events = read_all_events(&manager.session_dir(&session.session_id));
    let outputs = count_outputs(&events);
    let overflow: u64 = events
        .iter()
        .filter_map(|e| match e {
            SessionEvent::Overflow { dropped, .. } => Some(*dropped),
            _ => None,
        })
        .sum();

    assert_eq!(overflow, 0, "channel 容量足够时不应丢行，实际丢弃 {overflow}");
    assert_eq!(outputs, 5001, "5000 行 + DONE 应全部落盘，实际 {outputs}");

    // 行内容完整且顺序正确
    let lines: Vec<String> = events
        .iter()
        .filter_map(|e| match e {
            SessionEvent::Output { line, .. } => Some(line.clone()),
            _ => None,
        })
        .collect();
    assert_eq!(lines.first().map(String::as_str), Some("LINE_1"));
    assert_eq!(lines.get(4999).map(String::as_str), Some("LINE_5000"));
    assert_eq!(lines.last().map(String::as_str), Some("DONE"));
}

// ============================ f) 轮转 ============================

#[tokio::test]
async fn rotation_creates_shards_with_intact_lines_and_retention() {
    let env = TestEnv::new();
    let mut capture = CaptureConfig::default();
    // 4KB 一片 + 只保留 2 片，确保小体量输出也能触发多轮轮转
    capture.writer = LogWriterConfig {
        max_file_bytes: 4096,
        max_shards: 2,
        ..LogWriterConfig::default()
    };
    let manager = env.manager(capture);

    let app = cmd_app(&[
        "/c",
        "(for /L %i in (1,1,800) do @echo ROTATION_LINE_%i_PADDING_PADDING_PADDING)",
    ]);
    let session = manager.start(&app).await.expect("启动失败");
    manager
        .wait_for_exit(session.session_id, Duration::from_secs(120))
        .await
        .expect("等待退出失败");

    let dir = manager.session_dir(&session.session_id);
    let mut shards: Vec<u32> = Vec::new();
    let mut active_exists = false;
    for entry in std::fs::read_dir(&dir).expect("读目录失败").flatten() {
        let name = entry.file_name().to_string_lossy().into_owned();
        if name == "events.jsonl" {
            active_exists = true;
        } else if let Some(rest) = name.strip_prefix("events.") {
            if let Some(seq) = rest.strip_suffix(".jsonl") {
                if let Ok(seq) = seq.parse::<u32>() {
                    shards.push(seq);
                }
            }
        }
    }
    shards.sort_unstable();
    assert!(active_exists, "活跃片 events.jsonl 必须存在");
    assert!(!shards.is_empty(), "应产生至少一个分片，实际: {shards:?}");
    assert!(
        shards.len() <= 2,
        "保留上限 max_shards=2 应生效，实际分片: {shards:?}"
    );

    // 每个分片的每一行都必须是完整可解析的 JSON（轮转绝不切断行）
    for seq in &shards {
        let path = dir.join(format!("events.{seq}.jsonl"));
        let text = std::fs::read_to_string(&path).expect("读分片失败");
        assert!(!text.is_empty(), "分片 events.{seq}.jsonl 不应为空");
        for line in text.lines() {
            assert!(!line.is_empty(), "分片不应有空行: {line}");
            serde_json::from_str::<serde_json::Value>(line)
                .unwrap_or_else(|e| panic!("分片 events.{seq}.jsonl 存在不可解析行: {e}\n{line}"));
        }
    }

    // 分片里的内容是完整的 output 行（没有被切成半行）
    for seq in &shards {
        let text = std::fs::read_to_string(dir.join(format!("events.{seq}.jsonl"))).expect("读分片失败");
        for line in text.lines() {
            let event = SessionEvent::from_jsonl_line(line).expect("分片行应为合法事件");
            if let SessionEvent::Output { line: content, .. } = event {
                assert!(
                    content.starts_with("ROTATION_LINE_"),
                    "分片内容应为完整输出行，实际: {content:?}"
                );
            }
        }
    }
}

// ============================ g) 单 writer 所有权 ============================

/// 缺陷回归：状态循环**不得**自己开 `LogWriter`。
///
/// 修复前的写法是「每条 status 事件新开一个 writer」。每个 `LogWriter` 都自带
/// `shard_seq` 与「已轮转分片」这两个内存状态，于是：
/// - 序号从 0 重新起算 → 分片名乱跳（实测出现过 `[1, 28, 29]`）；
/// - prune 只看自己那份不完整的列表 → **旧分片永远删不掉**，长跑服务撑爆磁盘。
///
/// 这条用例把轮转压到极小、把状态采集压到极快，逼出「状态事件与输出事件交错轮转」
/// 的场景，然后断言两条不变量：
/// 1. 磁盘上的分片数不超过 `max_shards`（保留策略对**全部分片**生效）；
/// 2. 分片序号从 1 开始**连续不断档**——多 writer 竞争时必然出现空洞。
#[tokio::test]
async fn status_events_share_one_writer_with_output() {
    let env = TestEnv::new();
    let mut capture = CaptureConfig::default();
    // 1KB 一片 + 只留 3 片：少量输出就能触发多轮轮转
    capture.writer = LogWriterConfig {
        max_file_bytes: 1024,
        max_shards: 3,
        ..LogWriterConfig::default()
    };
    // 10ms 采一次：状态事件会密集插进输出流里
    capture.status_interval = Some(Duration::from_millis(10));
    capture.max_status_events = None;
    let manager = env.manager(capture).with_status_probe(Arc::new(CountingProbe));

    let app = cmd_app(&[
        "/c",
        "(for /L %i in (1,1,600) do @echo SW_LINE_%i_PADDING_PADDING_PADDING)",
    ]);
    let session = manager.start(&app).await.expect("启动失败");
    manager
        .wait_for_exit(session.session_id, Duration::from_secs(120))
        .await
        .expect("等待退出失败");

    let dir = manager.session_dir(&session.session_id);
    let mut shards: Vec<u32> = Vec::new();
    for entry in std::fs::read_dir(&dir).expect("读目录失败").flatten() {
        let name = entry.file_name().to_string_lossy().into_owned();
        if let Some(rest) = name.strip_prefix("events.") {
            if let Some(seq) = rest.strip_suffix(".jsonl") {
                if let Ok(seq) = seq.parse::<u32>() {
                    shards.push(seq);
                }
            }
        }
    }
    shards.sort_unstable();

    // 不变量 1：保留策略对全部分片生效
    assert!(
        shards.len() <= 3,
        "保留上限 max_shards=3 应生效，实际分片: {shards:?}"
    );

    // 不变量 2：序号从 1 起连续单调递增（分片被 prune 掉的是**最旧**的，
    // 因此磁盘上留下的必然是一段连续的尾部区间）
    if let Some(&first) = shards.first() {
        for (offset, seq) in shards.iter().enumerate() {
            assert_eq!(
                *seq,
                first + offset as u32,
                "分片序号必须连续无空洞（多 writer 竞争会打出空洞），实际: {shards:?}"
            );
        }
    }

    // 不变量 3：保留策略只删**最旧**的分片，因此留下的尾部里 exit 必须还在最后一位。
    // 注意这里不能断言「首条是 start」——轮转 + 保留本来就是要把最早的 start 丢掉，
    // 这正是保留策略在起作用的证据（全量事件的可读性由 SESSION_FORMAT 的读取约定保证）。
    let events = read_all_events(&dir);
    assert!(
        matches!(events.last(), Some(SessionEvent::Exit { .. })),
        "末条必须是 exit，实际: {:?}",
        events.last()
    );
    assert!(
        events.iter().any(|e| matches!(e, SessionEvent::Output { .. })),
        "保留下来的分片里应仍有 output 事件"
    );
    println!(
        "分片序号 {:?}（连续无空洞），保留事件 {} 条，末条 exit",
        shards,
        events.len()
    );
}

/// 状态事件的返回不阻塞收尾：进程退出后循环必须立刻醒，而不是睡满一个采集间隔。
///
/// 修复前状态循环只靠 `sleep(interval)` + 轮询 `RunState` 退出，而 `RunState`
/// 又只在守护任务最后一步才翻转，于是每个会话稳定被拖掉一个完整采集间隔
/// （默认 3s，集成测试从 ~2s 涨到 51s 正是这个原因）。
///
/// 这里把采集间隔设成 2s（远大于收尾应有的开销），断言「stop 到 exit 落盘」
/// 在 1s 内完成——只有靠显式信号唤醒才可能做到。
#[tokio::test]
async fn exit_does_not_wait_for_a_full_status_interval() {
    let env = TestEnv::new();
    let mut capture = CaptureConfig::default();
    // 采集间隔远大于允许的收尾耗时：靠 sleep 醒来的实现必然超时
    capture.status_interval = Some(Duration::from_secs(2));
    capture.max_status_events = None;
    let manager = env.manager(capture).with_status_probe(Arc::new(CountingProbe));

    let app = cmd_app(&["/c", "ping -n 60 127.0.0.1 > nul"]);
    let session = manager.start(&app).await.expect("启动失败");

    // 等状态循环真的跑起来并进入 sleep（第一轮采样完成后必然在等）
    tokio::time::sleep(Duration::from_millis(300)).await;

    let started = std::time::Instant::now();
    manager.stop(session.session_id).await.expect("停止失败");
    manager
        .wait_for_exit(session.session_id, Duration::from_secs(30))
        .await
        .expect("等待退出失败");
    let elapsed = started.elapsed();

    assert!(
        elapsed < Duration::from_secs(1),
        "收尾应被显式信号立即唤醒，不该等满 2s 采集间隔，实测 {elapsed:?}"
    );

    // exit 仍是末条：唤醒机制不得让 status 落到 exit 之后
    let events = read_all_events(&manager.session_dir(&session.session_id));
    assert!(
        matches!(events.last(), Some(SessionEvent::Exit { .. })),
        "末条应为 exit，实际: {:?}",
        events.last()
    );
    println!("stop -> exit 耗时 {elapsed:?}（采集间隔 2s）");
}

/// 测试用固定值 probe：只填窗口标题，不需要进程树枚举。
struct CountingProbe;

impl launcher_core::runtime::StatusProbe for CountingProbe {
    fn probe(
        &self,
        _ctx: &launcher_core::runtime::ProbeContext,
    ) -> launcher_core::runtime::StatusSnapshot {
        launcher_core::runtime::StatusSnapshot {
            window_title: Some("单 writer 用例".to_string()),
            ports: Vec::new(),
        }
    }

    fn name(&self) -> &'static str {
        "counting"
    }

    /// 不枚举整机进程表：这条用例只关心写盘路径，不应该被 ToolHelp 快照拖慢
    fn needs_process_tree(&self) -> bool {
        false
    }
}

// ============================ i) 事件流自洽 ============================

#[tokio::test]
async fn event_stream_is_self_consistent() {
    let env = TestEnv::new();
    let manager = env.manager(CaptureConfig::default());

    let app = cmd_app(&["/c", "echo first&echo second"]);
    let session = manager.start(&app).await.expect("启动失败");
    manager
        .wait_for_exit(session.session_id, Duration::from_secs(30))
        .await
        .expect("等待退出失败");

    let events = read_all_events(&manager.session_dir(&session.session_id));
    assert!(events.len() >= 3, "至少应有 start/output/exit 三类事件");

    // start 必须是首条
    assert!(matches!(events.first(), Some(SessionEvent::Start { .. })), "首条必须是 start");
    // exit 必须是末条
    assert!(matches!(events.last(), Some(SessionEvent::Exit { .. })), "末条必须是 exit");

    // 所有事件版本号一致且为 1
    for event in &events {
        assert_eq!(event.version(), 1, "所有事件版本应为 1: {event:?}");
        assert!(!event.timestamp().is_empty(), "事件必须有时间戳");
    }

    // start 事件字段齐全，且 start_time 可用于重关联
    match events.first() {
        Some(SessionEvent::Start {
            session_id,
            app_id,
            cmd,
            cwd,
            argv,
            pid,
            ppid,
            start_time,
            ..
        }) => {
            assert_eq!(*session_id, session.session_id);
            assert_eq!(app_id, "test-app");
            assert!(!cmd.is_empty(), "cmd 不应为空");
            assert!(!cwd.is_empty(), "cwd 不应为空");
            assert_eq!(argv.first().map(String::as_str), Some("cmd"));
            assert_eq!(*pid, session.handle.pid);
            assert!(*ppid > 0, "ppid 应为启动器 pid");
            assert_eq!(*start_time, session.handle.start_time.as_raw());
        }
        other => panic!("首条应为 start，实际: {other:?}"),
    }
}

// ============================ h) 重关联 ============================

/// 手工构造一个「有 start 无 exit」的会话目录。
fn craft_dangling_session(
    root: &std::path::Path,
    pid: u32,
    start_time: u64,
) -> (SessionId, std::path::PathBuf) {
    let session_id = SessionId::new_v4();
    let dir = root.join(session_id.to_string());
    std::fs::create_dir_all(&dir).expect("创建会话目录失败");

    let event = SessionEvent::Start {
        v: 1,
        ts: "2026-01-01T00:00:00Z".to_string(),
        session_id,
        app_id: "test-app".to_string(),
        cmd: "cmd /c ping -n 60 127.0.0.1 > nul".to_string(),
        cwd: "C:/".to_string(),
        argv: vec!["cmd".to_string()],
        pid,
        ppid: std::process::id(),
        start_time,
    };
    let line = event.to_jsonl_line().expect("序列化 start 失败");
    std::fs::write(dir.join("events.jsonl"), format!("{line}\n")).expect("写会话日志失败");
    (session_id, dir)
}

#[tokio::test]
async fn recover_marks_stale_pid_as_crashed_when_start_time_mismatches() {
    let env = TestEnv::new();
    let manager = env.manager(CaptureConfig::default());
    let root = env.sessions_root();
    std::fs::create_dir_all(&root).expect("创建 sessions 目录失败");

    // 用本进程真实 pid，但启动时间故意填错 -> 必须判定为 PID 复用
    let real_pid = std::process::id();
    let wrong_start_time = 1u64;
    let (session_id, dir) = craft_dangling_session(&root, real_pid, wrong_start_time);

    let recovered = manager.recover_sessions().await.expect("重关联失败");
    assert_eq!(recovered.len(), 1, "应识别出 1 个未完成会话");
    assert_eq!(recovered[0].session_id, session_id);
    assert_eq!(
        recovered[0].state,
        RunState::Crashed,
        "启动时间不匹配应判定为崩溃（PID 已被复用）"
    );
    assert_eq!(
        recovered[0].last_exit,
        Some(ExitInfo {
            code: None,
            reason: ExitReason::Crashed
        })
    );

    // 必须真的补写了 exit 事件
    let events = read_all_events(&dir);
    assert_eq!(
        last_exit(&events),
        Some((None, ExitReason::Crashed)),
        "应补写 exit(crashed)"
    );
}

#[tokio::test]
async fn recover_marks_live_process_as_running() {
    let env = TestEnv::new();
    let manager = env.manager(CaptureConfig::default());

    // 先真的起一个长跑进程，拿到它的真实 pid + start_time
    let app = cmd_app(&["/c", "ping -n 60 127.0.0.1 > nul"]);
    let session = manager.start(&app).await.expect("启动失败");
    let pid = session.handle.pid;
    let start_time = session.handle.start_time.as_raw();

    assert!(
        wait_until(|| launcher_core::platform::is_process_alive(pid), Duration::from_secs(10)).await,
        "进程应处于运行中"
    );

    // 用真实身份构造一个「有 start 无 exit」的历史会话，模拟上一次启动器留下的残局
    let root = env.sessions_root();
    let (recovered_id, dir) = craft_dangling_session(&root, pid, start_time);

    let recovered = manager.recover_sessions().await.expect("重关联失败");
    let target = recovered
        .iter()
        .find(|s| s.session_id == recovered_id)
        .expect("应接管这个未完成会话");
    assert_eq!(target.state, RunState::Running, "存活进程应标记为 Running");
    assert_eq!(target.pid, pid);
    assert_eq!(target.last_exit, None);

    // 必须补写了 status(running) 事件，且没有伪造 exit
    let events = read_all_events(&dir);
    assert!(
        events.iter().any(|e| matches!(
            e,
            SessionEvent::Status {
                state: RunState::Running,
                ..
            }
        )),
        "应补写 status(running)"
    );
    assert!(last_exit(&events).is_none(), "运行中的会话不应有 exit 事件");

    // 收尾：把这个进程停掉，避免测试结束后残留
    manager.stop(recovered_id).await.expect("停止恢复的会话失败");
    manager
        .stop(session.session_id)
        .await
        .expect("停止原会话失败");
}
