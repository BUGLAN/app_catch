//! Tauri 2 壳层入口：只做 command 薄封装，业务逻辑一律放在 launcher-core。
//!
//! 模块划分（M2）：
//!
//! | 模块 | 职责 |
//! | --- | --- |
//! | [`app_state`] | 共享状态：配置仓库、会话管理器、捕获配置、事件读取器 |
//! | [`commands`] | Tauri 命令薄封装，统一返回中文结构化错误 |
//! | [`dto`] | 命令层数据传输对象与输入校验 |
//! | [`error`] | `launcher-core` 错误 -> 前端可读的 `{ kind, message }` |
//! | [`events_reader`] | 会话事件分片合并 + 增量游标 + 边写边读 |

pub mod app_state;
pub mod commands;
pub mod dto;
pub mod error;
pub mod events_reader;

use tauri::Manager;

use app_state::AppState;

/// 启动 Tauri 应用。
///
/// 初始化顺序：`AppState::bootstrap()`（建目录 + 造管理器）-> 注册命令 -> 跑事件循环。
/// 会话恢复**不在这里**：它是异步的，由前端首屏调用 `recover_sessions` 命令触发，
/// 这样恢复慢（枚举进程表）也不会拖慢窗口显示。
pub fn run() {
    tauri::Builder::default()
        .setup(|app| {
            // 初始化失败（例如主目录不可用）直接把错误交给 Tauri 报错退出，
            // 而不是带着半初始化状态继续跑——那样界面会全是莫名其妙的错误。
            let state = AppState::bootstrap()?;
            app.manage(state);
            Ok(())
        })
        .invoke_handler(tauri::generate_handler![
            commands::core_version,
            commands::app_version,
            commands::list_apps,
            commands::get_app,
            commands::create_app,
            commands::update_app,
            commands::delete_app,
            commands::start_app,
            commands::stop_session,
            commands::restart_session,
            commands::list_active_sessions,
            commands::get_session_status,
            commands::read_session_events,
            commands::get_capture_config,
            commands::set_capture_config,
            commands::recover_sessions,
            commands::get_app_icon,
            commands::default_icon,
        ])
        .run(tauri::generate_context!())
        .expect("启动 Tauri 应用失败");
}
