//! macOS `.app` 启动的集成测试（真实进程）。
//!
//! - 直接运行：启动命令写 `Foo.app`，后端解析成 `Contents/MacOS/<CFBundleExecutable>` 再 spawn，
//!   输出照常捕获；很快以 0 退出时日志里有「可能交给了已有实例」的提示；
//! - 像 Dock 一样打开（`launch_mode = "open"`）：交给 LaunchServices，按 pid 跟踪，
//!   停止能结束进程；应用已在运行时第二个会话跟踪同一个进程。
//!   这两条会真的经过 LaunchServices（需要图形会话），默认 `#[ignore]`，手动运行：
//!   `cargo test -p launcher-core --test macos_app_bundle -- --ignored`
#![cfg(target_os = "macos")]

use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::Duration;

use launcher_core::config::{AppConfig, AppId, LaunchMode};
use launcher_core::platform::default_process_backend;
use launcher_core::runtime::{CaptureConfig, SessionManager, APP_CATCH_NOTE_PREFIX};
use launcher_core::session::{ExitReason, RunState, SessionEvent};

const WAIT: Duration = Duration::from_secs(20);

fn manager(root: &Path) -> SessionManager {
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

/// 造一个 `.app` 包：`executable_src` 复制成 `Contents/MacOS/<exec_name>`。
fn make_bundle(root: &Path, name: &str, exec_name: &str, executable_src: &Path) -> PathBuf {
    let bundle = root.join(format!("{name}.app"));
    let macos = bundle.join("Contents/MacOS");
    std::fs::create_dir_all(&macos).expect("建包目录失败");
    std::fs::copy(executable_src, macos.join(exec_name)).expect("复制可执行文件失败");
    let id = format!("com.appcatch.test.{}", uuid_like());
    std::fs::write(
        bundle.join("Contents/Info.plist"),
        format!(
            r#"<?xml version="1.0" encoding="UTF-8"?>
<!DOCTYPE plist PUBLIC "-//Apple//DTD PLIST 1.0//EN" "http://www.apple.com/DTDs/PropertyList-1.0.dtd">
<plist version="1.0"><dict>
<key>CFBundleExecutable</key><string>{exec_name}</string>
<key>CFBundleIdentifier</key><string>{id}</string>
<key>CFBundleName</key><string>{name}</string>
<key>CFBundlePackageType</key><string>APPL</string>
<key>LSUIElement</key><true/>
</dict></plist>"#
        ),
    )
    .expect("写 Info.plist 失败");
    // 复制来的系统二进制带着 Apple 的签名与原标识，launchd 拒绝把它当成这个包拉起
    // （POSIX 162）；像本地构建一样整体 ad-hoc 重签即可
    let signed = std::process::Command::new("/usr/bin/codesign")
        .args(["--force", "--sign", "-"])
        .arg(&bundle)
        .output()
        .expect("运行 codesign 失败");
    assert!(
        signed.status.success(),
        "ad-hoc 签名失败: {}",
        String::from_utf8_lossy(&signed.stderr)
    );
    bundle
}

/// 不引入 uuid 依赖：用纳秒时间戳 + pid 拼一个唯一后缀。
fn uuid_like() -> String {
    let nanos = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_nanos())
        .unwrap_or(0);
    format!("p{}n{nanos}", std::process::id())
}

/// 读会话日志里的全部 output 行。
fn output_lines(
    manager: &SessionManager,
    session: &launcher_core::session::SessionId,
) -> Vec<String> {
    let path = manager.session_dir(session).join("events.jsonl");
    std::fs::read_to_string(&path)
        .unwrap_or_default()
        .lines()
        .filter_map(|line| serde_json::from_str::<SessionEvent>(line).ok())
        .filter_map(|event| match event {
            SessionEvent::Output { line, .. } => Some(line),
            _ => None,
        })
        .collect()
}

fn alive(pid: u32) -> bool {
    std::process::Command::new("/bin/kill")
        .args(["-0", &pid.to_string()])
        .status()
        .map(|s| s.success())
        .unwrap_or(false)
}

#[tokio::test]
async fn direct_launch_resolves_bundle_executable_and_hints_quick_exit() {
    let dir = tempfile::tempdir().expect("临时目录");
    let script = dir.path().join("greet.sh");
    std::fs::write(&script, "#!/bin/sh\necho \"hello from bundle $1\"\n").expect("写脚本");
    std::process::Command::new("/bin/chmod")
        .args(["+x"])
        .arg(&script)
        .status()
        .expect("chmod");
    let bundle = make_bundle(dir.path(), "Greeter", "greeter", &script);

    let manager = manager(dir.path());
    let mut app = AppConfig::new(
        AppId::new("greeter").expect("id"),
        "Greeter",
        bundle.to_string_lossy(),
    );
    app.args = vec!["x".to_string()];
    let handle = manager.start(&app).await.expect("直接运行 .app 应能启动");
    let exit = manager
        .wait_for_exit(handle.session_id, WAIT)
        .await
        .expect("应退出");
    assert_eq!(exit.reason, ExitReason::Normal);

    let lines = output_lines(&manager, &handle.session_id);
    assert!(
        lines.iter().any(|l| l == "hello from bundle x"),
        "应捕获包内可执行文件的输出: {lines:?}"
    );
    assert!(
        lines
            .iter()
            .any(|l| l.starts_with(APP_CATCH_NOTE_PREFIX) && l.contains("已有实例")),
        "很快以 0 退出时应提示可能交给了已有实例: {lines:?}"
    );
}

#[tokio::test]
async fn open_mode_rejects_non_bundle_command() {
    let dir = tempfile::tempdir().expect("临时目录");
    let manager = manager(dir.path());
    let mut app = AppConfig::new(AppId::new("not-bundle").expect("id"), "x", "/bin/sleep");
    app.launch_mode = LaunchMode::Open;
    let err = manager
        .start(&app)
        .await
        .expect_err("非 .app 不能用系统打开");
    assert!(err.to_string().contains(".app"), "错误信息: {err}");
}

#[tokio::test]
#[ignore = "经过 LaunchServices，需要图形会话；手动运行 --ignored"]
async fn open_mode_tracks_launched_app_and_reuses_running_instance() {
    let dir = tempfile::tempdir().expect("临时目录");
    let bundle = make_bundle(dir.path(), "Sleeper", "sleeper", Path::new("/bin/sleep"));
    let executable = bundle.join("Contents/MacOS/sleeper");

    let manager = manager(dir.path());
    let mut app = AppConfig::new(
        AppId::new("sleeper").expect("id"),
        "Sleeper",
        bundle.to_string_lossy(),
    );
    app.args = vec!["60".to_string()];
    app.launch_mode = LaunchMode::Open;

    let first = manager.start(&app).await.expect("系统打开应成功");
    let pid = first.handle.pid;
    assert!(alive(pid), "打开后应能跟踪到存活进程");
    let ps = std::process::Command::new("/bin/ps")
        .args(["-o", "comm=", "-p", &pid.to_string()])
        .output()
        .expect("ps");
    // 临时目录在 /var 下，而 /var 是指向 /private/var 的链接：ps 报的是真实路径
    assert_eq!(
        PathBuf::from(String::from_utf8_lossy(&ps.stdout).trim()),
        std::fs::canonicalize(&executable).expect("规范化路径"),
        "跟踪的应是包内的可执行文件"
    );
    let lines = output_lines(&manager, &first.session_id);
    assert!(
        lines.iter().any(|l| l.contains("已交给系统打开")),
        "日志应说明启动方式: {lines:?}"
    );

    // 已在运行时再打开：跟踪同一个进程，并说明这一点
    let second = manager.start(&app).await.expect("再次打开应成功");
    assert_eq!(second.handle.pid, pid, "已在运行时应跟踪已有进程");
    let lines = output_lines(&manager, &second.session_id);
    assert!(
        lines.iter().any(|l| l.contains("已在运行")),
        "应说明跟踪的是已有进程: {lines:?}"
    );

    manager.stop(first.session_id).await.expect("停止应成功");
    let exit = manager
        .wait_for_exit(first.session_id, WAIT)
        .await
        .expect("应退出");
    assert_eq!(exit.reason, ExitReason::Killed);
    assert!(!alive(pid), "停止后进程应已结束");

    // 另一个会话跟踪的同一进程也随之结束；外部结束且拿不到退出码时按正常退出归类
    let other = manager
        .wait_for_exit(second.session_id, WAIT)
        .await
        .expect("第二个会话也应结束");
    assert_eq!(other.reason, ExitReason::Normal);
    let snapshot = manager.status(second.session_id).await.expect("状态");
    assert_eq!(snapshot.state, RunState::Exited);
}
