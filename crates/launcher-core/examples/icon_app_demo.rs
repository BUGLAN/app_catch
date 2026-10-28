//! 手动取证示例：抓一个**第三方 GUI 应用**的窗口图标（最接近真实使用场景）。
//!
//! 用法：
//!   APP_CATCH_HOME=/tmp/icon-app cargo run -p launcher-core --example icon_app_demo -- "C:/Program Files/Bandizip/Bandizip.exe"

#[cfg(not(windows))]
fn main() {
    eprintln!("icon_app_demo 依赖 Windows API（Job Object / Shell 图标），仅在 Windows 上可用。");
}

#[cfg(windows)]
fn main() {
    imp::main()
}

#[cfg(windows)]
mod imp {
    use std::time::Duration;

    use launcher_core::config::{AppConfig, AppId};
    use launcher_core::platform::icon::load_png_file;
    use launcher_core::platform::{
        collect_tree_pids, default_icon_capture, default_process_backend, window_icon_for_pids,
        IconCapture, IconTarget,
    };
    use launcher_core::runtime::capture::CaptureConfig;
    use launcher_core::runtime::SessionManager;

    #[tokio::main]
    pub async fn main() {
        let exe = std::env::args().nth(1).expect("用法: icon_app_demo <exe 路径>");
        if std::env::var_os("APP_CATCH_HOME").is_none() {
            eprintln!("请先设置 APP_CATCH_HOME");
            std::process::exit(2);
        }

        let mut app = AppConfig::new(
            AppId::new("third-party").expect("合法 id"),
            "第三方应用",
            exe.clone(),
        );
        // 不给参数：让应用以默认方式打开主窗口
        app.args = Vec::new();

        let manager = SessionManager::from_home(
            std::sync::Arc::new(default_process_backend()),
            CaptureConfig::default(),
        )
        .expect("构造 SessionManager 失败");

        let session = match manager.start(&app).await {
            Ok(s) => s,
            Err(e) => {
                println!("启动失败（跳过）：{e}");
                return;
            }
        };
        let pid = session.handle.pid;
        println!("已启动 {exe}，pid = {pid}");

        // 第三方应用启动较慢：最多等 20 秒
        let mut window_image = None;
        for _ in 0..200 {
            let pids = collect_tree_pids(pid);
            if let Some(image) = window_icon_for_pids(&pids) {
                window_image = Some(image);
                break;
            }
            tokio::time::sleep(Duration::from_millis(100)).await;
        }

        match &window_image {
            Some(image) => println!(
                "窗口图标: {}x{}，{} 像素字节",
                image.width,
                image.height,
                image.pixels.len()
            ),
            None => println!("窗口图标: 未抓到（窗口未出现或不是可交互桌面）"),
        }

        let target = IconTarget::for_app(AppId::new("third-party").expect("合法 id"))
            .with_executable(Some(std::path::PathBuf::from(&exe)))
            .with_pid(Some(pid));
        if let Some(p) = default_icon_capture().capture(&target) {
            let meta = std::fs::metadata(&p).expect("读元数据失败");
            let image = load_png_file(&p).expect("解码失败");
            println!("缓存路径  : {}", p.display());
            println!("文件大小  : {} 字节", meta.len());
            println!("图像尺寸  : {}x{}", image.width, image.height);
        } else {
            println!("capture: None");
        }

        let _ = manager.stop(session.session_id).await;
        let _ = manager
            .wait_for_exit(session.session_id, Duration::from_secs(20))
            .await;
    }
}
