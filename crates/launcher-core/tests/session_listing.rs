//! 会话列表、幂等停止与变化通知的集成测试（真实拉起 shell，跨平台）。
//!
//! 覆盖：
//! - `list_sessions` 在进程退出后仍保留会话，并按应用限制已退出会话的条数；
//! - `list_active` / `list_sessions` 按启动先后排序；
//! - 对已退出会话调用 `stop` 是幂等的，不改写退出原因；
//! - `subscribe` 能收到启动 / 退出的 `Changed` 与输出落盘后的 `LogAppended`，
//!   且收到 `LogAppended` 时日志文件里已经能读到对应输出。
//!
//! 不依赖 `APP_CATCH_HOME`：会话根目录直接指向临时目录，因此可以并行执行。

use std::sync::Arc;
use std::time::Duration;

use launcher_core::config::{AppConfig, AppId};
use launcher_core::platform::default_process_backend;
use launcher_core::runtime::{CaptureConfig, SessionManager, SessionNotice, RECENT_EXITED_PER_APP};
use launcher_core::session::{ExitReason, RunState};

const WAIT: Duration = Duration::from_secs(20);

fn manager(root: &std::path::Path) -> SessionManager {
    let capture = CaptureConfig {
        status_interval: None,
        ..CaptureConfig::default()
    };
    SessionManager::new(
        Arc::new(default_process_backend()),
        capture,
        root.join("sessions"),
    )
}

/// 跑一段 shell 脚本的应用（Unix: `/bin/sh -c`，Windows: `cmd /D /S /C`）。
fn shell_app(id: &str, script: &str) -> AppConfig {
    #[cfg(unix)]
    let (command, args) = ("/bin/sh", vec!["-c".to_string(), script.to_string()]);
    #[cfg(windows)]
    let (command, args) = (
        "cmd",
        vec![
            "/D".to_string(),
            "/S".to_string(),
            "/C".to_string(),
            script.to_string(),
        ],
    );
    let mut app = AppConfig::new(AppId::new(id).expect("合法 id"), id, command);
    app.args = args;
    app
}

/// 一个会持续运行到被停止的脚本。
fn long_running_script() -> &'static str {
    if cfg!(windows) {
        "ping -n 60 127.0.0.1 > nul"
    } else {
        "sleep 60"
    }
}

#[tokio::test]
async fn exited_session_stays_listed_but_not_active() {
    let dir = tempfile::tempdir().expect("临时目录");
    let manager = manager(dir.path());

    let handle = manager
        .start(&shell_app("quick", "echo bye"))
        .await
        .expect("启动失败");
    let exit = manager
        .wait_for_exit(handle.session_id, WAIT)
        .await
        .expect("应退出");
    assert_eq!(exit.reason, ExitReason::Normal);

    assert!(
        manager.list_active().await.is_empty(),
        "list_active 只含运行中的会话"
    );
    let listed = manager.list_sessions().await;
    assert_eq!(listed.len(), 1, "已退出的会话应仍在 list_sessions 里");
    assert_eq!(listed[0].session_id, handle.session_id);
    assert_eq!(listed[0].state, RunState::Exited);
    assert_eq!(
        listed[0].last_exit.map(|e| e.reason),
        Some(ExitReason::Normal)
    );
}

#[tokio::test]
async fn list_sessions_keeps_only_recent_exited_per_app_in_start_order() {
    let dir = tempfile::tempdir().expect("临时目录");
    let manager = manager(dir.path());

    let mut started = Vec::new();
    for _ in 0..RECENT_EXITED_PER_APP + 2 {
        let handle = manager
            .start(&shell_app("burst", "echo hi"))
            .await
            .expect("启动失败");
        manager
            .wait_for_exit(handle.session_id, WAIT)
            .await
            .expect("应退出");
        started.push(handle.session_id);
    }
    // 另一个应用的已退出会话单独计数，不受 burst 的上限影响
    let other = manager
        .start(&shell_app("other", "echo hi"))
        .await
        .expect("启动失败");
    manager
        .wait_for_exit(other.session_id, WAIT)
        .await
        .expect("应退出");

    let listed: Vec<_> = manager
        .list_sessions()
        .await
        .into_iter()
        .map(|s| s.session_id)
        .collect();
    let mut expected: Vec<_> = started[started.len() - RECENT_EXITED_PER_APP..].to_vec();
    expected.push(other.session_id);
    assert_eq!(
        listed, expected,
        "应只保留每个应用最近的已退出会话，且按启动先后排序"
    );
}

#[tokio::test]
async fn list_active_is_in_start_order() {
    let dir = tempfile::tempdir().expect("临时目录");
    let manager = manager(dir.path());

    let mut started = Vec::new();
    for _ in 0..3 {
        let handle = manager
            .start(&shell_app("multi", long_running_script()))
            .await
            .expect("启动失败");
        started.push(handle.session_id);
    }

    let active: Vec<_> = manager
        .list_active()
        .await
        .into_iter()
        .map(|s| s.session_id)
        .collect();
    assert_eq!(active, started, "最后一个应是最近启动的会话");

    for id in started {
        manager.stop(id).await.expect("停止失败");
    }
}

#[tokio::test]
async fn stopping_an_exited_session_is_a_no_op() {
    let dir = tempfile::tempdir().expect("临时目录");
    let manager = manager(dir.path());

    let handle = manager
        .start(&shell_app("done", "exit 3"))
        .await
        .expect("启动失败");
    manager
        .wait_for_exit(handle.session_id, WAIT)
        .await
        .expect("应退出");

    manager
        .stop(handle.session_id)
        .await
        .expect("停止已退出的会话应直接成功");
    let snapshot = manager.status(handle.session_id).await.expect("仍可查询");
    assert_eq!(snapshot.state, RunState::Crashed, "退出原因不应被改写");
    assert_eq!(snapshot.last_exit.and_then(|e| e.code), Some(3));
}

#[tokio::test]
async fn subscribers_hear_start_output_and_exit() {
    let dir = tempfile::tempdir().expect("临时目录");
    let manager = manager(dir.path());
    let mut notices = manager.subscribe();

    let handle = manager
        .start(&shell_app("noisy", "echo notice-me"))
        .await
        .expect("启动失败");
    let id = handle.session_id;
    let log_path = manager.session_dir(&id).join("events.jsonl");

    let mut changed = 0;
    let mut saw_output_on_disk = false;
    let mut saw_exit_state = false;
    let deadline = tokio::time::Instant::now() + WAIT;
    while !(saw_output_on_disk && saw_exit_state) {
        let notice = tokio::time::timeout_at(deadline, notices.recv())
            .await
            .expect("等待通知超时")
            .expect("通知通道不应关闭");
        assert_eq!(notice.session_id(), id);
        match notice {
            SessionNotice::Changed(_) => {
                changed += 1;
                let snapshot = manager.status(id).await.expect("可查询");
                if snapshot.state != RunState::Running {
                    saw_exit_state = true;
                }
            }
            SessionNotice::LogAppended(_) => {
                // 通知发出时数据必须已对读者可见
                let text = std::fs::read_to_string(&log_path).expect("读日志失败");
                if text.contains("notice-me") {
                    saw_output_on_disk = true;
                }
            }
        }
    }
    assert!(changed >= 2, "启动与退出都应发 Changed，实际 {changed} 次");
}
