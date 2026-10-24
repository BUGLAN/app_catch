//! 手动验收示例：真实拉起一个子进程，把整个会话的产物打出来。
//!
//! 用法（默认 `cmd /c echo hello`）：
//!
//! ```bash
//! cargo run -p launcher-core --example session_demo
//! cargo run -p launcher-core --example session_demo -- cmd /c "echo A & echo B & exit /b 3"
//! ```
//!
//! 输出内容：会话目录、事件条数、JSONL 尾部若干行，便于人工确认完整性。
//! 示例使用独立的临时 `APP_CATCH_HOME`，不会污染真实的 `~/.app_catch`。

use std::sync::Arc;

use launcher_core::config::{AppConfig, AppId};
use launcher_core::platform::default_process_backend;
use launcher_core::runtime::{CaptureConfig, SessionManager};
use launcher_core::session::SessionEvent;

#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    // 1) 命令行参数：第一个是程序，其余是参数；缺省用 cmd /c echo hello
    let args: Vec<String> = std::env::args().skip(1).collect();
    let (program, rest) = match args.split_first() {
        Some((program, rest)) => (program.clone(), rest.to_vec()),
        None => (
            "cmd".to_string(),
            vec!["/c".to_string(), "echo hello".to_string()],
        ),
    };

    // 2) 隔离运行目录：APP_CATCH_HOME 指向系统临时目录下的独立子目录
    let home = std::env::temp_dir().join(format!("app-catch-demo-{}", std::process::id()));
    std::fs::create_dir_all(&home)?;
    std::env::set_var("APP_CATCH_HOME", &home);

    println!("== app_catch 会话演示 ==");
    println!("APP_CATCH_HOME = {}", home.display());
    println!("将要执行: {program} {rest:?}");
    println!();

    // 3) 构造应用配置并启动会话
    let app = AppConfig::new(AppId::new("demo")?, "演示应用", program.clone());
    let mut app = app;
    app.args = rest.clone();

    // 可选：用环境变量把轮转上限压小，便于人工观察分片行为
    //   APP_CATCH_DEMO_MAX_BYTES=4096 APP_CATCH_DEMO_MAX_SHARDS=2
    let mut capture = CaptureConfig::default();
    if let Ok(raw) = std::env::var("APP_CATCH_DEMO_MAX_BYTES") {
        capture.writer.max_file_bytes = raw.parse().unwrap_or(capture.writer.max_file_bytes);
    }
    if let Ok(raw) = std::env::var("APP_CATCH_DEMO_MAX_SHARDS") {
        capture.writer.max_shards = raw.parse().unwrap_or(capture.writer.max_shards);
    }

    let manager = SessionManager::from_home(Arc::new(default_process_backend()), capture)?;

    let session = manager.start(&app).await?;
    println!("会话 id    = {}", session.session_id);
    println!("子进程 pid = {}", session.handle.pid);
    println!("启动时间   = {}", session.handle.start_time);
    println!("会话目录   = {}", manager.session_dir(&session.session_id).display());
    println!();

    // 3.5) 可选：等若干毫秒后主动 stop，用于人工验证杀树
    //      例：APP_CATCH_DEMO_STOP_AFTER_MS=3000
    if let Ok(raw) = std::env::var("APP_CATCH_DEMO_STOP_AFTER_MS") {
        if let Ok(ms) = raw.parse::<u64>() {
            tokio::time::sleep(std::time::Duration::from_millis(ms)).await;
            println!("主动 stop（等待 {ms}ms 之后）...");
            manager.stop(session.session_id).await?;
            println!("stop 返回，进程树应已回收");
        }
    }

    // 4) 等进程退出（演示用 30s 上限，足够任何短命命令）
    let exit = manager
        .wait_for_exit(session.session_id, std::time::Duration::from_secs(30))
        .await?;
    println!("退出: code={:?} reason={}", exit.code, exit.reason.as_str());
    println!();

    // 5) 读回产物：事件条数与 JSONL 尾部
    let dir = manager.session_dir(&session.session_id);
    let log_path = dir.join("events.jsonl");
    // 进程退出的收尾是后台任务，稍等它把 exit 事件写进去
    let mut text = String::new();
    for _ in 0..100 {
        text = std::fs::read_to_string(&log_path).unwrap_or_default();
        if text.contains("\"event\":\"exit\"") {
            break;
        }
        tokio::time::sleep(std::time::Duration::from_millis(50)).await;
    }

    let lines: Vec<&str> = text.lines().filter(|l| !l.trim().is_empty()).collect();
    let mut outputs = 0usize;
    let mut parse_errors = 0usize;
    for line in &lines {
        match SessionEvent::from_jsonl_line(line) {
            Ok(SessionEvent::Output { .. }) => outputs += 1,
            Ok(_) => {}
            Err(_) => parse_errors += 1,
        }
    }

    println!("事件总数 = {}（其中 output {} 条）", lines.len(), outputs);
    println!("不可解析行 = {parse_errors}");
    // 轮转分片也一并展示
    let mut shards: Vec<String> = std::fs::read_dir(&dir)?
        .filter_map(|e| e.ok())
        .map(|e| e.file_name().to_string_lossy().into_owned())
        .filter(|n| n.starts_with("events."))
        .collect();
    shards.sort();
    println!("会话目录文件 = {shards:?}");
    println!();
    println!("---- JSONL 尾部（最多 8 行）----");
    let tail = lines.len().saturating_sub(8);
    for line in &lines[tail..] {
        println!("{line}");
    }

    Ok(())
}
