//! Windows 图标抓取：窗口 HICON / exe 资源图标 -> RGBA -> PNG 缓存。
//!
//! # 优先级链（与 docs/ICONS.md 一致）
//!
//! ```text
//! 1. 用户自定义 icon（AppConfig.icon，.png/.ico 且文件有效）
//! 2. 运行中窗口图标（对主窗口发 WM_GETICON -> GCLP_HICON -> GCLP_HICONSM）
//! 3. exe 资源图标（PrivateExtractIconsW -> ExtractIconExW -> SHGetFileInfoW）
//! 4. None（前端用内置默认图兜底）
//! ```
//!
//! 为什么窗口优先于 exe：需求基线原文是「dev 模式下可执行文件常是默认图标，真图标在窗口上」。
//! Python/PySide 开发态、Electron 开发态都是这种情况——exe 是解释器的通用图标，
//! 只有窗口的 WM_GETICON 才带着应用自己 setWindowIcon 设置的真图标。
//!
//! # 资源释放
//!
//! 每一类句柄都有对应的 RAII 包装（[`OwnedIcon`] / [`OwnedBitmap`] / [`OwnedDc`] / [`ScreenDc`]），
//! Drop 时分别调用 DestroyIcon / DeleteObject / DeleteDC / ReleaseDC，
//! 因此错误路径上也不会泄漏，不存在需要手工释放的分支。
//!
//! 特别注意**外来 HICON**：WM_GETICON 与 GetClassLongPtrW 返回的是目标进程的图标句柄，
//! 我们既不能 DestroyIcon 它（会破坏别人的资源），也不能在转换期间假设它一直有效。
//! 因此统一先用 `CopyImage` 复制一份，复制出来的句柄才归本进程所有。
//!
//! # 失败语义
//!
//! 每一步都是「尽力而为」：拿不到就试下一档，全部失败返回 `None`，
//! **不写日志、不返回错误、不 panic**——图标抓不到是预期内的正常情况
//! （Python 服务类应用根本没有窗口）。

#![allow(unsafe_code)]

use std::path::{Path, PathBuf};

use windows_sys::Win32::Foundation::{HWND, LPARAM, WPARAM};
use windows_sys::Win32::System::Com::{
    COINIT_APARTMENTTHREADED, COINIT_DISABLE_OLE1DDE, CoInitializeEx, CoUninitialize,
};
use windows_sys::Win32::Graphics::Gdi::{
    BITMAP, BITMAPINFO, BITMAPINFOHEADER, BI_RGB, CreateCompatibleDC, DIB_RGB_COLORS, DeleteDC,
    DeleteObject, GetDC, GetDIBits, GetObjectW, HDC, HBITMAP, HGDIOBJ, ReleaseDC,
};
use windows_sys::Win32::UI::Shell::{
    ExtractIconExW, SHFILEINFOW, SHGFI_ICON, SHGFI_LARGEICON, SHGetFileInfoW,
};
use windows_sys::Win32::UI::WindowsAndMessaging::{
    CopyImage, DestroyIcon, GCLP_HICON, GCLP_HICONSM, GetClassLongPtrW, GetIconInfo, HICON,
    ICON_BIG, ICON_SMALL, ICONINFO, IMAGE_ICON, LR_DEFAULTCOLOR, PrivateExtractIconsW,
    SMTO_ABORTIFHUNG, SendMessageTimeoutW, WM_GETICON,
};

use crate::platform::icon::{
    CapturedIcon, IconSource, IconTier, RgbaImage, load_png_file, resolve_and_cache,
};
use crate::platform::{IconCapture, IconTarget};

/// 发 WM_GETICON 的超时（毫秒）。
///
/// 目标窗口若正卡死在自己的一轮消息处理里，同步的 SendMessage 会把调用方一起拖住；
/// 用 SendMessageTimeoutW + SMTO_ABORTIFHUNG 保证最多等这么久就放弃。
const ICON_MESSAGE_TIMEOUT_MS: u32 = 300;

/// 向 exe/ico 资源请求图标时的首选边长（Windows 大图标档位）。
const PREFERRED_ICON_SIDE: i32 = 256;

/// 依次尝试的资源图标边长：先要大的，拿不到再退档。
const ICON_SIDES: [i32; 3] = [PREFERRED_ICON_SIDE, 48, 32];

// ---------------------------------------------------------------------------
// RAII 句柄
// ---------------------------------------------------------------------------

/// HICON 的 RAII 包装：Drop 时调用 DestroyIcon。
///
/// 只用于**本进程拥有**的图标（CopyImage / PrivateExtractIconsW / ExtractIconExW /
/// SHGetFileInfoW 返回的那些）。外来句柄必须先 CopyImage 再交给它。
struct OwnedIcon(HICON);

impl OwnedIcon {
    /// 接管一个裸 HICON；空句柄返回 None。
    fn from_raw(raw: HICON) -> Option<Self> {
        if raw.is_null() {
            None
        } else {
            Some(Self(raw))
        }
    }

    /// 取得用于查询的裸句柄（不转移所有权）。
    fn raw(&self) -> HICON {
        self.0
    }
}

impl Drop for OwnedIcon {
    fn drop(&mut self) {
        // SAFETY: 句柄由本结构独占持有，且只销毁一次。
        unsafe {
            DestroyIcon(self.0);
        }
    }
}

/// HBITMAP 的 RAII 包装：Drop 时调用 DeleteObject。
struct OwnedBitmap(HBITMAP);

impl OwnedBitmap {
    fn from_raw(raw: HBITMAP) -> Option<Self> {
        if raw.is_null() {
            None
        } else {
            Some(Self(raw))
        }
    }

    fn raw(&self) -> HBITMAP {
        self.0
    }
}

impl Drop for OwnedBitmap {
    fn drop(&mut self) {
        // SAFETY: 位图句柄由本结构独占持有，只删除一次。
        unsafe {
            DeleteObject(self.0 as HGDIOBJ);
        }
    }
}

/// 兼容 DC 的 RAII 包装：Drop 时调用 DeleteDC。
struct OwnedDc(HDC);

impl OwnedDc {
    fn from_compatible(hdc: HDC) -> Option<Self> {
        // SAFETY: 只创建一个兼容 DC；失败返回空句柄。
        let raw = unsafe { CreateCompatibleDC(hdc) };
        if raw.is_null() {
            None
        } else {
            Some(Self(raw))
        }
    }

    fn raw(&self) -> HDC {
        self.0
    }
}

impl Drop for OwnedDc {
    fn drop(&mut self) {
        // SAFETY: DC 由本结构独占持有，只删除一次。
        unsafe {
            DeleteDC(self.0);
        }
    }
}

/// 屏幕 DC 的 RAII 包装：Drop 时用 ReleaseDC 归还。
struct ScreenDc(HDC);

impl ScreenDc {
    fn acquire() -> Option<Self> {
        // SAFETY: 传空 HWND 取屏幕 DC，失败返回空句柄。
        let raw = unsafe { GetDC(std::ptr::null_mut()) };
        if raw.is_null() {
            None
        } else {
            Some(Self(raw))
        }
    }

    fn raw(&self) -> HDC {
        self.0
    }
}

impl Drop for ScreenDc {
    fn drop(&mut self) {
        // SAFETY: DC 由 GetDC(NULL) 获得，必须用 ReleaseDC(NULL, ...) 归还。
        unsafe {
            ReleaseDC(std::ptr::null_mut(), self.0);
        }
    }
}

// ---------------------------------------------------------------------------
// COM 初始化
// ---------------------------------------------------------------------------

/// 串行化 Shell 图标 API 的进程级互斥锁。
///
/// **为什么需要**（实测结论）：`SHGetFileInfoW` 解析 `.lnk` 时会走 Shell 的
/// 链接解析链路，并发调用在高线程数下会**稳定失败**（本机实测：并发 ≤6 线程
/// 全成功，≥8 线程必现一次失败，且与路径字符串无关）。
/// 失败是静默的（返回 0 / 空 hIcon），调用方无从区分「这文件真没图标」
/// 与「这次撞上了并发缺陷」。
///
/// 代价可以忽略：图标每个应用只抓一次，之后都走磁盘缓存；
/// 真正需要抓取的次数是「应用个数」量级，而不是请求量级。
/// 用一次串行化换掉一整类不确定性，是明显划算的交易。
///
/// 锁中毒按「继续用」处理：图标抓取失败不影响正确性，没必要因为
/// 别的线程 panic 过就让后续所有抓取都失败。
static ICON_API_LOCK: std::sync::Mutex<()> = std::sync::Mutex::new(());

/// 保证当前线程处于 STA 的 RAII 守卫。
///
/// 为什么需要：**解析 `.lnk` 走的是 COM**（Shell Link 对象）。
/// 线程没有初始化 COM 时 `SHGetFileInfoW` 对快捷方式**直接返回 0**，
/// 拿不到任何 hIcon。这个问题在多线程并发时必现（集成测试里稳定复现），
/// 而命令层正是把图标抓取放在 `spawn_blocking` 的线程池上跑的——
/// 那些 worker 线程的 COM 状态不受我们控制，因此**必须在抓取入口自己保证**。
///
/// 释放语义按 COM 的约定处理：
/// - `S_OK`（0，我们初始化的）与 `S_FALSE`（1，别人已初始化）都要 `CoUninitialize`；
/// - `RPC_E_CHANGED_MODE`（0x80010106，线程已在别的 apartment）**不能**反初始化；
/// - 其它失败码：本次就没初始化成功，同样不反初始化。
///
/// 失败不算错误：拿不到 COM 只是「Shell 档位可能失效」，图标抓取仍然尽力而为。
struct ComApartment {
    /// 本线程是否需要在 Drop 时调用 CoUninitialize。
    needs_uninit: bool,
}

impl ComApartment {
    /// 在当前线程尝试进入 STA；无论成败都返回守卫（失败时内部标记为无需释放）。
    fn enter() -> Self {
        // SAFETY: 标准 COM 初始化调用；null 表示使用默认的进程级安全设置。
        let hr = unsafe {
            CoInitializeEx(
                std::ptr::null(),
                (COINIT_APARTMENTTHREADED | COINIT_DISABLE_OLE1DDE) as u32,
            )
        };
        Self {
            // S_OK 与 S_FALSE 都要求配对调用 CoUninitialize
            needs_uninit: hr >= 0,
        }
    }
}

impl Drop for ComApartment {
    fn drop(&mut self) {
        if self.needs_uninit {
            // SAFETY: 与上面成功的 CoInitializeEx 严格配对，且只调用一次。
            unsafe {
                CoUninitialize();
            }
        }
    }
}

// ---------------------------------------------------------------------------
// HICON -> RGBA
// ---------------------------------------------------------------------------

/// 把 HICON 的位图抠成 RGBA 位图。
///
/// 流程：`GetIconInfo` 拿到颜色位图与掩码位图 -> `GetObjectW` 读尺寸
/// -> `GetDIBits` 以 32 位 BGRA 读像素 -> 判 alpha -> BGRA 转 RGBA。
///
/// 两个容易踩的点，都在这里处理掉了：
/// 1. **alpha 判定**：32bpp 图标自带 alpha；但不少老 exe 的图标是 24bpp + AND 掩码，
///    读出来 alpha 全 0。这里用「alpha 通道是否全 0」区分：
///    全 0 说明根本没有 alpha 信息，把整张图按不透明处理
///    （精确合成 1bpp 掩码会把抗锯齿边缘啃成硬边，缩略图反而更难看）；
/// 2. **行序**：DIB 默认自底向上。这里给 `biHeight` 传负数，直接要「自顶向下」，
///    省掉一次手工翻转，也就不存在翻错方向的可能。
fn icon_to_rgba(icon: &OwnedIcon) -> Option<RgbaImage> {
    // SAFETY: ICONINFO 是全 POD 结构，全零是合法初值；icon 句柄有效。
    let mut info: ICONINFO = unsafe { std::mem::zeroed() };
    // SAFETY: info 是栈上有效输出缓冲区，icon 句柄有效。
    if unsafe { GetIconInfo(icon.raw(), &mut info) } == 0 {
        return None;
    }
    // GetIconInfo 会创建两个位图句柄，无论后续成败都必须由我们删除
    let mask = OwnedBitmap::from_raw(info.hbmMask);
    let color = OwnedBitmap::from_raw(info.hbmColor);
    // 掩码位图在这里不当位图用，只借作用域析构；颜色位图必须存在
    let _mask = mask;
    let color = color?;

    // 读取位图尺寸（图标场景下 bmWidth/bmHeight 就是像素尺寸）
    // SAFETY: BITMAP 是全 POD 结构，全零合法。
    let mut bitmap: BITMAP = unsafe { std::mem::zeroed() };
    // SAFETY: bitmap 是栈上有效输出缓冲区，color 句柄有效。
    let read = unsafe {
        GetObjectW(
            color.raw() as HGDIOBJ,
            std::mem::size_of::<BITMAP>() as i32,
            std::ptr::addr_of_mut!(bitmap) as *mut core::ffi::c_void,
        )
    };
    if read == 0 || bitmap.bmWidth <= 0 || bitmap.bmHeight <= 0 {
        return None;
    }
    let width = bitmap.bmWidth as u32;
    let height = bitmap.bmHeight as u32;

    // GetDIBits 需要一个 DC 作为查询上下文
    let screen = ScreenDc::acquire()?;
    let dc = OwnedDc::from_compatible(screen.raw())?;

    // SAFETY: BITMAPINFO 是全 POD 结构，全零合法（bmiColors 对 32bpp 无意义）。
    let mut header: BITMAPINFO = unsafe { std::mem::zeroed() };
    header.bmiHeader = BITMAPINFOHEADER {
        biSize: std::mem::size_of::<BITMAPINFOHEADER>() as u32,
        biWidth: width as i32,
        // 负数 = 要求自顶向下的行序（省掉手工翻转）
        biHeight: -(height as i32),
        biPlanes: 1,
        biBitCount: 32,
        biCompression: BI_RGB,
        biSizeImage: 0,
        biXPelsPerMeter: 0,
        biYPelsPerMeter: 0,
        biClrUsed: 0,
        biClrImportant: 0,
    };

    let mut buffer = vec![0u8; (width as usize) * (height as usize) * 4];
    // SAFETY: buffer 长度与请求的尺寸一致；header 已按约定填好。
    let lines = unsafe {
        GetDIBits(
            dc.raw(),
            color.raw(),
            0,
            height,
            buffer.as_mut_ptr() as *mut core::ffi::c_void,
            &mut header,
            DIB_RGB_COLORS,
        )
    };
    if lines == 0 {
        return None;
    }

    // 是否真的带 alpha：只要有一个像素的 alpha 非 0 就认为有效
    let has_alpha = buffer.chunks_exact(4).any(|px| px[3] != 0);
    for px in buffer.chunks_exact_mut(4) {
        px.swap(0, 2); // BGRA -> RGBA
        if !has_alpha {
            px[3] = 255;
        }
    }

    RgbaImage::new(width, height, buffer)
}

// ---------------------------------------------------------------------------
// 窗口图标
// ---------------------------------------------------------------------------

/// 对窗口发 WM_GETICON，返回**外来** HICON（不拥有，不可 DestroyIcon）。
fn window_icon_message(hwnd: HWND, which: WPARAM) -> Option<HICON> {
    let mut result: usize = 0;
    // SAFETY: hwnd 由窗口枚举得到；超时保证不会无限等待；result 是栈上有效输出。
    let ok = unsafe {
        SendMessageTimeoutW(
            hwnd,
            WM_GETICON,
            which,
            0 as LPARAM,
            SMTO_ABORTIFHUNG,
            ICON_MESSAGE_TIMEOUT_MS,
            &mut result,
        )
    };
    if ok == 0 || result == 0 {
        None
    } else {
        Some(result as HICON)
    }
}

/// 取窗口类的图标（窗口本身没设 WM_GETICON 时的常见来源），返回**外来** HICON。
fn window_class_icon(hwnd: HWND, index: i32) -> Option<HICON> {
    // SAFETY: hwnd 有效；GetClassLongPtrW 只读类信息，无副作用。
    let raw = unsafe { GetClassLongPtrW(hwnd, index) };
    if raw == 0 {
        None
    } else {
        Some(raw as HICON)
    }
}

/// 复制一份外来 HICON，使其变成「本进程拥有、可以 DestroyIcon」的句柄。
fn copy_foreign_icon(raw: HICON) -> Option<OwnedIcon> {
    // SAFETY: raw 是目标进程的图标句柄，CopyImage 只读它并在本进程创建新句柄；
    // cx/cy 传 0 表示保持原尺寸。
    let copied = unsafe { CopyImage(raw, IMAGE_ICON, 0, 0, LR_DEFAULTCOLOR) };
    OwnedIcon::from_raw(copied as HICON)
}

/// 从运行中的窗口抓图标：ICON_BIG -> ICON_SMALL -> 类图标 -> 小类图标。
fn icon_from_window(hwnd: HWND) -> Option<RgbaImage> {
    let candidates = [
        window_icon_message(hwnd, ICON_BIG as WPARAM),
        window_icon_message(hwnd, ICON_SMALL as WPARAM),
        window_class_icon(hwnd, GCLP_HICON),
        window_class_icon(hwnd, GCLP_HICONSM),
    ];
    for raw in candidates.into_iter().flatten() {
        // 外来句柄先复制再转换：复制成功才由 OwnedIcon 负责销毁
        let Some(owned) = copy_foreign_icon(raw) else {
            continue;
        };
        if let Some(image) = icon_to_rgba(&owned) {
            if !image.is_fully_transparent() {
                return Some(image);
            }
        }
    }
    None
}

// ---------------------------------------------------------------------------
// 文件（exe / ico / lnk）图标
// ---------------------------------------------------------------------------

/// 把路径转成 NUL 结尾的 UTF-16 序列；空路径返回空 Vec。
///
/// **必须把 `/` 换成 `\\`**：这是实测踩出来的坑——
/// `SHGetFileInfoW` 对正斜杠路径直接返回 0（拿不到 hIcon），
/// 而本项目里用户填的路径到处都是正斜杠（`C:/Program Files/App/app.exe`、
/// `E:/repo/GPT-SoVITS/main.py` 这种写法在文档与配置示例里就是主流）。
/// `PrivateExtractIconsW` 对正斜杠宽容一些，因此这个 bug 只在
/// 「exe 档位失败、回落到 Shell 档位」时才暴露——正是 .lnk 的场景。
///
/// 其余字节原样保留：不做大小写规范化，也不做路径简写展开，
/// 因为这些都不该由图标抓取来决定。
fn to_wide(path: &Path) -> Vec<u16> {
    use std::os::windows::ffi::OsStrExt;
    let mut wide: Vec<u16> = path
        .as_os_str()
        .encode_wide()
        .map(|unit| if unit == u16::from(b'/') { u16::from(b'\\') } else { unit })
        .collect();
    if wide.is_empty() {
        return wide;
    }
    wide.push(0);
    wide
}

/// 用 PrivateExtractIconsW 按指定尺寸抽一个图标。
fn extract_icon_at(path_wide: &[u16], side: i32) -> Option<RgbaImage> {
    let mut icon: HICON = std::ptr::null_mut();
    // SAFETY: path_wide 以 NUL 结尾；icon 是栈上有效输出；请求 1 个图标。
    let extracted = unsafe {
        PrivateExtractIconsW(
            path_wide.as_ptr(),
            0,
            side,
            side,
            &mut icon,
            std::ptr::null_mut(),
            1,
            0,
        )
    };
    if extracted == 0 {
        return None;
    }
    let owned = OwnedIcon::from_raw(icon)?;
    icon_to_rgba(&owned)
}

/// 用 ExtractIconExW 取大图标（PrivateExtractIconsW 对某些 .ico 会失败的退路）。
fn extract_icon_ex(path_wide: &[u16]) -> Option<RgbaImage> {
    let mut large: HICON = std::ptr::null_mut();
    let mut small: HICON = std::ptr::null_mut();
    // SAFETY: path_wide 以 NUL 结尾；两个输出指针都是栈上有效地址。
    let count = unsafe { ExtractIconExW(path_wide.as_ptr(), 0, &mut large, &mut small, 1) };
    if count == 0 {
        return None;
    }
    // 两个句柄都由 ExtractIconExW 创建，必须都销毁
    let large = OwnedIcon::from_raw(large);
    let _small = OwnedIcon::from_raw(small);
    icon_to_rgba(large.as_ref()?)
}

/// 用 Shell 的「大图标」兜底（能覆盖 .lnk 目标；无图标资源的 exe 会拿到系统通用图标）。
fn shell_icon(path_wide: &[u16]) -> Option<RgbaImage> {
    // SAFETY: SHFILEINFOW 是全 POD 结构，全零合法。
    let mut info: SHFILEINFOW = unsafe { std::mem::zeroed() };
    // SAFETY: path_wide 以 NUL 结尾；info 是栈上有效输出缓冲区，cb 与结构体大小一致。
    let ok = unsafe {
        SHGetFileInfoW(
            path_wide.as_ptr(),
            0,
            &mut info,
            std::mem::size_of::<SHFILEINFOW>() as u32,
            SHGFI_ICON | SHGFI_LARGEICON,
        )
    };
    if ok == 0 {
        return None;
    }
    let owned = OwnedIcon::from_raw(info.hIcon)?;
    icon_to_rgba(&owned)
}

/// 从 exe/ico/lnk 文件抓图标：PrivateExtractIconsW（多档尺寸） -> ExtractIconExW -> Shell。
///
/// 入口处先把当前线程置入 STA：`.lnk` 的解析依赖 COM，未初始化时 Shell 档位必失败。
fn icon_from_file(path: &Path) -> Option<RgbaImage> {
    // 先拿进程级锁再初始化 COM：两者顺序固定，避免与其他线程交错
    let _guard = ICON_API_LOCK.lock().unwrap_or_else(|e| e.into_inner());
    let _com = ComApartment::enter();
    let wide = to_wide(path);
    if wide.is_empty() {
        return None;
    }

    for side in ICON_SIDES {
        if let Some(image) = extract_icon_at(&wide, side) {
            if !image.is_fully_transparent() {
                return Some(image);
            }
        }
    }
    if let Some(image) = extract_icon_ex(&wide) {
        if !image.is_fully_transparent() {
            return Some(image);
        }
    }
    let image = shell_icon(&wide)?;
    if image.is_fully_transparent() {
        return None;
    }
    Some(image)
}

// ---------------------------------------------------------------------------
// 对外可测的原始能力
// ---------------------------------------------------------------------------

/// 从窗口抓图标的原始能力（不含缓存）。
///
/// `pids` 应是「根 pid + 全部子孙 pid」——真正的窗口常常属于子进程。
/// 供 [`WindowsIconCapture`] 与集成测试直接调用。
pub fn window_icon_for_pids(pids: &[u32]) -> Option<RgbaImage> {
    if pids.is_empty() {
        return None;
    }
    let hwnd = crate::platform::window::main_window_for_pids(pids)?;
    icon_from_window(hwnd)
}

/// 从 exe/ico/lnk/png 文件抓图标的原始能力（不含缓存）。
pub fn file_icon(path: &Path) -> Option<RgbaImage> {
    if !path.is_file() {
        return None;
    }
    // .png 由我们自己解码：交给 Shell 反而会拿到「通用图片」图标
    let is_png = path
        .extension()
        .map(|ext| ext.eq_ignore_ascii_case("png"))
        .unwrap_or(false);
    if is_png {
        let image = load_png_file(path)?;
        return if image.is_fully_transparent() {
            None
        } else {
            Some(image)
        };
    }
    icon_from_file(path)
}

// ---------------------------------------------------------------------------
// IconCapture 实现
// ---------------------------------------------------------------------------

/// Windows 图标捕获实现：抓住运行时/资源图标并落 PNG 缓存。
#[derive(Debug, Default, Clone, Copy)]
pub struct WindowsIconCapture;

impl WindowsIconCapture {
    /// 构造图标捕获实现。
    pub fn new() -> Self {
        Self
    }
}

impl IconSource for WindowsIconCapture {
    /// 按优先级链抠图：自定义图标 -> 运行中窗口 -> exe 资源。
    ///
    /// 三档之间是「失败即继续」而不是「失败即放弃」：
    /// 用户填了一个已被删除的图标路径，不该连带让 exe 图标也抓不到。
    ///
    /// 每一档都带上 [`IconTier`]：调用方靠它判断这张图能否覆盖缓存里已有的图。
    ///
    /// **exe 档位会跳过脚本**：`pnpm.cmd` / `npm.cmd` 这类启动脚本没有图标资源，
    /// Shell 只会返回通用批处理图标；把它当成真图标缓存下来，就是
    /// 「所有 Electron 应用显示同一张齿轮图」的原因。此时宁可返回 `None`
    /// 让前端用内置默认图，也不要把错误信息固化进缓存。
    fn produce(&self, target: &IconTarget) -> Option<CapturedIcon> {
        // 1) 用户自定义图标（file_icon 内部会拿 ICON_API_LOCK）
        if let Some(custom) = target.custom_icon.as_deref() {
            if let Some(image) = file_icon(custom) {
                return Some(CapturedIcon::new(image, IconTier::Custom));
            }
        }

        // 2) 运行中窗口图标：基线点名的「dev 模式真图标在窗口上」
        if let Some(pid) = target.pid {
            let pids = crate::platform::collect_tree_pids(pid);
            if let Some(image) = window_icon_for_pids(&pids) {
                return Some(CapturedIcon::new(image, IconTier::Window));
            }
        }

        // 3) exe 资源图标（脚本文件不参与：见上方说明）
        if let Some(exe) = target.executable.as_deref() {
            if !crate::platform::icon::is_script(exe) {
                if let Some(image) = file_icon(exe) {
                    return Some(CapturedIcon::new(image, IconTier::Exe));
                }
            }
        }

        // 4) 兜底：由前端用内置默认图
        None
    }
}

impl IconCapture for WindowsIconCapture {
    /// 抓取并缓存；抓不到返回 `None`（绝不报错、绝不 panic）。
    ///
    /// 缓存与规范化策略在平台无关的 [`resolve_and_cache`] 里，
    /// 这里只提供「从哪抠图」这一半（[`IconSource::produce`]）。
    fn capture(&self, target: &IconTarget) -> Option<PathBuf> {
        let icons_dir = crate::paths::icons_dir().ok()?;
        resolve_and_cache(self, &icons_dir, target)
    }

    /// 忽略缓存强制重抓：会话刚启动、窗口刚出现时用这一次。
    ///
    /// 典型场景正是本次修复的问题：应用第一次打开列表时还没运行，
    /// 缓存里落下的是 `pnpm.cmd` 的通用图标；等窗口起来后必须能把它换掉。
    fn recapture(&self, target: &IconTarget) -> Option<PathBuf> {
        let icons_dir = crate::paths::icons_dir().ok()?;
        crate::platform::icon::resolve_and_cache_forced(self, &icons_dir, target)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::config::AppId;

    /// 不存在 exe 路径：capture 必须安静返回 None，不 panic。
    #[test]
    fn missing_executable_yields_none_without_panic() {
        let dir = tempfile::tempdir().expect("临时目录失败");
        std::env::set_var(crate::paths::HOME_ENV, dir.path());

        let target = IconTarget::for_app(AppId::new("ghost").expect("合法 id"))
            .with_executable(Some(dir.path().join("no-such-app.exe")));
        let capture = WindowsIconCapture::new();
        assert_eq!(capture.capture(&target), None);
        // 也不应留下任何缓存文件
        let cache = crate::paths::icons_dir()
            .expect("icons 目录")
            .join("ghost.png");
        assert!(!cache.exists(), "抓不到时不应写缓存文件: {}", cache.display());

        std::env::remove_var(crate::paths::HOME_ENV);
    }

    /// 空目标（既无窗口也无 exe）→ None，且不写缓存。
    #[test]
    fn empty_target_yields_none() {
        let dir = tempfile::tempdir().expect("临时目录失败");
        std::env::set_var(crate::paths::HOME_ENV, dir.path());
        let target = IconTarget::for_app(AppId::new("empty").expect("合法 id"));
        assert_eq!(WindowsIconCapture::new().capture(&target), None);
        std::env::remove_var(crate::paths::HOME_ENV);
    }

    /// 无 pid 的空进程树不应调用任何窗口 API。
    #[test]
    fn empty_pid_list_has_no_window_icon() {
        assert!(window_icon_for_pids(&[]).is_none());
        assert!(window_icon_for_pids(&[0]).is_none());
    }

    /// 不存在的文件不应产生图标，也不 panic。
    #[test]
    fn missing_file_has_no_icon() {
        assert!(file_icon(Path::new("C:/definitely/not/here.exe")).is_none());
    }

    /// 一个真实的 32 位 RGBA PNG 应能被当作自定义图标读出来。
    #[test]
    fn png_file_is_decoded_directly() {
        let dir = tempfile::tempdir().expect("临时目录失败");
        let path = dir.path().join("icon.png");
        let image = RgbaImage::solid(64, 64, [200, 30, 40, 255]).expect("构造失败");
        let bytes = crate::platform::icon::encode_png(&image).expect("编码失败");
        std::fs::write(&path, bytes).expect("写 PNG 失败");

        let loaded = file_icon(&path).expect("应能读出 PNG 图标");
        assert_eq!((loaded.width, loaded.height), (64, 64));
        assert!(!loaded.is_fully_transparent());
    }

    /// 全透明的 PNG 视为「没抓到」，避免把空图当图标缓存下来。
    #[test]
    fn fully_transparent_png_is_rejected() {
        let dir = tempfile::tempdir().expect("临时目录失败");
        let path = dir.path().join("blank.png");
        let image = RgbaImage::solid(32, 32, [0, 0, 0, 0]).expect("构造失败");
        std::fs::write(&path, crate::platform::icon::encode_png(&image).expect("编码失败"))
            .expect("写 PNG 失败");
        assert!(file_icon(&path).is_none(), "全透明图不应被当作有效图标");
    }
}