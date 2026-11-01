//! 窗口控制：自定义标题栏所需的最小 IPC 面。
//!
//! 为什么不用 `@tauri-apps/api/window`：
//! - 前端的窗口按钮只做六件事，走项目自己的命令层可以复用统一的错误形状
//!   （`{ kind, message }`）与中文文案，和 `commands.rs` 的分层约定一致；
//! - capabilities 不需要为此新增任何 `core:window:allow-*` 权限。
//!
//! 平台差异由配置层承担（见 `tauri.conf.json` 与 `tauri.macos.conf.json`）：
//! - Windows / Linux：`decorations: false`，标题栏完全由前端自绘；
//! - macOS：`titleBarStyle: "Overlay"` + `decorations: true`，保留系统窗口的圆角、阴影
//!   与边缘缩放，但用 [`hide_native_traffic_lights`] 把红黄绿藏起来，
//!   窗口按钮与 Windows 一样由前端自绘，两端标题栏长得一样。
//!
//! 最大化状态用**事件**推给前端，而不是让前端轮询：
//! 窗口拖动缩放时 `Resized` 会密集触发，这里只在「结果真的变了」时发一次，
//! 避免把 IPC 变成每秒几十次的噪音。

use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;

use tauri::{Emitter, Manager, WebviewWindow};

use crate::error::CommandError;

/// 最大化状态变化事件名。载荷是 `bool`（true = 已最大化）。
pub const MAXIMIZED_EVENT: &str = "window:maximized-changed";

/// 主窗口 label，与 `tauri.conf.json` 的默认窗口一致。
const MAIN_WINDOW: &str = "main";

/// 把 `tauri::Error` 收敛成前端可读的中文错误。
fn map_err(err: tauri::Error) -> CommandError {
    CommandError::new("window", format!("窗口操作失败：{err}"))
}

/// 最小化窗口。
#[tauri::command]
pub fn window_minimize(window: WebviewWindow) -> Result<(), CommandError> {
    window.minimize().map_err(map_err)
}

/// 最大化窗口（已最大化时不做事，保证幂等）。
#[tauri::command]
pub fn window_maximize(window: WebviewWindow) -> Result<(), CommandError> {
    if window.is_maximized().map_err(map_err)? {
        return Ok(());
    }
    window.maximize().map_err(map_err)
}

/// 还原窗口（未最大化时不做事）。
#[tauri::command]
pub fn window_unmaximize(window: WebviewWindow) -> Result<(), CommandError> {
    if !window.is_maximized().map_err(map_err)? {
        return Ok(());
    }
    window.unmaximize().map_err(map_err)
}

/// 在最大化 / 还原之间切换，返回切换后的状态。
///
/// 返回值让前端可以立即更新图标，不必等 `window:maximized-changed` 事件绕一圈。
#[tauri::command]
pub fn window_toggle_maximize(window: WebviewWindow) -> Result<bool, CommandError> {
    let maximized = window.is_maximized().map_err(map_err)?;
    if maximized {
        window.unmaximize().map_err(map_err)?;
    } else {
        window.maximize().map_err(map_err)?;
    }
    Ok(!maximized)
}

/// 查询是否已最大化（首屏同步图标用）。
#[tauri::command]
pub fn window_is_maximized(window: WebviewWindow) -> Result<bool, CommandError> {
    window.is_maximized().map_err(map_err)
}

/// 关闭窗口。主窗口关闭即退出应用（与系统原生行为一致）。
#[tauri::command]
pub fn window_close(window: WebviewWindow) -> Result<(), CommandError> {
    window.close().map_err(map_err)
}

/// macOS：隐藏系统红黄绿（关闭 / 最小化 / 缩放）。
///
/// 只隐藏按钮、不去掉 `decorations`：无边框窗口在 macOS 上会丢掉圆角，
/// 而 Overlay 标题栏 + 隐藏按钮能保住原生窗口外观。
/// AppKit 在进出全屏、切换焦点后可能把按钮重新摆出来，所以由
/// [`attach_maximize_watcher`] 在这些事件后再调一次；`setHidden` 幂等，重复调用没有代价。
#[cfg(target_os = "macos")]
pub fn hide_native_traffic_lights(window: &WebviewWindow) {
    use objc2_app_kit::{NSWindow, NSWindowButton};

    let Ok(ptr) = window.ns_window() else {
        return;
    };
    // 裸指针不是 Send：按地址传进闭包，到主线程再还原
    let addr = ptr as usize;
    // NSWindow 只能在主线程上碰；setup 与窗口事件回调本来就在主线程，这里再兜一次底
    let _ = window.run_on_main_thread(move || {
        // SAFETY: 地址来自 tao 持有的 NSWindow，窗口存活期间有效；当前在主线程
        let ns_window = unsafe { &*(addr as *const NSWindow) };
        for button in [
            NSWindowButton::CloseButton,
            NSWindowButton::MiniaturizeButton,
            NSWindowButton::ZoomButton,
        ] {
            if let Some(button) = ns_window.standardWindowButton(button) {
                button.setHidden(true);
            }
        }
    });
}

/// 挂上最大化状态监听。
///
/// 由 `lib.rs` 的 setup 调用一次。找不到主窗口时静默跳过：
/// 窗口还没建好不应该让启动失败。
pub fn attach_maximize_watcher(app: &tauri::AppHandle) {
    let Some(window) = app.get_webview_window(MAIN_WINDOW) else {
        return;
    };

    // 初值取当前真实状态，避免第一次 Resized 把「本来就没变」误判成变化
    let last = Arc::new(AtomicBool::new(window.is_maximized().unwrap_or(false)));
    let handle = app.clone();

    #[cfg(target_os = "macos")]
    hide_native_traffic_lights(&window);

    window.on_window_event(move |event| {
        // 进出全屏会触发 Resized，焦点切换也可能让 AppKit 重新显示红黄绿
        #[cfg(target_os = "macos")]
        if matches!(
            event,
            tauri::WindowEvent::Resized(_) | tauri::WindowEvent::Focused(_)
        ) {
            if let Some(window) = handle.get_webview_window(MAIN_WINDOW) {
                hide_native_traffic_lights(&window);
            }
        }

        // 只有尺寸变化可能意味着最大化 / 还原；移动、焦点变化都不需要转发
        if !matches!(event, tauri::WindowEvent::Resized(_)) {
            return;
        }
        let Some(window) = handle.get_webview_window(MAIN_WINDOW) else {
            return;
        };
        let Ok(maximized) = window.is_maximized() else {
            return;
        };
        // swap 返回旧值：旧值 == 新值说明状态没变，不必发事件
        if last.swap(maximized, Ordering::Relaxed) != maximized {
            let _ = handle.emit(MAXIMIZED_EVENT, maximized);
        }
    });
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn maximized_event_name_is_stable() {
        // 前端 `src/lib/api.ts` 里监听的就是这个字符串，改名必须两边同时改
        assert_eq!(MAXIMIZED_EVENT, "window:maximized-changed");
    }

    #[test]
    fn main_window_label_matches_config() {
        assert_eq!(MAIN_WINDOW, "main");
    }
}
