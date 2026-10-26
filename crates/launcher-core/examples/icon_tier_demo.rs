//! 手动取证：复现并验证「Electron 开发态应用抓到 .cmd 通用图标」的修复。
//!
//! 用法：
//!   APP_CATCH_HOME=/tmp/icon-fix cargo run -p launcher-core --example icon_tier_demo
//!
//! 场景（与真实使用一致）：
//!   command = "pnpm.cmd"，args = ["electron:dev"]，进程正在运行。
//!
//! 验证三件事：
//!   1. 修复前会命中的 \`.cmd\` 路径现在被判定为「不是图标来源」；
//!   2. 应用运行时，窗口档位能抓到真图标并写入缓存；
//!   3. \`recapture\`（强制重抓）能把早先落下的低档位图换掉。

use std::path::PathBuf;

use launcher_core::config::AppId;
use launcher_core::platform::icon::{
    cache_path, is_script, load_png_file, read_tier, resolve_executable, tier_path,
};
use launcher_core::platform::{IconCapture, IconTarget, default_icon_capture};

fn main() {
    // 允许用 argv 覆盖默认值，便于在别的机器上跑
    let command = std::env::args()
        .nth(1)
        .unwrap_or_else(|| "pnpm.cmd".to_string());
    let pid: Option<u32> = std::env::args().nth(2).and_then(|s| s.parse().ok());

    println!("=== 1. 命令行解析 ===");
    println!("command            : {command}");
    let resolved = resolve_executable(&command);
    println!("resolve_executable : {resolved:?}");
    if let Some(path) = &resolved {
        println!(
            "  is_script        : {}  （true 表示不会当成图标来源）",
            is_script(path)
        );
    }

    println!();
    println!("=== 2. 抓取（模拟已运行） ===");
    let app = AppId::new("demo-electron").expect("合法 id");
    let target = IconTarget::for_app(app.clone())
        .with_executable(resolved)
        .with_pid(pid);

    let capture = default_icon_capture();
    match capture.capture(&target) {
        Some(path) => report(&path, &app),
        None => println!("capture: None（将使用前端内置默认图）"),
    }

    if pid.is_some() {
        println!();
        println!("=== 3. 强制重抓（recapture） ===");
        match capture.recapture(&target) {
            Some(path) => report(&path, &app),
            None => println!("recapture: None"),
        }
    } else {
        println!();
        println!("（未提供 pid，跳过 recapture：这段用于验证「窗口出现后能升级」）");
    }
}

fn report(path: &PathBuf, app: &AppId) {
    let image = load_png_file(path).expect("解码失败");
    let meta = std::fs::metadata(path).expect("读元数据失败");
    let icons_dir = path.parent().expect("有父目录");
    println!("缓存路径 : {}", path.display());
    println!("文件大小 : {} 字节", meta.len());
    println!("图像尺寸 : {}x{}", image.width, image.height);
    println!(
        "档位     : {:?}",
        read_tier(&tier_path(icons_dir, app))
    );
    // 顺带断言一下路径自洽：cache_path 与 tier_path 应该同目录同前缀
    debug_assert_eq!(cache_path(icons_dir, app), *path);
}
