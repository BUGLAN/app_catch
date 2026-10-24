//! Tauri 2 壳层入口：只做 command 薄封装，业务逻辑一律放在 launcher-core。

/// 示例 command：返回 launcher-core 的版本号，验证「前端 -> command -> core」链路。
///
/// M0 阶段仅用于打通链路；M1 起真正的配置/进程/会话 command 在此逐步注册。
#[tauri::command]
fn core_version() -> String {
    launcher_core::version().to_string()
}

/// 示例 command：返回应用自身版本号。
#[tauri::command]
fn app_version() -> String {
    env!("CARGO_PKG_VERSION").to_string()
}

#[cfg_attr(mobile, tauri::mobile_entry_point)]
pub fn run() {
    tauri::Builder::default()
        .invoke_handler(tauri::generate_handler![core_version, app_version])
        .run(tauri::generate_context!())
        .expect("启动 Tauri 应用失败");
}
