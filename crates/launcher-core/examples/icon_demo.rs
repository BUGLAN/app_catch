//! 手动取证用示例（不提交测试矩阵）：真实抓取图标并把结果打印到 stdout。
//!
//! 用法：
//!   cargo run -p launcher-core --example icon_demo -- notepad
//!   cargo run -p launcher-core --example icon_demo -- C:/Windows/System32/notepad.exe
use launcher_core::config::AppId;
use launcher_core::platform::icon::{load_png_file, resolve_executable};
use launcher_core::platform::{default_icon_capture, IconCapture, IconTarget};

fn main() {
    let arg = std::env::args().nth(1).unwrap_or_else(|| "notepad".to_string());

    // 尊重外部传入的 APP_CATCH_HOME（便于把证据产物留在固定位置）；
    // 没传就用独立临时目录，绝不污染真实 ~/.app_catch
    let created_temp = std::env::var_os("APP_CATCH_HOME").is_none();
    let dir = match std::env::var_os("APP_CATCH_HOME") {
        Some(raw) => std::path::PathBuf::from(raw),
        None => std::env::temp_dir().join(format!("app-catch-icon-demo-{}", std::process::id())),
    };
    std::env::set_var("APP_CATCH_HOME", &dir);

    let exe = resolve_executable(&arg);
    println!("输入命令   : {arg}");
    println!("解析到的 exe: {exe:?}");

    let target = IconTarget::for_app(AppId::new("demo-app").expect("合法 id"))
        .with_executable(exe);

    let capture = default_icon_capture();
    let started = std::time::Instant::now();
    let path = capture.capture(&target);
    let first_ms = started.elapsed().as_millis();

    match &path {
        Some(p) => {
            let meta = std::fs::metadata(p).expect("读缓存元数据失败");
            let image = load_png_file(p).expect("解码缓存失败");
            println!("--- 首次抓取 ---");
            println!("缓存路径   : {}", p.display());
            println!("文件大小   : {} 字节", meta.len());
            println!("图像尺寸   : {}x{}", image.width, image.height);
            println!("耗时       : {first_ms} ms");
        }
        None => {
            println!("--- 首次抓取 ---");
            println!("结果       : None（走前端内置默认图兜底，不是错误）");
            println!("耗时       : {first_ms} ms");
        }
    }

    // 二次调用：缓存命中证据
    let mtime_before = path
        .as_ref()
        .and_then(|p| std::fs::metadata(p).ok())
        .and_then(|m| m.modified().ok());
    let started = std::time::Instant::now();
    let second = capture.capture(&target);
    let second_us = started.elapsed().as_micros();
    let mtime_after = second
        .as_ref()
        .and_then(|p| std::fs::metadata(p).ok())
        .and_then(|m| m.modified().ok());

    println!("--- 二次调用（缓存） ---");
    println!("返回路径   : {:?}", second.as_ref().map(|p| p.display().to_string()));
    println!("耗时       : {second_us} us");
    println!("mtime 未变 : {}", mtime_before == mtime_after);

    // 兜底：不存在的 exe
    let ghost = IconTarget::for_app(AppId::new("ghost").expect("合法 id"))
        .with_executable(Some(std::path::PathBuf::from("C:/no/such/app.exe")));
    let ghost_result = capture.capture(&ghost);
    println!("--- 兜底（不存在的 exe） ---");
    println!("结果       : {ghost_result:?}");
    println!(
        "缓存目录残留: {:?}",
        std::fs::read_dir(&dir).map(|d| d.count()).unwrap_or(0)
    );

    if created_temp {
        let _ = std::fs::remove_dir_all(&dir);
    } else {
        println!("证据产物已保留在: {}", dir.display());
    }
}
