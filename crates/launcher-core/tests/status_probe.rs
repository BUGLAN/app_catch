//! 状态采集集成测试：窗口标题、监听端口、变化才写事件、循环退出无泄漏。
//!
//! 隔离原则与前一个集成测试一致：每个用例把 `APP_CATCH_HOME` 指向独立临时目录，
//! 并用进程内互斥锁串行化（环境变量是进程级全局状态）。
//!
//! 仅 Windows：窗口标题 / 监听端口采集只在 Windows 实现。
#![cfg(windows)]

use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::{Arc, Mutex, OnceLock};
use std::time::Duration;

use launcher_core::config::{AppConfig, AppId};
use launcher_core::platform::default_process_backend;
use launcher_core::runtime::capture::CaptureConfig;
use launcher_core::runtime::log_writer::LogWriterConfig;
use launcher_core::runtime::{
    CompositeProbe, ProbeContext, SessionManager, StatusProbe, StatusSnapshot,
};
use launcher_core::session::{ExitReason, RunState, SessionEvent};

/// 串行化全局环境变量与真实进程测试的互斥锁。
fn env_lock() -> &'static Mutex<()> {
    static LOCK: OnceLock<Mutex<()>> = OnceLock::new();
    LOCK.get_or_init(|| Mutex::new(()))
}

/// 测试用隔离环境。
struct TestEnv {
    /// 临时目录必须一直被持有，drop 即删除 —— 不要改成具名字段再读它。
    _dir: tempfile::TempDir,
    _guard: std::sync::MutexGuard<'static, ()>,
}

impl TestEnv {
    fn new() -> Self {
        let guard = env_lock().lock().unwrap_or_else(|e| e.into_inner());
        let dir = tempfile::tempdir().expect("创建临时目录失败");
        std::env::set_var("APP_CATCH_HOME", dir.path());
        Self {
            _dir: dir,
            _guard: guard,
        }
    }

    fn manager(&self, capture: CaptureConfig) -> SessionManager {
        SessionManager::from_home(Arc::new(default_process_backend()), capture)
            .expect("构造 SessionManager 失败")
    }

    fn manager_with_probe(
        &self,
        capture: CaptureConfig,
        probe: Arc<dyn StatusProbe>,
    ) -> SessionManager {
        self.manager(capture).with_status_probe(probe)
    }
}

impl Drop for TestEnv {
    fn drop(&mut self) {
        std::env::remove_var("APP_CATCH_HOME");
    }
}

/// 构造一个执行 `cmd` 的应用配置。
fn cmd_app(args: &[&str]) -> AppConfig {
    let mut app = AppConfig::new(AppId::new("test-app").expect("合法 id"), "测试应用", "cmd");
    app.args = args.iter().map(|s| s.to_string()).collect();
    app
}

/// 快节奏采集配置：100ms 采一次，便于在小段时间内观察多轮采样。
fn fast_status_capture() -> CaptureConfig {
    CaptureConfig {
        channel_capacity: 1024,
        writer: LogWriterConfig::default(),
        max_line_bytes: 1024 * 1024,
        status_interval: Some(Duration::from_millis(100)),
        max_status_events: None,
    }
}

/// 等待状态循环跑够「至少 n 轮」。
///
/// 为什么不写成固定 sleep：每一轮采集都要枚举整机进程表（本机实测
/// CreateToolhelp32Snapshot 单次约 250ms），cargo test 默认 8 个用例并行时机器会被打满，
/// 实际采样频率远低于配置的间隔。这里等的是「事实发生了」而不是「时间过去了」，
/// 阈值只要够证明「多轮采样」这个性质即可，不追求精确频率——否则测试必然是 flaky 的。
async fn wait_for_rounds(probe: &ScriptedProbe, rounds: usize, timeout_secs: u64) -> bool {
    wait_until(|| probe.calls() >= rounds, Duration::from_secs(timeout_secs)).await
}

/// 关闭状态采集的配置（对照组用）。
fn status_disabled_capture() -> CaptureConfig {
    CaptureConfig {
        status_interval: None,
        ..fast_status_capture()
    }
}

/// 读出会话目录里的全部事件（主文件 + 分片，按 seq 顺序）。
fn read_all_events(dir: &std::path::Path) -> Vec<SessionEvent> {
    let mut main_files: Vec<std::path::PathBuf> = Vec::new();
    let mut shards: Vec<(u32, std::path::PathBuf)> = Vec::new();
    for entry in std::fs::read_dir(dir).expect("读会话目录失败").flatten() {
        let name = entry.file_name().to_string_lossy().into_owned();
        if name == "events.jsonl" {
            main_files.push(entry.path());
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
    ordered.extend(main_files);

    let mut events = Vec::new();
    for path in ordered {
        let text = std::fs::read_to_string(&path).expect("读日志失败");
        for line in text.lines() {
            if line.trim().is_empty() {
                continue;
            }
            events.push(
                SessionEvent::from_jsonl_line(line)
                    .unwrap_or_else(|e| panic!("日志行不可解析（{}）: {e}\n{line}", path.display())),
            );
        }
    }
    events
}

/// 统计 status 事件数。
fn count_status(events: &[SessionEvent]) -> usize {
    events
        .iter()
        .filter(|e| matches!(e, SessionEvent::Status { .. }))
        .count()
}

/// 轮询等待条件成立。
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

// ==================== 假 probe：返回值可控 ====================

/// 返回值可变的 probe：第 n 次调用返回第 n 个预设快照（超出后重复最后一个）。
struct ScriptedProbe {
    snapshots: Vec<StatusSnapshot>,
    calls: AtomicUsize,
}

impl ScriptedProbe {
    fn fixed(title: Option<&str>, ports: Vec<u16>) -> Arc<Self> {
        Arc::new(Self {
            snapshots: vec![StatusSnapshot {
                window_title: title.map(|t| t.to_string()),
                ports,
            }],
            calls: AtomicUsize::new(0),
        })
    }

    fn scripted(snapshots: Vec<StatusSnapshot>) -> Arc<Self> {
        Arc::new(Self {
            snapshots,
            calls: AtomicUsize::new(0),
        })
    }

    fn calls(&self) -> usize {
        self.calls.load(Ordering::Relaxed)
    }
}

impl StatusProbe for ScriptedProbe {
    fn probe(&self, _ctx: &ProbeContext) -> StatusSnapshot {
        let index = self.calls.fetch_add(1, Ordering::Relaxed);
        self.snapshots
            .get(index)
            .or_else(|| self.snapshots.last())
            .cloned()
            .unwrap_or_default()
    }

    fn name(&self) -> &'static str {
        "scripted"
    }
}

// ==================== a) 端口采集：真实 TcpListener ====================

/// 用真实 TcpListener 验证端口采集：绑定后能采到，drop 后不再包含。
///
/// 这是 a) 与 b) 两条验收标准的合并用例——它们共用同一份绑定/释放逻辑，
/// 拆开会让「绑定后发现端口消失」这类失败很难定位。
#[tokio::test]
async fn status_probe_sees_bound_port_then_not_after_release() {
    use std::net::TcpListener;

    let probe = launcher_core::runtime::default_status_probe();
    let listener = TcpListener::bind("127.0.0.1:0").expect("绑定 loopback 失败");
    let port = listener.local_addr().expect("取本地地址失败").port();
    assert!(port > 0, "应拿到真实端口");

    let ctx = ProbeContext::with_session(std::process::id(), "port-test");

    // 绑定期间：必须采到
    let bound = probe.probe(&ctx);
    println!("绑定 {port} 后采集到的端口: {:?}", bound.ports);
    assert!(
        bound.ports.contains(&port),
        "应采到刚绑定的端口 {port}，实际: {:?}",
        bound.ports
    );
    assert!(
        bound.ports.windows(2).all(|w| w[0] < w[1]),
        "端口应去重升序: {:?}",
        bound.ports
    );

    // 释放后：必须不再包含
    drop(listener);
    // 系统释放监听条目可能有毫秒级延迟，轮询等它消失（最多 2 秒）
    let gone = wait_until(
        || !probe.probe(&ctx).ports.contains(&port),
        Duration::from_secs(2),
    )
    .await;
    assert!(gone, "释放监听后不应再采到端口 {port}");

    // 打印一次释放后的完整结果，作为验收证据
    let released = probe.probe(&ctx);
    println!("释放 {port} 后采集到的端口: {:?}", released.ports);
}

// ==================== c) 窗口标题：真实带窗口进程 ====================

/// 整机是否处于**可交互桌面会话**。
///
/// 交互式控制台/远程桌面下 `SESSIONNAME` 是 `Console` / `RDP-Tcp#N`；
/// Session 0 的服务进程不设置该变量，容器里通常也没有。
fn has_interactive_desktop() -> bool {
    std::env::var("SESSIONNAME")
        .map(|name| {
            let name = name.trim();
            !name.is_empty() && !name.eq_ignore_ascii_case("services")
        })
        .unwrap_or(false)
}

/// 起一个真实带窗口的进程，验证窗口标题采集。
///
/// 选型说明（实测结论，记在这里避免后人重复踩坑）：
/// - `notepad.exe` 在这台机器上是 **Microsoft Store 应用的转发 stub**：
///   进程存活但窗口属于另一个包进程，直接子进程始终采不到标题；
/// - `calc.exe` 同样是 Store 转发，启动后很快退出；
/// - `mspaint.exe` 可用（实测 1.5 秒内采到「无标题 - 画图」），但并非所有 Windows 版本都自带；
/// - **PowerShell + WinForms** 最可靠：窗口直接属于被拉起的 powershell 进程本身，
///   任何带 .NET 的 Windows 都能跑，且标题由我们自己指定，断言更精确。
///
/// **为什么不加 `#[ignore]`、又如何在无 GUI 环境不假红**：
/// 窗口标题采集的前提是存在可交互的窗口站（WinForms 才建得出窗口），
/// Session 0 / 容器 / 无 GUI 的服务环境里这一点不成立——那是**环境限制，不是代码缺陷**。
/// 因此这里用「运行时探测 + 显式跳过并打印原因」代替编译期的 `#[ignore]`：
/// - 有交互桌面（本机开发环境）：**默认全量测试就会自动执行并严格断言**；
/// - 没有交互桌面（服务/容器）：打印一行原因后跳过，不让无 GUI 环境出现假红。
///
/// 判据是 `SESSIONNAME`：交互式控制台/远程桌面下它是 `Console` / `RDP-Tcp#N`，
/// 而 Session 0 的服务进程不设置该变量。零依赖，足以区分两种环境。
///
/// 唯一的缺口是「有桌面但窗口建不出来」时会退化成不报错的早退；
/// 因此断言保留两条独立的硬证据（标题非空 + 与窗口实际标题逐字相等），
/// 并且会打印采到的标题，人工复核只需看一眼输出。
///
/// 手动验证命令（单独跑、并显示采到的标题）：
/// ```bash
/// cargo test -p launcher-core --test status_probe -- --nocapture window_title
/// ```
#[tokio::test]
async fn window_title_probe_reads_real_window_title() {
    use launcher_core::platform::window_title_for_pid;

    if !has_interactive_desktop() {
        println!("跳过：当前进程不在可交互桌面会话中（SESSIONNAME 未设置或为 Services）");
        return;
    }

    let env = TestEnv::new();
    let manager = env.manager(fast_status_capture());

    // 窗口标题用 ASCII，避免 PowerShell 参数里的中文经过控制台代码页时被改写
    const EXPECTED_TITLE: &str = "app_catch window probe";
    let script = format!(
        "Add-Type -AssemblyName System.Windows.Forms; \
         $f = New-Object System.Windows.Forms.Form; \
         $f.Text = '{EXPECTED_TITLE}'; \
         $f.Show(); \
         Start-Sleep -Seconds 30"
    );

    let mut app = AppConfig::new(
        AppId::new("window-probe").expect("合法 id"),
        "窗口探针",
        "powershell.exe",
    );
    app.args = vec![
        "-NoProfile".to_string(),
        "-WindowStyle".to_string(),
        "Hidden".to_string(),
        "-Command".to_string(),
        script,
    ];

    let session = manager.start(&app).await.expect("启动 powershell 失败");
    let pid = session.handle.pid;

    // 窗口创建需要一点时间：轮询等待（最多 15 秒，实测 2 秒内出现）
    let mut title: Option<String> = None;
    for _ in 0..150 {
        title = window_title_for_pid(pid);
        if title.is_some() {
            break;
        }
        tokio::time::sleep(Duration::from_millis(100)).await;
    }

    // 无论成败都先收尾，不留后台 powershell
    let _ = manager.stop(session.session_id).await;
    let _ = manager
        .wait_for_exit(session.session_id, Duration::from_secs(30))
        .await;

    let title = title.expect("应采到 powershell 的窗口标题（无桌面环境时本用例应被 --ignored 跳过）");
    println!("采到的窗口标题 = {title:?}");
    assert!(!title.trim().is_empty(), "窗口标题不应为空: {title:?}");
    assert_eq!(
        title, EXPECTED_TITLE,
        "采到的标题应与窗口实际标题一致（证明拿到的是主窗口而不是别的窗口）"
    );
}

// ==================== d) 变化才写事件 ====================

/// 固定返回值的 probe 跑一段时间：status 事件应恒为 1 条（只在首次写）。
#[tokio::test]
async fn status_loop_writes_once_for_constant_state() {
    let env = TestEnv::new();
    let probe = ScriptedProbe::fixed(Some("固定窗口"), vec![9000]);
    let manager = env.manager_with_probe(fast_status_capture(), probe.clone());

    let app = cmd_app(&["/c", "ping -n 60 127.0.0.1 > nul"]);
    let session = manager.start(&app).await.expect("启动失败");

    // 等够多轮采样：关键性质是「采了不止一次」，而不是精确频率
    assert!(
        wait_for_rounds(&probe, 3, 60).await,
        "状态循环应被多次调用，实际 {} 次",
        probe.calls()
    );

    manager.stop(session.session_id).await.expect("停止失败");
    manager
        .wait_for_exit(session.session_id, Duration::from_secs(30))
        .await
        .expect("等待退出失败");

    let events = read_all_events(&manager.session_dir(&session.session_id));
    let status_count = count_status(&events);
    println!(
        "固定状态：probe 调用 {} 次，status 事件 {} 条",
        probe.calls(),
        status_count
    );
    assert_eq!(
        status_count, 1,
        "状态不变时只应在首轮写 1 条 status 事件，实际 {status_count} 条"
    );

    // 首条 status 必须带上 probe 给的值
    let first = events
        .iter()
        .find(|e| matches!(e, SessionEvent::Status { .. }))
        .expect("应有 status 事件");
    match first {
        SessionEvent::Status {
            state,
            window_title,
            ports,
            ..
        } => {
            assert_eq!(*state, RunState::Running);
            assert_eq!(window_title.as_deref(), Some("固定窗口"));
            assert_eq!(ports, &Some(vec![9000]));
        }
        other => panic!("应为 status 事件，实际: {other:?}"),
    }

    // 日志末条必须是 exit（状态循环先收尾，exit 才落盘）
    assert!(
        matches!(events.last(), Some(SessionEvent::Exit { .. })),
        "末条应为 exit，实际: {:?}",
        events.last()
    );
}

/// 可变 probe：第二轮返回值变化后应新增 1 条 status 事件（总计 2 条）。
#[tokio::test]
async fn status_loop_writes_again_only_after_state_changes() {
    let env = TestEnv::new();
    let probe = ScriptedProbe::scripted(vec![
        StatusSnapshot {
            window_title: Some("窗口".to_string()),
            ports: vec![9000],
        },
        // 从第二轮起端口变了
        StatusSnapshot {
            window_title: Some("窗口".to_string()),
            ports: vec![9001],
        },
    ]);
    let manager = env.manager_with_probe(fast_status_capture(), probe.clone());

    let app = cmd_app(&["/c", "ping -n 60 127.0.0.1 > nul"]);
    let session = manager.start(&app).await.expect("启动失败");

    assert!(
        wait_for_rounds(&probe, 3, 60).await,
        "状态循环应被多次调用，实际 {} 次",
        probe.calls()
    );

    manager.stop(session.session_id).await.expect("停止失败");
    manager
        .wait_for_exit(session.session_id, Duration::from_secs(30))
        .await
        .expect("等待退出失败");

    let events = read_all_events(&manager.session_dir(&session.session_id));
    let ports: Vec<Option<Vec<u16>>> = events
        .iter()
        .filter_map(|e| match e {
            SessionEvent::Status { ports, .. } => Some(ports.clone()),
            _ => None,
        })
        .collect();
    println!(
        "变化场景：probe 调用 {} 次，status 事件 {} 条，ports 序列 {:?}",
        probe.calls(),
        ports.len(),
        ports
    );

    assert_eq!(
        count_status(&events),
        2,
        "状态变化 1 次应只新增 1 条事件（1 首次 + 1 变化），实际: {ports:?}"
    );
    assert_eq!(ports, vec![Some(vec![9000]), Some(vec![9001])]);
}

// ==================== e) 进程退出后循环结束、无泄漏 ====================

/// 长跑进程 stop 后：状态循环必须自行结束，日志末条为 exit。
#[tokio::test]
async fn status_loop_terminates_after_process_exit() {
    let env = TestEnv::new();
    let probe = ScriptedProbe::fixed(Some("窗口"), vec![]);
    let manager = env.manager_with_probe(fast_status_capture(), probe.clone());

    let app = cmd_app(&["/c", "ping -n 60 127.0.0.1 > nul"]);
    let session = manager.start(&app).await.expect("启动失败");

    assert!(wait_for_rounds(&probe, 2, 60).await, "状态循环应先跑起来");
    let calls_before_stop = probe.calls();

    manager.stop(session.session_id).await.expect("停止失败");
    manager
        .wait_for_exit(session.session_id, Duration::from_secs(30))
        .await
        .expect("等待退出失败");

    // 退出后的宽限期内，probe 不应再被调用：说明循环确实结束了。
    // 宽限期要大于采集间隔（100ms）的若干倍，才能区分「循环已结束」与「恰好还没轮到下一轮」。
    let calls_at_exit = probe.calls();
    tokio::time::sleep(Duration::from_secs(2)).await;
    let calls_after = probe.calls();
    assert_eq!(
        calls_at_exit, calls_after,
        "进程退出后状态循环必须停止采样（退出时 {calls_at_exit} 次，2 秒后 {calls_after} 次）"
    );
    println!(
        "退出前采样 {calls_before_stop} 次，退出时 {calls_at_exit} 次，2 秒后仍为 {calls_after} 次（不再增长即证明循环已结束）"
    );

    // 快照必须已收敛到退出态
    let snapshot = manager.status(session.session_id).await.expect("查状态失败");
    assert_eq!(snapshot.state, RunState::Exited);
    assert_eq!(
        snapshot.last_exit.map(|e| e.reason),
        Some(ExitReason::Killed)
    );

    // 日志末条必须是 exit
    let events = read_all_events(&manager.session_dir(&session.session_id));
    assert!(
        matches!(events.last(), Some(SessionEvent::Exit { .. })),
        "末条应为 exit，实际: {:?}",
        events.last()
    );

    // 会话目录事件流自洽：start 首条、exit 末条
    assert!(matches!(events.first(), Some(SessionEvent::Start { .. })));
}

// ==================== 快照与配置行为 ====================

/// status() 应返回最新采集到的状态（供 M2 直接读）。
#[tokio::test]
async fn snapshot_exposes_latest_status_for_gui() {
    let env = TestEnv::new();
    let probe = ScriptedProbe::fixed(Some("GUI 主窗口"), vec![8080, 8443]);
    let manager = env.manager_with_probe(fast_status_capture(), probe.clone());

    let app = cmd_app(&["/c", "ping -n 60 127.0.0.1 > nul"]);
    let session = manager.start(&app).await.expect("启动失败");

    assert!(wait_for_rounds(&probe, 1, 60).await, "状态循环应至少采样一次");
    // 再等一小段，确保快照回写已完成
    tokio::time::sleep(Duration::from_millis(300)).await;

    let snapshot = manager.status(session.session_id).await.expect("查状态失败");
    let latest = snapshot.latest_status.expect("应已有 latest_status");
    assert_eq!(latest.window_title.as_deref(), Some("GUI 主窗口"));
    assert_eq!(latest.ports, vec![8080, 8443]);

    // list_active 也应带上同样的状态
    let active = manager.list_active().await;
    assert_eq!(active.len(), 1);
    assert_eq!(
        active[0].latest_status.as_ref().map(|s| s.ports.clone()),
        Some(vec![8080, 8443])
    );

    manager.stop(session.session_id).await.expect("停止失败");
    manager
        .wait_for_exit(session.session_id, Duration::from_secs(30))
        .await
        .expect("等待退出失败");
}

/// status_interval 为 None 时不启动循环：不写 status 事件，快照保持 None。
#[tokio::test]
async fn disabled_status_loop_writes_no_events() {
    let env = TestEnv::new();
    let probe = ScriptedProbe::fixed(Some("不该出现"), vec![1234]);
    let manager = env.manager_with_probe(status_disabled_capture(), probe.clone());

    let app = cmd_app(&["/c", "ping -n 3 127.0.0.1 > nul"]);
    let session = manager.start(&app).await.expect("启动失败");
    manager
        .wait_for_exit(session.session_id, Duration::from_secs(30))
        .await
        .expect("等待退出失败");

    assert_eq!(probe.calls(), 0, "禁用时不应调用 probe");

    let events = read_all_events(&manager.session_dir(&session.session_id));
    assert_eq!(count_status(&events), 0, "禁用时不应写 status 事件");

    let snapshot = manager.status(session.session_id).await.expect("查状态失败");
    assert!(snapshot.latest_status.is_none(), "禁用时快照应保持 None");
}

// ==================== f) status 事件 JSON 形状 ====================

/// 有值时序列化正确；为 null 时字段仍在（与既有格式一致，不省略）。
#[test]
fn status_event_json_shape_with_and_without_values() {
    let with_values = SessionEvent::status(
        RunState::Running,
        Some("GPT-SoVITS".to_string()),
        Some(vec![9880, 9881]),
    );
    let line = with_values.to_jsonl_line().expect("序列化失败");
    println!("有值 status: {line}");
    assert!(line.contains("\"window_title\":\"GPT-SoVITS\""));
    assert!(line.contains("\"ports\":[9880,9881]"));
    assert!(!line.contains('\n'), "必须是单行");

    let without_values = SessionEvent::status(RunState::Running, None, None);
    let line = without_values.to_jsonl_line().expect("序列化失败");
    println!("空值 status: {line}");
    assert!(
        line.contains("\"window_title\":null"),
        "window_title 为 None 时仍应输出字段: {line}"
    );
    assert!(
        line.contains("\"ports\":null"),
        "ports 为 None 时仍应输出字段: {line}"
    );

    // 空 ports 的 Vec 必须映射成 null 而不是 []
    let snapshot = StatusSnapshot {
        window_title: None,
        ports: vec![],
    };
    assert_eq!(snapshot.ports_field(), None);
}

/// CompositeProbe 的窗口 + 端口应各自填充，互不覆盖。
#[test]
fn composite_probe_layers_fields_from_multiple_probes() {
    struct TitleOnly;
    impl StatusProbe for TitleOnly {
        fn probe(&self, _ctx: &ProbeContext) -> StatusSnapshot {
            StatusSnapshot {
                window_title: Some("标题".to_string()),
                ports: Vec::new(),
            }
        }
        fn name(&self) -> &'static str {
            "title"
        }
    }
    struct PortsOnly;
    impl StatusProbe for PortsOnly {
        fn probe(&self, _ctx: &ProbeContext) -> StatusSnapshot {
            StatusSnapshot {
                window_title: None,
                ports: vec![7000],
            }
        }
        fn name(&self) -> &'static str {
            "ports"
        }
    }

    let composite = CompositeProbe::with(vec![
        Arc::new(TitleOnly) as Arc<dyn StatusProbe>,
        Arc::new(PortsOnly) as Arc<dyn StatusProbe>,
    ]);
    let snapshot = composite.probe(&ProbeContext::new(1));

    assert_eq!(snapshot.window_title.as_deref(), Some("标题"));
    assert_eq!(snapshot.ports, vec![7000]);
}
