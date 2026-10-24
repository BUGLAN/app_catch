//! Windows 窗口标题采集：从进程树中找出「主窗口」并取其标题。
//!
//! 定位策略（顺序即优先级）：
//!
//! ```text
//! EnumWindows 遍历全部顶层窗口
//!   └── 逐个过滤 ------------------------------------------------------+
//!         ├── IsWindowVisible?            否 -> 跳过（托盘/隐藏窗口）   |
//!         ├── GetWindowThreadProcessId -> pid 属于目标进程树？ 否 -> 跳过|
//!         └── GetWindowTextW -> 标题为空？               是 -> 跳过     |
//!                                                                        |
//!   对全部候选打分：                                                      |
//!         ├── 带 WS_EX_TOOLWINDOW 的直接淘汰（工具窗口不是主界面）        |
//!         └── 剩余按「窗口面积」取最大者 -------------------------------+
//! ```
//!
//! 为什么按面积取最大：一个 PySide/Electron 应用常见会有多个可见窗口（主窗口、气泡提示、
//! 透明的输入法候选层），主窗口通常是其中面积最大的那个。带 WS_EX_TOOLWINDOW 的窗口
//! 按约定就不是「主界面」（托盘提示、浮动工具条），因此先淘汰再比面积。
//!
//! 本模块只做「尽力而为」的采集：任何一步失败都静默跳过该窗口，
//! 整体找不到时返回 `None`，绝不报错、绝不 panic，也不影响会话继续运行。
//!
//! 资源释放：`EnumWindows` 需要把一个 `Vec` 的可变引用穿过 `LPARAM`，
//! 回调读取后立即归还；窗口句柄由系统持有，本模块不打开任何需要关闭的句柄，
//! 因此不存在泄漏路径。

#![allow(unsafe_code)]

use windows_sys::Win32::Foundation::{HWND, LPARAM, RECT, TRUE};
use windows_sys::Win32::UI::WindowsAndMessaging::{
    EnumWindows, GetWindowLongW, GetWindowRect, GetWindowTextLengthW, GetWindowTextW,
    GetWindowThreadProcessId, IsWindowVisible, GWL_EXSTYLE, WS_EX_TOOLWINDOW,
};

/// 一个候选窗口：窗口句柄 + 标题 + 面积。
struct Candidate {
    /// 窗口句柄；M3 的图标抓取直接对它发 WM_GETICON。
    hwnd: HWND,
    title: String,
    /// 窗口面积（宽 × 高）；`GetWindowRect` 失败时为 0。
    area: i64,
}

/// 枚举时暂存到 `LPARAM` 里的上下文。
///
/// 只放「正在收集的候选列表」与「允许的 pid 集合」，
/// 生命周期严格限定在 `EnumWindows` 调用期间。
struct EnumContext<'a> {
    /// 属于目标进程树的 pid 集合（用切片 + contains，进程树通常只有个位数成员）。
    pids: &'a [u32],
    /// 收集到的候选窗口。
    candidates: Vec<Candidate>,
}

/// 取窗口标题；空标题或调用失败返回 `None`。
///
/// 先 `GetWindowTextLengthW` 拿到长度再一次性分配缓冲区，避免截断或多轮探测。
fn window_title(hwnd: HWND) -> Option<String> {
    // SAFETY: hwnd 由 EnumWindows 回调提供，在回调期间有效。
    let len = unsafe { GetWindowTextLengthW(hwnd) };
    if len <= 0 {
        return None;
    }

    // 容量按「UTF-16 码元数 + 1」预留：GetWindowTextW 写入时最多写 buf.len() 个码元
    let mut buf: Vec<u16> = vec![0; len as usize + 1];
    // SAFETY: buf 至少 len+1 个 u16，hwnd 有效；返回值是实际写入的码元数（不含终止符）。
    let written = unsafe { GetWindowTextW(hwnd, buf.as_mut_ptr(), buf.len() as i32) };
    if written <= 0 {
        return None;
    }

    let text = String::from_utf16_lossy(&buf[..written as usize]);
    let trimmed = text.trim();
    if trimmed.is_empty() {
        return None;
    }
    Some(trimmed.to_string())
}

/// 窗口是否带 `WS_EX_TOOLWINDOW` 扩展样式（是则视为工具窗口，不是主界面）。
fn has_tool_window_style(hwnd: HWND) -> bool {
    // SAFETY: hwnd 在回调期间有效；GWL_EXSTYLE 只读扩展样式，无副作用。
    let ex_style = unsafe { GetWindowLongW(hwnd, GWL_EXSTYLE) } as u32;
    ex_style & WS_EX_TOOLWINDOW != 0
}

/// 窗口面积；`GetWindowRect` 失败或尺寸非正时返回 0。
fn window_area(hwnd: HWND) -> i64 {
    let mut rect = RECT {
        left: 0,
        top: 0,
        right: 0,
        bottom: 0,
    };
    // SAFETY: rect 是栈上有效输出缓冲区，hwnd 在回调期间有效。
    let ok = unsafe { GetWindowRect(hwnd, &mut rect) };
    if ok == 0 {
        return 0;
    }
    let width = (rect.right - rect.left).max(0) as i64;
    let height = (rect.bottom - rect.top).max(0) as i64;
    width * height
}

/// `EnumWindows` 回调：把符合条件的窗口收进上下文。
///
/// 返回 `TRUE` 表示继续枚举——**即使已经找到候选也要继续**，
/// 因为需要比较全部候选的面积才能选出主窗口。
unsafe extern "system" fn enum_proc(hwnd: HWND, lparam: LPARAM) -> windows_sys::core::BOOL {
    // SAFETY: lparam 是 enum_windows 传入的 &mut EnumContext 的裸地址，
    // 其生命周期覆盖整个 EnumWindows 调用；这里以 &mut 重新借出，回调结束即失效。
    let ctx = unsafe { &mut *(lparam as *mut EnumContext<'_>) };

    // 隐藏窗口（托盘、最小化到托盘的提示窗等）不是主界面
    // SAFETY: hwnd 由 EnumWindows 提供，在回调期间有效。
    if unsafe { IsWindowVisible(hwnd) } == 0 {
        return TRUE;
    }

    let mut pid: u32 = 0;
    // SAFETY: pid 是栈上有效输出缓冲区。
    unsafe { GetWindowThreadProcessId(hwnd, &mut pid) };
    if pid == 0 || !ctx.pids.contains(&pid) {
        return TRUE;
    }

    // 工具窗口不是主界面：先淘汰，避免托盘/浮动条被选成标题
    if has_tool_window_style(hwnd) {
        return TRUE;
    }

    let Some(title) = window_title(hwnd) else {
        return TRUE;
    };

    ctx.candidates.push(Candidate {
        hwnd,
        title,
        area: window_area(hwnd),
    });
    TRUE
}

/// 枚举本进程树内全部顶层窗口的候选（标题 + 面积）。
///
/// 返回 `None` 表示 `EnumWindows` 本身调用失败；空 `Vec` 表示枚举成功但没有候选。
fn collect_candidates(pids: &[u32]) -> Option<Vec<Candidate>> {
    if pids.is_empty() {
        return Some(Vec::new());
    }

    let mut ctx = EnumContext {
        pids,
        candidates: Vec::new(),
    };
    // SAFETY: 把 &mut ctx 的地址作为 LPARAM 传给 EnumWindows；EnumWindows 是同步调用，
    // 返回时所有回调都已结束，ctx 的借用随之结束，不存在悬垂指针。
    let ok = unsafe {
        EnumWindows(
            Some(enum_proc),
            &mut ctx as *mut EnumContext<'_> as LPARAM,
        )
    };
    if ok == 0 {
        return None;
    }
    Some(ctx.candidates)
}

/// 取目标进程树中「主窗口」的窗口句柄；找不到返回 `None`（不报错）。
///
/// 与 [`window_title_for_pids`] 共用同一套打分规则（淘汰工具窗口 + 取面积最大者），
/// 保证「采到的标题」和「抓到的图标」来自**同一个窗口**——
/// 否则会出现「标题是主窗口、图标却是托盘气泡」这种自相矛盾的结果。
///
/// 用途：M3 的运行时图标抓取（对主窗口发 WM_GETICON）。
pub fn main_window_for_pids(pids: &[u32]) -> Option<HWND> {
    let candidates = collect_candidates(pids)?;
    candidates.into_iter().max_by_key(|c| c.area).map(|c| c.hwnd)
}

/// 取目标进程树中「主窗口」的标题；找不到返回 `None`（不报错）。
///
/// `pids` 应当包含根 pid 与其全部子孙 pid（见 `descendant_pids`），
/// 因为 GUI 应用常见的情况是「启动器直接拉起的是 shim/启动脚本，真正的窗口属于它的子进程」。
pub fn window_title_for_pids(pids: &[u32]) -> Option<String> {
    let candidates = collect_candidates(pids)?;
    candidates.into_iter().max_by_key(|c| c.area).map(|c| c.title)
}

/// 取指定根 pid 及其子孙进程的「主窗口」标题；找不到返回 `None`。
///
/// 这是给 `StatusProbe` 用的便捷入口。
pub fn window_title_for_pid(root_pid: u32) -> Option<String> {
    if root_pid == 0 {
        return None;
    }
    let mut pids = vec![root_pid];
    pids.extend(super::windows::descendant_pids(root_pid));
    window_title_for_pids(&pids)
}

#[cfg(test)]
mod tests {
    use super::*;

    /// 空 pid 列表不应调用系统 API，直接返回 None。
    #[test]
    fn empty_pid_list_yields_none() {
        assert_eq!(window_title_for_pids(&[]), None);
        assert_eq!(window_title_for_pid(0), None);
        assert_eq!(main_window_for_pids(&[]), None, "空列表不应返回任何窗口句柄");
    }

    /// 当前测试进程通常没有可见的顶层窗口（cargo test 是控制台进程）。
    ///
    /// 这条测试只保证「查不到时安静返回 None」，绝不 panic。
    #[test]
    fn own_process_without_window_yields_none_or_title() {
        let title = window_title_for_pid(std::process::id());
        // 两种结果都合法：CI 上无窗口 -> None；某些终端宿主可能带窗口 -> Some(非空)
        if let Some(t) = title {
            assert!(!t.trim().is_empty(), "返回的标题不应是空白串");
        }
    }
}
