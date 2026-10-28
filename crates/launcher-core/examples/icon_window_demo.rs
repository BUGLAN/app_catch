//! 手动取证用示例：起一个真实带窗口的进程，从**窗口**抓运行时图标并落盘。
//!
//! 用法：
//!   APP_CATCH_HOME=/tmp/icon-runtime cargo run -p launcher-core --example icon_window_demo
//!
//! 为什么需要它：集成测试只在「有交互桌面」时执行且断言较宽松，
//! 这个示例把过程与产物都打印出来，便于人工核对运行时图标的实际效果。

#[cfg(not(windows))]
fn main() {
    eprintln!("icon_window_demo 依赖 Windows API（窗口 HICON），仅在 Windows 上可用。");
}

#[cfg(windows)]
fn main() {
    imp::main()
}

#[cfg(windows)]
mod imp {
    use std::time::Duration;

    use launcher_core::config::{AppConfig, AppId};
    use launcher_core::platform::icon::{load_png_file, resolve_executable};
    use launcher_core::platform::{
        collect_tree_pids, default_icon_capture, default_process_backend, window_icon_for_pids,
        IconCapture, IconTarget,
    };
    use launcher_core::runtime::capture::CaptureConfig;
    use launcher_core::runtime::SessionManager;

    #[tokio::main]
    pub async fn main() {
        if std::env::var_os("APP_CATCH_HOME").is_none() {
            eprintln!("请先设置 APP_CATCH_HOME，例如：");
            eprintln!("  APP_CATCH_HOME=/tmp/icon-runtime cargo run -p launcher-core --example icon_window_demo");
            std::process::exit(2);
        }

        // 起一个自己是窗口宿主的进程：PowerShell + WinForms
        let script = "Add-Type -AssemblyName System.Windows.Forms; \
             $f = New-Object System.Windows.Forms.Form; \
             $f.Text = 'app_catch icon demo'; \
             $f.Show(); \
             Start-Sleep -Seconds 25";
        let mut app = AppConfig::new(
            AppId::new("win-demo").expect("合法 id"),
            "窗口图标演示",
            "powershell.exe",
        );
        app.args = vec![
            "-NoProfile".to_string(),
            "-WindowStyle".to_string(),
            "Hidden".to_string(),
            "-Command".to_string(),
            script.to_string(),
        ];

        let manager = SessionManager::from_home(
            std::sync::Arc::new(default_process_backend()),
            CaptureConfig::default(),
        )
        .expect("构造 SessionManager 失败");

        let session = manager.start(&app).await.expect("启动 powershell 失败");
        let pid = session.handle.pid;
        println!("已启动 powershell，pid = {pid}");

        // 轮询等窗口出现
        let mut window_image = None;
        for _ in 0..150 {
            let pids = collect_tree_pids(pid);
            if let Some(image) = window_icon_for_pids(&pids) {
                window_image = Some(image);
                break;
            }
            tokio::time::sleep(Duration::from_millis(100)).await;
        }

        match &window_image {
            Some(image) => println!(
                "从窗口抓到图标: {}x{}，{} 像素字节",
                image.width,
                image.height,
                image.pixels.len()
            ),
            None => println!("窗口图标: 未抓到（可能当前不是可交互桌面会话）"),
        }

        // 走完整 capture 链路（带 pid，所以走窗口分支）
        let exe = resolve_executable("powershell.exe");
        let target = IconTarget::for_app(AppId::new("win-demo").expect("合法 id"))
            .with_executable(exe)
            .with_pid(Some(pid));

        let path = default_icon_capture().capture(&target);
        match &path {
            Some(p) => {
                let meta = std::fs::metadata(p).expect("读元数据失败");
                let image = load_png_file(p).expect("解码失败");
                println!("缓存路径   : {}", p.display());
                println!("文件大小   : {} 字节", meta.len());
                println!("图像尺寸   : {}x{}", image.width, image.height);
            }
            None => println!("capture 结果: None"),
        }

        let _ = manager.stop(session.session_id).await;
        let _ = manager
            .wait_for_exit(session.session_id, Duration::from_secs(30))
            .await;
    }
}
