//! 窗口控制：自定义标题栏所需的最小 IPC 面。
//!
//! 为什么不用 `@tauri-apps/api/window`：
//! - 前端的窗口按钮只做六件事，走项目自己的命令层可以复用统一的错误形状
//!   （`{ kind, message }`）与中文文案，和 `commands.rs` 的分层约定一致；
//! - capabilities 不需要为此新增任何 `core:window:allow-*` 权限。
//!
//! 平台差异由配置层承担（见 `tauri.conf.json` 与 `tauri.macos.conf.json`）：
//! - Windows / Linux：`decorations: false`，标题栏完全由前端自绘；
//! - macOS：同样 `decorations: false`（tao 建的是 Borderless | Resizable 窗口，没有系统红黄绿），
//!   再加 `transparent: true`，由 [`apply_window_corner_radius`] 把内容视图的 CALayer
//!   裁成 [`WINDOW_CORNER_RADIUS`] 的圆角并重算阴影——外框圆角从此由我们决定，
//!   与内容区（`tokens.css` 的 `--radius-window`）同半径。窗口按钮与 Windows 一样由前端自绘。
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

/// 窗口外框圆角半径（逻辑像素）。
///
/// 必须与 `src/styles/tokens.css` 中 macOS 的 `--radius-window` 相同：内容区圆角对齐的就是它，
/// 由 `tests::corner_radius_matches_css_token` 守住两边不漂移。
/// 非 macOS 平台的外框圆角由系统决定（Windows 11 DWM 为 8px），这个常量只在 macOS 上生效。
pub const WINDOW_CORNER_RADIUS: f64 = 13.0;

/// macOS：把透明的无边框窗口裁成圆角，并让阴影跟着圆角走。
///
/// 做法只用公开 API：
/// 1. 窗口不透明标记关掉、背景色设为 clear —— 圆角外的四个小角才会真正透明；
/// 2. 内容视图开 layer，`cornerRadius` + `masksToBounds` 把 WebView 一起裁成圆角；
/// 3. `invalidateShadow`：透明窗口的阴影按内容的 alpha 形状计算，形状变了要让系统重算。
///
/// 进出全屏、缩放之后 AppKit 可能重建或改动图层，所以由 [`attach_maximize_watcher`]
/// 在 `Resized` 后再调一次；所有设置都是幂等的，重复调用没有代价。
#[cfg(target_os = "macos")]
pub fn apply_window_corner_radius(window: &WebviewWindow) {
    use objc2_app_kit::{NSColor, NSWindow};

    let Ok(ptr) = window.ns_window() else {
        return;
    };
    // 裸指针不是 Send：按地址传进闭包，到主线程再还原
    let addr = ptr as usize;
    // NSWindow 只能在主线程上碰；setup 与窗口事件回调本来就在主线程，这里再兜一次底
    let _ = window.run_on_main_thread(move || {
        // SAFETY: 地址来自 tao 持有的 NSWindow，窗口存活期间有效；当前在主线程
        let ns_window = unsafe { &*(addr as *const NSWindow) };
        ns_window.setOpaque(false);
        ns_window.setBackgroundColor(Some(&NSColor::clearColor()));
        ns_window.setHasShadow(true);
        if let Some(view) = ns_window.contentView() {
            view.setWantsLayer(true);
            if let Some(layer) = view.layer() {
                layer.setCornerRadius(WINDOW_CORNER_RADIUS);
                layer.setMasksToBounds(true);
            }
        }
        ns_window.invalidateShadow();
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
    apply_window_corner_radius(&window);

    window.on_window_event(move |event| {
        // 缩放 / 进出全屏后图层可能被重建，阴影形状也要按新尺寸重算
        #[cfg(target_os = "macos")]
        if matches!(event, tauri::WindowEvent::Resized(_)) {
            if let Some(window) = handle.get_webview_window(MAIN_WINDOW) {
                apply_window_corner_radius(&window);
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

    #[test]
    fn corner_radius_matches_css_token() {
        // 外框圆角（Rust）与内容区圆角（CSS）必须同值，否则又回到「两处圆角不一样」
        let path =
            std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../src/styles/tokens.css");
        let css = std::fs::read_to_string(&path).expect("读取 tokens.css 失败");
        let block_start = css
            .find(":root[data-platform=\"macos\"]")
            .expect("tokens.css 应有 macOS 平台覆盖块");
        let block = &css[block_start..];
        let block = &block[..block.find('}').expect("macOS 覆盖块应闭合")];
        let expected = format!("--radius-window: {}px;", WINDOW_CORNER_RADIUS);
        assert!(
            block.contains(&expected),
            "tokens.css 的 macOS --radius-window 应为 {WINDOW_CORNER_RADIUS}px，实际块:\n{block}"
        );
    }
}
