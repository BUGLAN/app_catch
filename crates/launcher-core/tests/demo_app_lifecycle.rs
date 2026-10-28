//! Unix 进程后端的端到端测试：用自带的小应用（tests/fixtures/demo_app.sh）走完整生命周期。
//!
//! 小应用模拟一个典型的开发服务器进程树：组长 + 组内 worker + 一个脱离进程组的孙进程，
//! 并同时往 stdout / stderr 输出。验收点：
//!   1. `start` 成功，且应用持续运行（不是一闪而过）；
//!   2. stdout 与 stderr 都被捕获进会话日志；
//!   3. `stop` 后会话在超时内退出，退出原因是 Killed；
//!   4. 组内 worker 与脱组孙进程都被回收，没有孤儿。
//!
//! Windows 的同类覆盖见 runtime_sessions.rs（Job Object 路径）。
#![cfg(unix)]

use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::Duration;

use launcher_core::config::{AppConfig, AppId};
use launcher_core::platform::default_process_backend;
use launcher_core::runtime::capture::CaptureConfig;
use launcher_core::runtime::SessionManager;
use launcher_core::session::{ExitReason, RunState};

fn fixture() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures/demo_app.sh")
}

/// 会话日志里所有输出行（主文件 + 轮转分片）。
fn output_lines(dir: &Path) -> Vec<String> {
    let Ok(entries) = std::fs::read_dir(dir) else {
        return Vec::new();
    };
    entries
        .flatten()
        .filter(|e| e.file_name().to_string_lossy().starts_with("events"))
        .filter_map(|e| std::fs::read_to_string(e.path()).ok())
        .flat_map(|text| text.lines().map(str::to_owned).collect::<Vec<_>>())
        .filter(|line| line.contains("\"event\":\"output\""))
        .collect()
}

/// 读 pid 文件；子进程可能还没写完，最多等 3 秒。
async fn read_pid(path: &Path) -> u32 {
    for _ in 0..60 {
        if let Some(pid) = std::fs::read_to_string(path)
            .ok()
            .and_then(|s| s.trim().parse().ok())
        {
            return pid;
        }
        tokio::time::sleep(Duration::from_millis(50)).await;
    }
    panic!("等不到 pid 文件 {}", path.display());
}

/// `kill -0`：进程是否还存在（不发真实信号）。
fn alive(pid: u32) -> bool {
    std::process::Command::new("kill")
        .args(["-0", &pid.to_string()])
        .stderr(std::process::Stdio::null())
        .status()
        .map(|s| s.success())
        .unwrap_or(false)
}

#[tokio::test]
async fn demo_app_runs_logs_and_stops_without_orphans() {
    let home = tempfile::tempdir().expect("创建临时目录失败");
    let pids = tempfile::tempdir().expect("创建 pid 目录失败");
    std::env::set_var("APP_CATCH_HOME", home.path());

    let manager = SessionManager::from_home(
        Arc::new(default_process_backend()),
        CaptureConfig::default(),
    )
    .expect("构造 SessionManager 失败");

    let mut app = AppConfig::new(AppId::new("demo-app").expect("合法 id"), "Demo App", "sh");
    app.args = vec![
        fixture().display().to_string(),
        pids.path().display().to_string(),
    ];

    let session = manager.start(&app).await.expect("启动 demo 应用失败");
    let worker = read_pid(&pids.path().join("worker.pid")).await;
    let detached = read_pid(&pids.path().join("detached.pid")).await;

    // 持续运行：1.5 秒后仍是 Running
    tokio::time::sleep(Duration::from_millis(1500)).await;
    let snapshot = manager
        .status(session.session_id)
        .await
        .expect("查询状态失败");
    assert_eq!(
        snapshot.state,
        RunState::Running,
        "应用不应自行退出: {:?}",
        snapshot.last_exit
    );
    assert!(alive(worker) && alive(detached), "子孙进程应在运行");

    let lines = output_lines(&manager.session_dir(&session.session_id));
    assert!(
        lines.iter().any(|l| l.contains("demo-app 就绪")),
        "应捕获 stdout: {lines:?}"
    );
    assert!(
        lines.iter().any(|l| l.contains("走 stderr")),
        "应捕获 stderr: {lines:?}"
    );
    assert!(
        lines.iter().any(|l| l.contains("worker 心跳")),
        "应捕获子进程输出: {lines:?}"
    );

    let stop_started = std::time::Instant::now();
    manager.stop(session.session_id).await.expect("停止失败");
    let stop_took = stop_started.elapsed();
    eprintln!("stop 耗时 {stop_took:?}");
    // demo 应用会响应 SIGTERM：必须在 SIGTERM 宽限期（5 秒）内结束，而不是靠 SIGKILL 兜底
    assert!(
        stop_took < Duration::from_secs(4),
        "应在 SIGTERM 阶段优雅退出，实际 {stop_took:?}"
    );
    let exit = manager
        .wait_for_exit(session.session_id, Duration::from_secs(15))
        .await
        .expect("停止后应在 15 秒内退出");
    assert_eq!(
        exit.reason,
        ExitReason::Killed,
        "退出原因应为被停止: {exit:?}"
    );

    assert!(!alive(worker), "组内 worker {worker} 应已被回收");
    assert!(!alive(detached), "脱组孙进程 {detached} 应已被回收");

    std::env::remove_var("APP_CATCH_HOME");
}
