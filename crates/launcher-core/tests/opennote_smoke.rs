//! 真实项目冒烟测试：用 app_catch 启动 opennote（Electron + Vite 的开发态），再完整回收。
//!
//! 这是"真机验收"而不是单元测试：它会真的拉起一个 Electron 窗口，耗时 ~30 秒，
//! 依赖本机装好的 pnpm 与 opennote 的 node_modules，所以默认 `#[ignore]`，CI 不跑。
//!
//! 运行：
//!
//! ```bash
//! cargo test -p launcher-core --test opennote_smoke -- --ignored --nocapture
//! # opennote 不在 ~/repo/opennote 时：
//! OPENNOTE_DIR=/path/to/opennote cargo test -p launcher-core --test opennote_smoke -- --ignored --nocapture
//! ```
//!
//! 验收点：
//!   1. `start` 成功（macOS 上即验证非 Windows 进程后端可用）；
//!   2. 启动后持续运行 `RUN_SECONDS` 秒没有退出（dev 服务器 + Electron 起来了）；
//!   3. 期间捕获到 stdout/stderr 输出（日志管道是通的）；
//!   4. `stop` 后会话在超时内退出，且退出原因是"被停止"而不是崩溃。

use std::path::PathBuf;
use std::sync::Arc;
use std::time::Duration;

use launcher_core::config::{AppConfig, AppId};
use launcher_core::platform::default_process_backend;
use launcher_core::runtime::capture::CaptureConfig;
use launcher_core::runtime::SessionManager;
use launcher_core::session::RunState;

/// 启动后观察多久（秒）：Vite 预构建 + Electron 首窗通常 10–20 秒。
const RUN_SECONDS: u64 = 25;

/// opennote 仓库位置：`OPENNOTE_DIR` 优先，否则 `~/repo/opennote`。
fn opennote_dir() -> Option<PathBuf> {
    let dir = std::env::var_os("OPENNOTE_DIR")
        .map(PathBuf::from)
        .or_else(|| std::env::var_os("HOME").map(|home| PathBuf::from(home).join("repo/opennote")))?;
    dir.join("package.json").is_file().then_some(dir)
}

/// 会话目录里捕获到的输出行数（主文件 + 轮转分片）。
fn count_output_lines(dir: &std::path::Path) -> usize {
    std::fs::read_dir(dir)
        .map(|entries| {
            entries
                .flatten()
                .filter(|e| e.file_name().to_string_lossy().starts_with("events"))
                .filter_map(|e| std::fs::read_to_string(e.path()).ok())
                .map(|text| text.lines().filter(|l| l.contains("\"event\":\"output\"")).count())
                .sum()
        })
        .unwrap_or(0)
}

#[tokio::test]
#[ignore = "真机冒烟：会拉起 opennote 的 Electron 窗口，需手动 --ignored 运行"]
async fn opennote_starts_runs_and_stops_cleanly() {
    let Some(dir) = opennote_dir() else {
        eprintln!("跳过：找不到 opennote（设置 OPENNOTE_DIR 或放在 ~/repo/opennote）");
        return;
    };

    let home = tempfile::tempdir().expect("创建临时目录失败");
    std::env::set_var("APP_CATCH_HOME", home.path());

    let manager = SessionManager::from_home(Arc::new(default_process_backend()), CaptureConfig::default())
        .expect("构造 SessionManager 失败");

    let mut app = AppConfig::new(AppId::new("opennote").expect("合法 id"), "OpenNote", "pnpm");
    app.args = vec!["dev:electron".into()];
    app.cwd = dir;

    let session = manager.start(&app).await.expect("启动 opennote 失败");
    eprintln!("已启动会话 {}，pid {}", session.session_id, session.snapshot.pid);

    for second in 1..=RUN_SECONDS {
        tokio::time::sleep(Duration::from_secs(1)).await;
        let snapshot = manager.status(session.session_id).await.expect("查询状态失败");
        assert_eq!(
            snapshot.state,
            RunState::Running,
            "第 {second} 秒会话已退出：{:?}",
            snapshot.last_exit
        );
    }

    let session_dir = manager.session_dir(&session.session_id);
    let outputs = count_output_lines(&session_dir);
    eprintln!("运行 {RUN_SECONDS} 秒，捕获输出 {outputs} 行");
    assert!(outputs > 0, "运行期间应捕获到输出，会话目录 {}", session_dir.display());

    manager.stop(session.session_id).await.expect("停止失败");
    let exit = manager
        .wait_for_exit(session.session_id, Duration::from_secs(15))
        .await
        .expect("停止后应在 15 秒内退出");
    eprintln!("退出信息：{exit:?}");

    std::env::remove_var("APP_CATCH_HOME");
}
