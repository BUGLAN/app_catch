//! 真实项目端到端验收工具（M5）。
//!
//! 与 `session_demo` 的区别：session_demo 演示「能起进程、能写 JSONL」，
//! 本示例面向**验收取证**——把一次真实启动的全过程变成可复核的证据：
//!
//! 1. 读取一份真实 `AppConfig`（TOML，由调用方给出，位于本示例独立 APP_CATCH_HOME 下）；
//! 2. 用 `SessionManager` 启动它，按秒轮询，直到拿到期望的状态或超时；
//! 3. 尽力抓一次运行时图标（GUI 类应用才有），打印尺寸与字节数；
//! 4. `stop` 回收整棵进程树，再用系统进程表复查「是否真的没了」；
//! 5. 复读会话目录里的全部 JSONL（含轮转分片），统计四类事件并打印样本行。
//!
//! 用法（bash，路径按需替换）：
//!
//! ```bash
//! export APP_CATCH_HOME="E:/repo/app_catch/.e2e/home-python"
//! cargo run -p launcher-core --example e2e_session -- \
//!     --app-id trendradar-mcp \
//!     --wait-seconds 20 \
//!     --expect-ports 3333
//! ```
//!
//! 参数：
//! - `--app-id <id>`：要启动的应用配置 id，必填（从 `<home>/apps/<id>.toml` 读取）；
//! - `--wait-seconds <n>`：启动后观察时长，默认 15 秒（给窗口/端口留出出现时间）；
//! - `--expect-ports <p1,p2>`：期望采到的监听端口，只用于打印「是否达成」，不参与断言；
//! - `--expect-window-substr <s>`：期望窗口标题包含的子串，同上；
//! - `--no-stop`：不主动停止（用于演示「进程自然退出」路径）。
//!
//! 本示例不修改任何被启动项目的文件，只读取其目录并拉起进程。

use std::collections::BTreeSet;
use std::path::{Path, PathBuf};
use std::time::Duration;

use launcher_core::config::{AppConfig, AppId, AppStore};
use launcher_core::platform::icon::load_png_file;
use launcher_core::platform::{
    collect_tree_pids, default_icon_capture, default_process_backend, pid_exists_in_snapshot,
    IconCapture, IconTarget,
};
use launcher_core::runtime::{CaptureConfig, SessionManager};
use launcher_core::session::SessionEvent;

/// 解析出来的命令行参数。
#[derive(Debug, Default)]
struct Args {
    app_id: String,
    wait_seconds: u64,
    expect_ports: Vec<u16>,
    expect_window_substr: Option<String>,
    no_stop: bool,
}

fn parse_args() -> Result<Args, String> {
    let mut args = Args {
        wait_seconds: 15,
        ..Args::default()
    };
    let mut it = std::env::args().skip(1);
    while let Some(flag) = it.next() {
        let mut value = || {
            it.next()
                .ok_or_else(|| format!("参数 {flag} 缺少取值"))
        };
        match flag.as_str() {
            "--app-id" => args.app_id = value()?,
            "--wait-seconds" => {
                args.wait_seconds = value()?
                    .parse()
                    .map_err(|e| format!("--wait-seconds 不是整数: {e}"))?
            }
            "--expect-ports" => {
                args.expect_ports = value()?
                    .split(',')
                    .filter(|s| !s.trim().is_empty())
                    .map(|s| {
                        s.trim()
                            .parse::<u16>()
                            .map_err(|e| format!("--expect-ports 端口不合法 {s}: {e}"))
                    })
                    .collect::<Result<Vec<_>, _>>()?
            }
            "--expect-window-substr" => args.expect_window_substr = Some(value()?),
            "--no-stop" => args.no_stop = true,
            other => return Err(format!("未知参数: {other}")),
        }
    }
    if args.app_id.is_empty() {
        return Err("必须提供 --app-id".to_string());
    }
    Ok(args)
}

/// 收集会话目录下的全部日志文件，按「分片 seq 升序 -> 活跃片最后」排序。
///
/// 与 `src-tauri::events_reader` 的排序规则保持一致，
/// 这样本示例读到的顺序就是界面读到的顺序。
fn ordered_log_files(dir: &Path) -> Vec<PathBuf> {
    let mut shards: Vec<(u32, PathBuf)> = Vec::new();
    let mut active: Option<PathBuf> = None;
    let Ok(entries) = std::fs::read_dir(dir) else {
        return Vec::new();
    };
    for entry in entries.flatten() {
        let name = entry.file_name().to_string_lossy().into_owned();
        if name == "events.jsonl" {
            active = Some(entry.path());
            continue;
        }
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
    shards.sort_by_key(|(seq, _)| *seq);
    let mut files: Vec<PathBuf> = shards.into_iter().map(|(_, p)| p).collect();
    if let Some(active) = active {
        files.push(active);
    }
    files
}

#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    let args = match parse_args() {
        Ok(args) => args,
        Err(message) => {
            eprintln!("参数错误: {message}");
            eprintln!(
                "用法: e2e_session --app-id <id> [--wait-seconds 15] \
                 [--expect-ports 3333] [--expect-window-substr app_catch] [--no-stop]"
            );
            std::process::exit(2);
        }
    };

    // 运行根目录必须显式给出：验收产生的配置与日志一律落在仓库内的隔离目录，
    // 绝不写真实用户主目录。
    let home = match std::env::var_os("APP_CATCH_HOME") {
        Some(raw) => PathBuf::from(raw),
        None => {
            eprintln!("请先设置 APP_CATCH_HOME（本工具只在隔离目录下运行）");
            std::process::exit(2);
        }
    };
    let home_display = home.display().to_string();

    let store = AppStore::from_home()?;
    let app_id = AppId::new(args.app_id.clone())?;
    let app: AppConfig = store.get(&app_id)?;

    println!("== app_catch 端到端验收 ==");
    println!("APP_CATCH_HOME = {home_display}");
    println!("应用配置路径   = {}", store.config_path(&app_id).display());
    println!("--- 配置 TOML ---");
    println!("{}", AppStore::to_toml(&app)?.trim_end());
    println!("--- 配置 TOML 结束 ---");
    println!();

    let manager = SessionManager::from_home(
        std::sync::Arc::new(default_process_backend()),
        CaptureConfig::default(),
    )?;

    let started = std::time::Instant::now();
    let session = manager.start(&app).await?;
    let session_id = session.session_id;
    let root_pid = session.handle.pid;
    let dir = manager.session_dir(&session_id);
    println!("会话 id    = {session_id}");
    println!("根进程 pid = {root_pid}（start_time = {}）", session.handle.start_time);
    println!("会话目录   = {}", dir.display());
    println!("观察时长   = {} 秒", args.wait_seconds);
    println!();

    // 每分钟探一次状态，把「观察窗口内的最新状态」攒下来，最后统一打印：
    // 这样即使 status 事件只在变化时写一条，证据也不会漏。
    let mut seen_ports: BTreeSet<u16> = BTreeSet::new();
    let mut seen_titles: BTreeSet<String> = BTreeSet::new();
    let deadline = started + Duration::from_secs(args.wait_seconds);
    while std::time::Instant::now() < deadline {
        tokio::time::sleep(Duration::from_millis(500)).await;
        if let Ok(snapshot) = manager.status(session_id).await {
            if let Some(status) = snapshot.latest_status {
                seen_ports.extend(status.ports.iter().copied());
                if let Some(title) = status.window_title {
                    seen_titles.insert(title);
                }
            }
            if snapshot.last_exit.is_some() {
                println!("进程在观察期内自然退出，提前结束观察");
                break;
            }
        }
        if !pid_exists_in_snapshot(root_pid) {
            println!("根进程已不在系统进程表中，提前结束观察");
            break;
        }
    }

    // 运行时图标：只有带窗口的应用能抓到；服务类应用返回 None 是预期行为。
    let tree_pids = collect_tree_pids(root_pid);
    let executable = launcher_core::platform::icon::resolve_executable(&app.command);
    let target = IconTarget::for_app(app.id.clone())
        .with_custom_icon(app.icon.clone())
        .with_executable(executable)
        .with_pid(Some(root_pid));
    let capture = default_icon_capture();
    let icon_path = capture.capture(&target);
    println!("进程树 pids = {tree_pids:?}");
    match &icon_path {
        Some(path) => {
            let bytes = std::fs::metadata(path).map(|m| m.len()).unwrap_or(0);
            match load_png_file(path) {
                Some(image) => println!(
                    "图标        = {}（{}x{}，{bytes} 字节）",
                    path.display(),
                    image.width,
                    image.height
                ),
                None => println!("图标        = {}（{bytes} 字节，PNG 解码失败）", path.display()),
            }
        }
        None => println!("图标        = 无（服务类应用无窗口，属预期）"),
    }
    println!();

    // 停止并复查：stop() 返回后必须能在进程表里确认整棵树都不在了。
    if args.no_stop {
        println!("--no-stop：跳过停止步骤");
    } else {
        let before = collect_tree_pids(root_pid);
        manager.stop(session_id).await?;
        // 给 Windows 一点时间把进程表刷新干净，再连续几次复查
        let mut remaining: Vec<u32> = Vec::new();
        for attempt in 0..10 {
            tokio::time::sleep(Duration::from_millis(200)).await;
            remaining = before
                .iter()
                .copied()
                .filter(|pid| pid_exists_in_snapshot(*pid))
                .collect();
            if remaining.is_empty() {
                println!("stop 后第 {} 次复查（间隔 200ms）：进程树已全部回收 {before:?}", attempt + 1);
                break;
            }
        }
        if !remaining.is_empty() {
            println!("!! stop 后仍有残留进程: {remaining:?}");
        }
    }

    // 等后台收尾把 exit 事件落盘
    let log_path = dir.join("events.jsonl");
    for _ in 0..100 {
        let text = std::fs::read_to_string(&log_path).unwrap_or_default();
        if text.contains("\"event\":\"exit\"") {
            break;
        }
        tokio::time::sleep(Duration::from_millis(50)).await;
    }

    // 复读全部事件（跨轮转分片）
    let files = ordered_log_files(&dir);
    let mut lines: Vec<String> = Vec::new();
    for path in &files {
        let text = std::fs::read_to_string(path).unwrap_or_default();
        lines.extend(
            text.lines()
                .filter(|l| !l.trim().is_empty())
                .map(|l| l.to_string()),
        );
    }
    let mut outputs = 0usize;
    let mut bad = 0usize;
    let mut first_output: Option<String> = None;
    let mut first_status_with_ports: Option<String> = None;
    let mut first_status_with_title: Option<String> = None;
    let mut start_line: Option<String> = None;
    let mut exit_line: Option<String> = None;
    for line in &lines {
        match SessionEvent::from_jsonl_line(line) {
            Ok(SessionEvent::Start { .. }) => {
                start_line.get_or_insert_with(|| line.clone());
            }
            Ok(SessionEvent::Output { .. }) => {
                outputs += 1;
                first_output.get_or_insert_with(|| line.clone());
            }
            Ok(SessionEvent::Status {
                ports,
                window_title,
                ..
            }) => {
                if ports.as_ref().is_some_and(|p| !p.is_empty()) {
                    first_status_with_ports.get_or_insert_with(|| line.clone());
                }
                if window_title.is_some() {
                    first_status_with_title.get_or_insert_with(|| line.clone());
                }
            }
            Ok(SessionEvent::Exit { .. }) => {
                exit_line.get_or_insert_with(|| line.clone());
            }
            Ok(SessionEvent::Overflow { .. }) => {}
            Err(_) => {
                bad += 1;
            }
        }
    }

    println!();
    println!("== 会话产物 ==");
    println!(
        "日志文件   = {:?}",
        files
            .iter()
            .map(|p| p.file_name().unwrap_or_default().to_string_lossy().into_owned())
            .collect::<Vec<_>>()
    );
    println!(
        "事件总数   = {}（output {} 条，不可解析 {} 行）",
        lines.len(),
        outputs,
        bad
    );
    println!("观察期采到端口 = {:?}", seen_ports.iter().copied().collect::<Vec<_>>());
    println!("观察期采到标题 = {:?}", seen_titles.iter().cloned().collect::<Vec<_>>());

    if !args.expect_ports.is_empty() {
        let hit: Vec<u16> = args
            .expect_ports
            .iter()
            .copied()
            .filter(|p| seen_ports.contains(p))
            .collect();
        println!(
            "期望端口 {:?} -> 命中 {:?}（{}）",
            args.expect_ports,
            hit,
            if hit.len() == args.expect_ports.len() {
                "达成"
            } else {
                "未完全达成"
            }
        );
    }
    if let Some(needle) = &args.expect_window_substr {
        let hit = seen_titles.iter().any(|t| t.contains(needle.as_str()))
            || first_status_with_title
                .as_deref()
                .is_some_and(|line| line.contains(needle.as_str()));
        println!("期望窗口标题含 {needle:?} -> {}", if hit { "达成" } else { "未达成" });
    }

    println!();
    println!("---- start 事件 ----");
    println!("{}", start_line.as_deref().unwrap_or("<缺失>"));
    println!("---- output 事件（首条）----");
    println!("{}", first_output.as_deref().unwrap_or("<缺失>"));
    println!("---- status 事件（首条含端口）----");
    println!("{}", first_status_with_ports.as_deref().unwrap_or("<缺失>"));
    println!("---- status 事件（首条含窗口标题）----");
    println!("{}", first_status_with_title.as_deref().unwrap_or("<缺失>"));
    println!("---- exit 事件 ----");
    println!("{}", exit_line.as_deref().unwrap_or("<缺失>"));

    Ok(())
}
