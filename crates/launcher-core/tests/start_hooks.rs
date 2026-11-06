//! 启动 hook 集成测试：真实拉起 `/bin/sh`，验证 before / after 的执行时机、日志与失败语义。
//!
//! 不依赖 `APP_CATCH_HOME`：会话根目录直接指向临时目录，因此可以并行执行。
//! 仅 Unix：Windows 后端的 hook 走同一条 `ProcessBackend::spawn` 路径（`cmd /D /S /C`）。
#![cfg(unix)]

use std::sync::Arc;
use std::time::Duration;

use launcher_core::config::{AppConfig, AppHooks, AppId};
use launcher_core::error::AppCatchError;
use launcher_core::platform::default_process_backend;
use launcher_core::runtime::capture::CaptureConfig;
use launcher_core::runtime::SessionManager;
use launcher_core::session::SessionEvent;

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

fn sh_app(cwd: &std::path::Path, script: &str, hooks: AppHooks) -> AppConfig {
    let mut app = AppConfig::new(
        AppId::new("hook-demo").expect("合法 id"),
        "hook 演示",
        "/bin/sh",
    );
    app.args = vec!["-c".to_string(), script.to_string()];
    app.cwd = cwd.to_path_buf();
    app.hooks = hooks;
    app
}

fn read_events(dir: &std::path::Path) -> Vec<SessionEvent> {
    let text = std::fs::read_to_string(dir.join("events.jsonl")).expect("读日志失败");
    text.lines()
        .filter(|l| !l.trim().is_empty())
        .map(|l| SessionEvent::from_jsonl_line(l).expect("日志行应可解析"))
        .collect()
}

fn output_lines(events: &[SessionEvent]) -> Vec<String> {
    events
        .iter()
        .filter_map(|e| match e {
            SessionEvent::Output { line, .. } => Some(line.clone()),
            _ => None,
        })
        .collect()
}

#[tokio::test]
async fn before_hook_runs_before_main_process_and_is_logged_after_start() {
    let dir = tempfile::tempdir().expect("临时目录");
    let manager = manager(dir.path());
    let app = sh_app(
        dir.path(),
        "cat marker.txt",
        AppHooks {
            before: vec![
                "echo \"pulled-$APP_CATCH_APP_ID-$APP_CATCH_HOOK_STAGE\" > marker.txt".to_string(),
                "echo hook-says-hi".to_string(),
            ],
            ..AppHooks::default()
        },
    );

    let handle = manager.start(&app).await.expect("启动应成功");
    manager
        .wait_for_exit(handle.session_id, Duration::from_secs(10))
        .await
        .expect("主进程应退出");

    let events = read_events(&manager.session_dir(&handle.session_id));
    assert_eq!(events[0].event_name(), "start", "start 必须仍是首行");
    let lines = output_lines(&events);
    let hook_pos = lines
        .iter()
        .position(|l| l == "[before] hook-says-hi")
        .unwrap_or_else(|| panic!("应记录 before hook 输出: {lines:?}"));
    let main_pos = lines
        .iter()
        .position(|l| l == "pulled-hook-demo-before")
        .unwrap_or_else(|| panic!("主进程应读到 before hook 写的文件: {lines:?}"));
    assert!(
        hook_pos < main_pos,
        "before 输出应排在主进程输出之前: {lines:?}"
    );
    assert!(lines
        .iter()
        .any(|l| l.starts_with("[before] 完成（退出码 0")));
}

#[tokio::test]
async fn failing_before_hook_aborts_start_without_session_dir() {
    let dir = tempfile::tempdir().expect("临时目录");
    let manager = manager(dir.path());
    let app = sh_app(
        dir.path(),
        "touch main-ran.txt",
        AppHooks {
            before: vec![
                "echo boom >&2; exit 3".to_string(),
                "touch second-hook-ran.txt".to_string(),
            ],
            ..AppHooks::default()
        },
    );

    let err = manager
        .start(&app)
        .await
        .expect_err("before 失败应中止启动");
    match &err {
        AppCatchError::HookFailed(msg) => {
            assert!(msg.contains("退出码 3"), "应带退出码: {msg}");
            assert!(msg.contains("boom"), "应带输出尾部: {msg}");
        }
        other => panic!("应为 HookFailed，实际: {other:?}"),
    }
    assert!(!dir.path().join("main-ran.txt").exists(), "主进程不应启动");
    assert!(
        !dir.path().join("second-hook-ran.txt").exists(),
        "后续 hook 不应执行"
    );
    let sessions = dir.path().join("sessions");
    let count = std::fs::read_dir(&sessions).map(|d| d.count()).unwrap_or(0);
    assert_eq!(count, 0, "失败时不应留下会话目录");
}

#[tokio::test]
async fn before_hook_timeout_is_a_failure() {
    let dir = tempfile::tempdir().expect("临时目录");
    let manager = manager(dir.path());
    let app = sh_app(
        dir.path(),
        "true",
        AppHooks {
            before: vec!["sleep 30".to_string()],
            timeout_secs: Some(1),
            ..AppHooks::default()
        },
    );
    let started = std::time::Instant::now();
    let err = manager.start(&app).await.expect_err("超时应失败");
    assert!(
        matches!(&err, AppCatchError::HookFailed(m) if m.contains("超时")),
        "{err:?}"
    );
    assert!(
        started.elapsed() < Duration::from_secs(10),
        "超时后应及时返回"
    );
}

#[tokio::test]
async fn after_hook_runs_once_main_process_started_and_failure_keeps_it_running() {
    let dir = tempfile::tempdir().expect("临时目录");
    let manager = manager(dir.path());
    let app = sh_app(
        dir.path(),
        "sleep 30",
        AppHooks {
            after: vec![
                "echo \"$APP_CATCH_PID\" > after-pid.txt; echo after-ok".to_string(),
                "exit 7".to_string(),
                "touch third-after-ran.txt".to_string(),
            ],
            ..AppHooks::default()
        },
    );

    let handle = manager.start(&app).await.expect("启动应成功");
    let pid_file = dir.path().join("after-pid.txt");
    let session_dir = manager.session_dir(&handle.session_id);
    let deadline = std::time::Instant::now() + Duration::from_secs(10);
    loop {
        let done = std::fs::read_to_string(session_dir.join("events.jsonl"))
            .map(|t| t.contains("[after] 失败（退出码 7"))
            .unwrap_or(false);
        if done {
            break;
        }
        assert!(
            std::time::Instant::now() < deadline,
            "after hook 应在 10 秒内跑完"
        );
        tokio::time::sleep(Duration::from_millis(50)).await;
    }

    let pid = std::fs::read_to_string(&pid_file).expect("after hook 应写 pid 文件");
    assert_eq!(
        pid.trim(),
        handle.snapshot.pid.to_string(),
        "应注入主进程 pid"
    );
    assert!(
        !dir.path().join("third-after-ran.txt").exists(),
        "失败后不再执行后续 after hook"
    );

    let status = manager.status(handle.session_id).await.expect("会话应存在");
    assert_eq!(
        status.state,
        launcher_core::RunState::Running,
        "after 失败不影响主进程"
    );

    manager.stop(handle.session_id).await.expect("停止失败");
    manager
        .wait_for_exit(handle.session_id, Duration::from_secs(10))
        .await
        .expect("应退出");
    let lines = output_lines(&read_events(&session_dir));
    assert!(lines.iter().any(|l| l == "[after] after-ok"), "{lines:?}");
}
