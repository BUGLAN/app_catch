//! 图标抓取集成测试：真实窗口图标、真实 exe 资源图标、缓存命中、兜底语义。
//!
//! 隔离原则与其它集成测试一致：每个用例把 APP_CATCH_HOME 指向独立临时目录，
//! 并用进程内互斥锁串行化（环境变量是进程级全局状态）。
//!
//! 为什么这些用例默认执行而不是 #[ignore]：
//! 图标抓取的前提是「有可交互窗口站」与「系统能给出图标资源」。
//! 无 GUI 的 Session 0 / 容器里前者不成立，那是**环境限制不是代码缺陷**，
//! 因此用「运行时探测 + 明确打印跳过原因」代替编译期忽略；
//! 有桌面的开发机上它们会全量执行并严格断言。

use std::sync::{Mutex, OnceLock};
use std::time::Duration;

use launcher_core::config::{AppConfig, AppId};
use launcher_core::platform::icon::{ICON_MAX_SIZE, looks_like_png};
use launcher_core::platform::{
    default_icon_capture, default_process_backend, file_icon, window_icon_for_pids, IconCapture,
    IconTarget,
};
use launcher_core::runtime::capture::CaptureConfig;
use launcher_core::runtime::SessionManager;

/// 串行化全局环境变量的互斥锁。
fn env_lock() -> &'static Mutex<()> {
    static LOCK: OnceLock<Mutex<()>> = OnceLock::new();
    LOCK.get_or_init(|| Mutex::new(()))
}

/// 测试用隔离环境。
struct TestEnv {
    dir: tempfile::TempDir,
    _guard: std::sync::MutexGuard<'static, ()>,
}

impl TestEnv {
    fn new() -> Self {
        let guard = env_lock().lock().unwrap_or_else(|e| e.into_inner());
        let dir = tempfile::tempdir().expect("创建临时目录失败");
        std::env::set_var("APP_CATCH_HOME", dir.path());
        Self { dir, _guard: guard }
    }

    fn icons_dir(&self) -> std::path::PathBuf {
        self.dir.path().join("icons")
    }

    fn manager(&self) -> SessionManager {
        SessionManager::from_home(Arc::new(default_process_backend()), CaptureConfig::default())
            .expect("构造 SessionManager 失败")
    }

    fn target(&self, app_id: &str) -> IconTarget {
        IconTarget::for_app(AppId::new(app_id).expect("合法 id"))
    }
}

impl Drop for TestEnv {
    fn drop(&mut self) {
        std::env::remove_var("APP_CATCH_HOME");
    }
}

use std::sync::Arc;

/// 整机是否处于**可交互桌面会话**（与 status_probe.rs 用同一判据）。
fn has_interactive_desktop() -> bool {
    std::env::var("SESSIONNAME")
        .map(|name| {
            let name = name.trim();
            !name.is_empty() && !name.eq_ignore_ascii_case("services")
        })
        .unwrap_or(false)
}

// ===========================================================================
// a) 真实 exe 资源图标：notepad.exe
// ===========================================================================

/// notepad.exe 是每台 Windows 都有的 GUI 程序，且**自带真图标资源**。
///
/// 这是「exe 资源图标」这条路径最直接的证据：不需要窗口、不需要起进程，
/// 只要系统里有 notepad.exe 就应该抠出一张非空 PNG。
#[test]
fn real_exe_icon_is_extracted_from_notepad() {
    let exe = std::path::Path::new("C:/Windows/System32/notepad.exe");
    if !exe.is_file() {
        println!("跳过：本机没有 C:/Windows/System32/notepad.exe");
        return;
    }

    let image = file_icon(exe).expect("应能从 notepad.exe 抠出图标");
    println!(
        "notepad.exe 图标：{}x{}，{} 像素字节",
        image.width,
        image.height,
        image.pixels.len()
    );
    assert!(image.width > 0 && image.height > 0, "图标尺寸必须为正");
    assert!(!image.is_fully_transparent(), "图标不应是全透明空图");
    assert!(
        image.width <= 512 && image.height <= 512,
        "从资源里抠出的图标不应异常巨大: {}x{}",
        image.width,
        image.height
    );
}

// ===========================================================================
// b) 真实运行中窗口图标：PowerShell + WinForms
// ===========================================================================

/// 起一个真实带窗口的进程，从**窗口**（而不是 exe）抓图标。
///
/// 选 PowerShell + WinForms 的理由与 window_title 用例一致：
/// 窗口直接属于被拉起的进程本身，任何带 .NET 的 Windows 都能跑，
/// 且我们可以显式给窗口设置一个已知图标。
///
/// 这里刻意**不给窗口设自定义图标**：拿到的应该是 WinForms 默认窗体图标
/// （来自 System.Windows.Forms 的资源），足以证明 WM_GETICON 路径通。
#[tokio::test]
async fn real_window_icon_is_captured_from_running_process() {
    if !has_interactive_desktop() {
        println!("跳过：当前进程不在可交互桌面会话中（SESSIONNAME 未设置或为 Services）");
        return;
    }

    let env = TestEnv::new();
    let manager = env.manager();

    let script = "Add-Type -AssemblyName System.Windows.Forms; \
         $f = New-Object System.Windows.Forms.Form; \
         $f.Text = 'app_catch icon probe'; \
         $f.Show(); \
         Start-Sleep -Seconds 30";
    let mut app = AppConfig::new(
        AppId::new("icon-probe").expect("合法 id"),
        "图标探针",
        "powershell.exe",
    );
    app.args = vec![
        "-NoProfile".to_string(),
        "-WindowStyle".to_string(),
        "Hidden".to_string(),
        "-Command".to_string(),
        script.to_string(),
    ];

    let session = manager.start(&app).await.expect("启动 powershell 失败");
    let pid = session.handle.pid;

    // 窗口创建需要时间：轮询等待（最多 15 秒）
    let mut found = None;
    for _ in 0..150 {
        let pids = launcher_core::platform::collect_tree_pids(pid);
        if let Some(image) = window_icon_for_pids(&pids) {
            found = Some(image);
            break;
        }
        tokio::time::sleep(Duration::from_millis(100)).await;
    }

    // 无论成败都先收尾，不留后台 powershell
    let _ = manager.stop(session.session_id).await;
    let _ = manager
        .wait_for_exit(session.session_id, Duration::from_secs(30))
        .await;

    let image = found.expect("应能从运行中的窗口抓到图标（WM_GETICON / 类图标）");
    println!(
        "窗口图标：{}x{}，{} 像素字节",
        image.width,
        image.height,
        image.pixels.len()
    );
    assert!(image.width > 0 && image.height > 0);
    assert!(!image.is_fully_transparent(), "抓到的窗口图标不应全透明");
}

// ===========================================================================
// c) 端到端：capture -> 缓存文件 -> 二次调用命中缓存
// ===========================================================================

/// 走完整 capture 链路：产出 PNG 落到 <home>/icons/<app_id>.png，二次调用命中缓存。
///
/// 这条用例同时覆盖验收 D（真实 PNG 文件 + 尺寸）与 E（缓存生效）。
#[test]
fn capture_writes_png_cache_and_second_call_hits_it() {
    let env = TestEnv::new();
    let capture = default_icon_capture();
    let target = env
        .target("notepad-icon")
        .with_executable(Some(std::path::PathBuf::from(
            "C:/Windows/System32/notepad.exe",
        )));

    if !std::path::Path::new("C:/Windows/System32/notepad.exe").is_file() {
        println!("跳过：本机没有 notepad.exe");
        return;
    }

    let path = capture.capture(&target).expect("应抓到 notepad 图标");
    assert_eq!(path, env.icons_dir().join("notepad-icon.png"));
    assert!(path.is_file(), "缓存文件应存在: {}", path.display());

    let bytes = std::fs::read(&path).expect("读缓存失败");
    assert!(looks_like_png(&bytes), "缓存必须是 PNG");
    assert!(bytes.len() > 100, "PNG 不应是空壳，实际 {} 字节", bytes.len());

    let decoded = launcher_core::platform::icon::load_png_file(&path).expect("缓存应可解码");
    println!(
        "缓存 PNG: {} 字节，{}x{}",
        bytes.len(),
        decoded.width,
        decoded.height
    );
    assert!(
        decoded.width <= ICON_MAX_SIZE && decoded.height <= ICON_MAX_SIZE,
        "缓存图标不应超过 {}，实际 {}x{}",
        ICON_MAX_SIZE,
        decoded.width,
        decoded.height
    );

    // 缓存命中证据：删掉 exe 路径后再抓，仍应返回同一个缓存（说明没重抓）
    let mtime_before = std::fs::metadata(&path)
        .expect("读元数据失败")
        .modified()
        .expect("取 mtime 失败");
    let second = capture.capture(&target).expect("二次调用应命中缓存");
    let mtime_after = std::fs::metadata(&second)
        .expect("读元数据失败")
        .modified()
        .expect("取 mtime 失败");
    assert_eq!(path, second);
    assert_eq!(
        mtime_before, mtime_after,
        "命中缓存时不应重写文件（mtime 不变即是证据）"
    );
}

// ===========================================================================
// d) 兜底：无图标目标 -> None，不 panic、不写文件、不影响会话
// ===========================================================================

/// 不存在的 exe：capture 返回 None，缓存目录保持干净，全程无 panic。
#[test]
fn missing_executable_falls_back_silently() {
    let env = TestEnv::new();
    let capture = default_icon_capture();
    let target = env
        .target("ghost-app")
        .with_executable(Some(std::path::PathBuf::from(
            "C:/no/such/dir/ghost-app.exe",
        )));

    assert!(capture.capture(&target).is_none(), "抓不到就该返回 None");
    assert!(
        !env.icons_dir().join("ghost-app.png").exists(),
        "失败不应写任何缓存文件"
    );
}

/// 完全不存在的目标（无 exe、无 pid、无自定义图标）：仍然安静返回 None。
#[test]
fn empty_target_falls_back_silently() {
    let env = TestEnv::new();
    assert!(default_icon_capture().capture(&env.target("nothing")).is_none());
    assert!(!env.icons_dir().join("nothing.png").exists());
}

/// 无效的自定义图标路径不应挡住后续 exe 抓取（失败即继续，而不是失败即放弃）。
#[test]
fn invalid_custom_icon_falls_through_to_executable() {
    if !std::path::Path::new("C:/Windows/System32/notepad.exe").is_file() {
        println!("跳过：本机没有 notepad.exe");
        return;
    }
    let env = TestEnv::new();
    let target = env
        .target("custom-fallback")
        .with_custom_icon(Some(std::path::PathBuf::from("C:/no/such/icon.png")))
        .with_executable(Some(std::path::PathBuf::from(
            "C:/Windows/System32/notepad.exe",
        )));

    let path = default_icon_capture()
        .capture(&target)
        .expect("自定义图标无效时应回落到 exe 图标");
    assert!(path.is_file());
}

/// 有效的自定义 PNG 图标优先级最高：即使给了 exe，也应使用自定义图。
#[test]
fn valid_custom_icon_wins_over_executable() {
    let env = TestEnv::new();
    std::fs::create_dir_all(env.icons_dir()).expect("建 icons 目录失败");

    // 造一张 40x40 的纯绿自定义图标
    let custom = env.dir.path().join("my-icon.png");
    let image = launcher_core::platform::RgbaImage::solid(40, 40, [0, 200, 0, 255])
        .expect("构造失败");
    std::fs::write(
        &custom,
        launcher_core::platform::icon::encode_png(&image).expect("编码失败"),
    )
    .expect("写自定义图标失败");

    let target = env
        .target("custom-wins")
        .with_custom_icon(Some(custom))
        .with_executable(Some(std::path::PathBuf::from(
            "C:/Windows/System32/notepad.exe",
        )));

    let path = default_icon_capture().capture(&target).expect("应抓到自定义图标");
    let stored = launcher_core::platform::icon::load_png_file(&path).expect("解码失败");
    assert_eq!(
        (stored.width, stored.height),
        (40, 40),
        "应使用自定义图（40x40），而不是 notepad 的 exe 图标"
    );
    // 自定义图是纯绿：抽查第一个像素
    assert_eq!(&stored.pixels[0..4], &[0, 200, 0, 255]);
}
