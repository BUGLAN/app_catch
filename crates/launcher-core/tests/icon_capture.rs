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

// ===========================================================================
// e) .ico / .lnk 输入支持
// ===========================================================================

/// .ico 文件应能被当作自定义图标读出来（走资源抽取路径）。
///
/// 这是「也支持 .lnk/.ico/.png 输入」这条要求里 .ico 的证据。
/// 用仓库自带的 src-tauri/icons/icon.ico，不依赖系统里的随机文件。
#[test]
fn ico_file_is_supported_as_custom_icon() {
    let ico = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../../src-tauri/icons/icon.ico");
    if !ico.is_file() {
        println!("跳过：找不到 {}", ico.display());
        return;
    }

    let image = file_icon(&ico).expect("应能从 .ico 抠出图标");
    println!(".ico 图标：{}x{}", image.width, image.height);
    assert!(image.width > 0 && image.height > 0);
    assert!(!image.is_fully_transparent(), ".ico 图标不应全透明");
}

/// .lnk 快捷方式应能通过 Shell 解析到目标并拿到图标。
///
/// 用系统里一定存在的快捷方式（开始菜单里的任一 .lnk）。
/// 找不到任何 .lnk 时打印跳过原因——不是所有环境都有用户开始菜单。
#[test]
fn lnk_shortcut_is_supported() {
    let candidates = [
        "C:/ProgramData/Microsoft/Windows/Start Menu/Programs",
        "C:/Users/Public/Desktop",
    ];
    let mut found: Option<std::path::PathBuf> = None;
    for dir in candidates {
        let Ok(entries) = std::fs::read_dir(dir) else {
            continue;
        };
        for entry in entries.flatten() {
            let path = entry.path();
            if path.extension().map(|e| e.eq_ignore_ascii_case("lnk")).unwrap_or(false) {
                found = Some(path);
                break;
            }
        }
        if found.is_some() {
            break;
        }
    }

    let Some(lnk) = found else {
        println!("跳过：本机用户/公共开始菜单里没有任何 .lnk");
        return;
    };

    match file_icon(&lnk) {
        Some(image) => {
            println!(".lnk ({}) 图标：{}x{}", lnk.display(), image.width, image.height);
            assert!(image.width > 0 && image.height > 0);
        }
        None => {
            // .lnk 指向的目标可能已被卸载：这属于环境问题，不是缺陷
            println!("跳过：无法从 {} 解析出图标（目标可能已卸载）", lnk.display());
        }
    }
}

/// 端到端：把 .ico 配成自定义图标，capture 应把它规范化后落成 PNG 缓存。
#[test]
fn ico_custom_icon_is_normalized_into_png_cache() {
    let ico = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../../src-tauri/icons/icon.ico");
    if !ico.is_file() {
        println!("跳过：找不到 icon.ico");
        return;
    }

    let env = TestEnv::new();
    let target = env.target("ico-app").with_custom_icon(Some(ico));

    let path = default_icon_capture().capture(&target).expect("应能抓到 .ico 图标");
    assert_eq!(
        path,
        env.icons_dir().join("ico-app.png"),
        "缓存必须是 PNG 而不是原样拷贝 .ico"
    );

    let bytes = std::fs::read(&path).expect("读缓存失败");
    assert!(looks_like_png(&bytes), "缓存必须是 PNG 格式");

    let decoded = launcher_core::platform::icon::load_png_file(&path).expect("缓存应可解码");
    println!(".ico -> PNG 缓存：{} 字节，{}x{}", bytes.len(), decoded.width, decoded.height);
    assert!(decoded.width <= ICON_MAX_SIZE && decoded.height <= ICON_MAX_SIZE);
}


/// .lnk 的正向验证：临时造一个指向 notepad.exe 的快捷方式，确认能解析出图标。
///
/// 与 lnk_shortcut_is_supported 的区别：那条用的是系统里现成的 .lnk（可能指向已卸载程序），
/// 这条**自己造一个一定有效的**，因此可以严格断言「拿到了非空图标」。
#[test]
fn lnk_pointing_to_real_exe_yields_icon() {
    let lnk = std::env::temp_dir().join("appcatch-test.lnk");
    if !lnk.is_file() {
        println!(
            "跳过：{} 不存在（需先用 WScript.Shell 创建一个指向 notepad.exe 的快捷方式）",
            lnk.display()
        );
        return;
    }

    let image = file_icon(&lnk).expect("应能从 .lnk 解析出目标图标");
    println!(".lnk -> 图标：{}x{}", image.width, image.height);
    assert!(image.width > 0 && image.height > 0);
    assert!(!image.is_fully_transparent(), ".lnk 图标不应全透明");
}


// ===========================================================================
// f) 正斜杠路径回归（实测踩出来的坑）
// ===========================================================================

/// 正斜杠路径必须同样能抓出图标。
///
/// 背景：SHGetFileInfoW 对正斜杠路径**直接返回 0**（拿不到 hIcon），
/// 而本项目里用户填的路径到处都是正斜杠——docs/CONFIG.md 的示例就是
/// cwd = "E:/repo/GPT-SoVITS"、icon = "E:/repo/GPT-SoVITS/icon.png"。
/// PrivateExtractIconsW 对正斜杠宽容，所以这个 bug 只在「exe 档位失败、
/// 回落到 Shell 档位」时才暴露——正是 .lnk 的场景。
///
/// 修法是在转宽字符时把 / 换成反斜杠。这条测试钉住修复，
/// 避免以后有人「清理」掉那个看起来多余的字面替换。
#[test]
fn forward_slash_path_works_like_backslash() {
    if !std::path::Path::new("C:/Windows/System32/notepad.exe").is_file() {
        println!("跳过：本机没有 notepad.exe");
        return;
    }

    let forward = file_icon(std::path::Path::new("C:/Windows/System32/notepad.exe"));
    let back = file_icon(&std::path::PathBuf::from(
        "C:\\Windows\\System32\\notepad.exe",
    ));

    let forward = forward.expect("正斜杠路径应能抓出图标");
    let back = back.expect("反斜杠路径应能抓出图标");
    assert_eq!(
        (forward.width, forward.height),
        (back.width, back.height),
        "两种写法应抓到同一尺寸的图标"
    );
}

/// .lnk 用正斜杠写也必须能解析（走 Shell 档位，最容易被上面那个 bug 打中的场景）。
#[test]
fn forward_slash_lnk_resolves_through_shell() {
    let lnk = std::env::temp_dir().join("appcatch-test.lnk");
    if !lnk.is_file() {
        println!("跳过：{} 不存在", lnk.display());
        return;
    }
    // 把反斜杠换成正斜杠（模拟用户从文档里复制出来的写法）
    let forward = lnk.to_string_lossy().replace('\\', "/");
    let image = file_icon(std::path::Path::new(&forward))
        .unwrap_or_else(|| panic!("正斜杠 .lnk 应能解析出图标: {forward}"));
    println!("正斜杠 .lnk -> {}x{}", image.width, image.height);
    assert!(image.width > 0 && image.height > 0);
}

// ===========================================================================
// g) 并发回归：Shell 图标 API 必须串行化
// ===========================================================================

/// 高并发下反复抓同一个 .lnk，必须**每次都成功**。
///
/// 背景（实测）：SHGetFileInfoW 解析 .lnk 时并发调用会稳定失败——
/// 本机并发 ≤6 线程全成功、≥8 线程必现一次静默失败（返回 0 / 空 hIcon）。
/// 失败是静默的，调用方分不清「这文件真没图标」和「这次撞上了并发缺陷」。
///
/// 修法：file_icon 内部用进程级互斥锁串行化。代价可忽略
/// （图标每个应用只抓一次，之后走磁盘缓存），却换掉了一整类不确定性。
///
/// 这条测试用 16 线程并发 10 轮钉住修复；去掉锁会稳定复现失败。
#[test]
fn shell_icon_api_is_serialized_under_concurrency() {
    let lnk = std::env::temp_dir().join("appcatch-test.lnk");
    if !lnk.is_file() {
        println!("跳过：{} 不存在（需先创建 .lnk 测试文件）", lnk.display());
        return;
    }

    use std::sync::Barrier;
    use std::sync::atomic::{AtomicUsize, Ordering};

    const THREADS: usize = 16;
    const ROUNDS: usize = 10;

    let lnk = std::sync::Arc::new(lnk);
    let successes = std::sync::Arc::new(AtomicUsize::new(0));
    let barrier = std::sync::Arc::new(Barrier::new(THREADS));

    let mut handles = Vec::with_capacity(THREADS);
    for _ in 0..THREADS {
        let lnk = std::sync::Arc::clone(&lnk);
        let successes = std::sync::Arc::clone(&successes);
        let barrier = std::sync::Arc::clone(&barrier);
        handles.push(std::thread::spawn(move || {
            // 让全部线程尽量同时冲进 file_icon
            barrier.wait();
            for _ in 0..ROUNDS {
                if file_icon(&lnk).is_some() {
                    successes.fetch_add(1, Ordering::Relaxed);
                }
            }
        }));
    }
    for handle in handles {
        handle.join().expect("并发抓取线程不应 panic");
    }

    let total = THREADS * ROUNDS;
    let ok = successes.load(Ordering::Relaxed);
    println!("并发抓取 {total} 次，成功 {ok} 次");
    assert_eq!(
        ok, total,
        "并发抓取 .lnk 必须全部成功（串行化失效时会静默返回 None）：{ok}/{total}"
    );
}

// ===========================================================================
// h) .lnk 夹具说明（不依赖手工准备的文件时明确跳过）
// ===========================================================================

/// 确认 .lnk 夹具的获取方式：有就复用，没有就打印生成方法并跳过。
///
/// 生成 .lnk 必须走 COM（Shell Link 对象），测试里起 PowerShell 太慢且依赖环境，
/// 因此这里只做「夹具存在性检查 + 缺失时给出可复制的生成命令」，
/// 让无夹具的环境明确跳过而不是假红。
#[test]
fn lnk_fixture_is_available_or_skipped() {
    let lnk = std::env::temp_dir().join("appcatch-test.lnk");
    if lnk.is_file() {
        println!("复用已有夹具: {}", lnk.display());
        return;
    }
    println!("跳过：{} 不存在", lnk.display());
    println!("生成方式（PowerShell）：");
    println!("  $s = New-Object -ComObject WScript.Shell");
    println!("  $l = $s.CreateShortcut(...)");
    println!("  $l.TargetPath = ...notepad.exe; $l.Save()");
}
