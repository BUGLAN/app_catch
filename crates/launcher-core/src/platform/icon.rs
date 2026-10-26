//! 图标核心：位图规范化、PNG 编解码、缓存判定与优先级链。
//!
//! 本模块**不含任何平台 API**，因此可以在任意平台上编译与单测；
//! 平台相关的「从哪里拿到图标」由 [`IconSource`] 的实现负责
//! （Windows 见 [`super::icon_windows`]，非 Windows 见 [`super::fallback`]）。
//!
//! 设计要点：
//!
//! 1. **统一出口是 PNG**：不管来源是窗口 HICON、exe 资源还是用户指定的 .png/.ico，
//!    最终都收敛成 [`RgbaImage`] 再编码成 PNG。这样缓存目录里只有一种格式，
//!    前端只需要处理 `<img src="data:image/png;base64,...">`。
//! 2. **不放大**：图标只在超过 [`ICON_MAX_SIZE`] 时等比缩小；原图更小就保持原尺寸。
//!    32×32 的图标硬拉到 256×256 只会糊，反而不如保持清晰的小图。
//! 3. **失败即 `None`**：整条链路上没有任何 [`crate::error::Result`] 会冒泡到调用方——
//!    图标抓不到是**预期内的正常情况**（Python 服务类应用根本没有窗口），
//!    绝不能因此让会话启动失败。
//! 4. **缓存按 app_id 命名**：`<home>/icons/<app_id>.png`。
//!    选它而不是内容 hash，是因为前端与命令层都能直接拼出路径，
//!    且「删掉这个文件就重新抓」是用户能理解、能操作的失效方式。

use std::path::{Path, PathBuf};
use std::time::SystemTime;

use crate::config::AppId;
use crate::error::{AppCatchError, Result};
use crate::platform::IconTarget;

/// 图标规范化的尺寸上限：超过就等比缩到这个边长以内。
///
/// 选 256 的理由：这是 Windows 大图标（jumbo）与高清屏 2x 显示的常见档位，
/// 同时单张 PNG 通常只有几 KB 到几十 KB，直接塞 data URL 也不会撑爆 IPC。
pub const ICON_MAX_SIZE: u32 = 256;

/// 缓存文件的扩展名。
pub const ICON_EXT: &str = "png";

/// 缓存文件名里 app_id 与后缀之间的分隔符（见 [`cache_path`]）。
pub const ICON_FILE_SUFFIX: &str = ".png";

/// 一张已解码的图标位图（RGBA8，行优先，左上角为原点）。
#[derive(Clone, PartialEq, Eq)]
pub struct RgbaImage {
    /// 像素宽度。
    pub width: u32,
    /// 像素高度。
    pub height: u32,
    /// RGBA 像素数据，长度恰好是 `width * height * 4`。
    pub pixels: Vec<u8>,
}

impl std::fmt::Debug for RgbaImage {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        // 不打印像素：几十万个字节会把日志刷爆
        f.debug_struct("RgbaImage")
            .field("width", &self.width)
            .field("height", &self.height)
            .field("bytes", &self.pixels.len())
            .finish()
    }
}

impl RgbaImage {
    /// 用给定尺寸与像素构造；像素长度不匹配时返回 `None`（防御外部数据）。
    pub fn new(width: u32, height: u32, pixels: Vec<u8>) -> Option<Self> {
        if width == 0 || height == 0 {
            return None;
        }
        let expected = (width as usize)
            .checked_mul(height as usize)?
            .checked_mul(4)?;
        if pixels.len() != expected {
            return None;
        }
        Some(Self {
            width,
            height,
            pixels,
        })
    }

    /// 构造一张纯色图（测试与占位用）。
    pub fn solid(width: u32, height: u32, rgba: [u8; 4]) -> Option<Self> {
        let count = (width as usize).checked_mul(height as usize)?;
        let mut pixels = Vec::with_capacity(count * 4);
        for _ in 0..count {
            pixels.extend_from_slice(&rgba);
        }
        Self::new(width, height, pixels)
    }

    /// 是否所有像素都完全透明（用于判定「抓到的其实是一张空图」）。
    pub fn is_fully_transparent(&self) -> bool {
        self.pixels.chunks_exact(4).all(|px| px[3] == 0)
    }

    /// 规范化到「最长边不超过 [`ICON_MAX_SIZE`]」。
    ///
    /// - 已经不超过上限：原样返回（**绝不放大**，避免糊图）；
    /// - 超过上限：按面积平均缩小，保持宽高比。
    pub fn normalize(&self) -> RgbaImage {
        self.fit_within(ICON_MAX_SIZE)
    }

    /// 等比缩放到最长边不超过 `max`；已满足则克隆返回。
    pub fn fit_within(&self, max: u32) -> RgbaImage {
        let longest = self.width.max(self.height);
        if max == 0 || longest <= max {
            return self.clone();
        }
        let scale = max as f64 / longest as f64;
        let width = ((self.width as f64 * scale).round() as u32).max(1);
        let height = ((self.height as f64 * scale).round() as u32).max(1);
        self.resample(width, height)
    }

    /// 面积平均（box filter）重采样到 `width × height`。
    ///
    /// 为什么用面积平均而不是最近邻：图标缩小到 1/8 时最近邻会丢笔画，
    /// 面积平均能保住边缘的过渡像素，缩略图看起来才不会「断线」。
    pub fn resample(&self, width: u32, height: u32) -> RgbaImage {
        if width == self.width && height == self.height {
            return self.clone();
        }
        if width == 0 || height == 0 {
            return self.clone();
        }

        let mut out = vec![0u8; (width as usize) * (height as usize) * 4];
        let x_ratio = self.width as f64 / width as f64;
        let y_ratio = self.height as f64 / height as f64;

        for oy in 0..height {
            let y0 = (oy as f64 * y_ratio).floor() as u32;
            let y1 = (((oy + 1) as f64 * y_ratio).ceil() as u32)
                .min(self.height)
                .max(y0 + 1);
            for ox in 0..width {
                let x0 = (ox as f64 * x_ratio).floor() as u32;
                let x1 = (((ox + 1) as f64 * x_ratio).ceil() as u32)
                    .min(self.width)
                    .max(x0 + 1);

                // 加权累计：颜色按 alpha 预乘后再平均，否则透明像素的黑色会污染边缘
                let mut sum = [0f64; 4];
                let mut weight_sum = 0f64;
                for sy in y0..y1 {
                    for sx in x0..x1 {
                        let idx = ((sy as usize) * (self.width as usize) + sx as usize) * 4;
                        let a = self.pixels[idx + 3] as f64 / 255.0;
                        sum[0] += self.pixels[idx] as f64 * a;
                        sum[1] += self.pixels[idx + 1] as f64 * a;
                        sum[2] += self.pixels[idx + 2] as f64 * a;
                        sum[3] += self.pixels[idx + 3] as f64;
                        weight_sum += 1.0;
                    }
                }
                if weight_sum == 0.0 {
                    continue;
                }
                let alpha = sum[3] / weight_sum;
                let alpha_norm = if alpha <= 0.0 { 0.0 } else { alpha / 255.0 };
                let idx = ((oy as usize) * (width as usize) + ox as usize) * 4;
                if alpha_norm > 0.0 {
                    out[idx] = (sum[0] / weight_sum / alpha_norm).clamp(0.0, 255.0) as u8;
                    out[idx + 1] = (sum[1] / weight_sum / alpha_norm).clamp(0.0, 255.0) as u8;
                    out[idx + 2] = (sum[2] / weight_sum / alpha_norm).clamp(0.0, 255.0) as u8;
                    out[idx + 3] = alpha.clamp(0.0, 255.0) as u8;
                }
            }
        }

        RgbaImage {
            width,
            height,
            pixels: out,
        }
    }
}

/// PNG 魔数（8 字节签名）。
pub const PNG_MAGIC: [u8; 8] = [0x89, 0x50, 0x4e, 0x47, 0x0d, 0x0a, 0x1a, 0x0a];

/// 内置默认图标（256×256 PNG，深色圆角底板 + 主色四宫格）。
///
/// 编译进二进制而不是放磁盘：兜底图必须在**任何情况下**都能用，
/// 不能依赖 `~/.app_catch` 可读、也不能依赖当前工作目录。
///
/// 抓不到图标是明确的产品预期（Python 服务类应用没有窗口），
/// 所以这不是「错误图」，而是正常状态下的默认外观。
pub const DEFAULT_ICON_PNG: &[u8] = include_bytes!("../../assets/default_icon.png");

/// 把 RGBA 位图编码成 PNG 字节。
///
/// 用 `png` crate 而不是 `image`：本 crate 只需要「RGBA → PNG」这一条路径，
/// `image` 会连带引入 jpeg/gif/webp/tiff 等一整套解码器，编译体积与依赖面都大一个数量级。
/// 选择理由与体积对比见 `docs/ICONS.md`。
pub fn encode_png(image: &RgbaImage) -> Result<Vec<u8>> {
    let mut out = Vec::new();
    {
        let mut encoder = png::Encoder::new(&mut out, image.width, image.height);
        encoder.set_color(png::ColorType::Rgba);
        encoder.set_depth(png::BitDepth::Eight);
        let mut writer = encoder
            .write_header()
            .map_err(|e| AppCatchError::Serialize(format!("PNG 头写入失败: {e}")))?;
        writer
            .write_image_data(&image.pixels)
            .map_err(|e| AppCatchError::Serialize(format!("PNG 像素写入失败: {e}")))?;
        writer
            .finish()
            .map_err(|e| AppCatchError::Serialize(format!("PNG 收尾失败: {e}")))?;
    }
    Ok(out)
}

/// 把 PNG 字节解码成 RGBA 位图（支持任意输入色彩类型，统一展开成 RGBA8）。
pub fn decode_png(bytes: &[u8]) -> Result<RgbaImage> {
    // Cursor<&[u8]> 同时满足 BufRead + Seek（png 解码器要能回退读 chunk）
    let mut decoder = png::Decoder::new(std::io::Cursor::new(bytes));
    // 调色板/灰度/16bit 一律展开成 8bit 彩色，省掉调用方自己判断 ColorType
    decoder.set_transformations(png::Transformations::normalize_to_color8());
    let mut reader = decoder
        .read_info()
        .map_err(|e| AppCatchError::Serialize(format!("PNG 解析失败: {e}")))?;

    let buffer_size = reader
        .output_buffer_size()
        .ok_or_else(|| AppCatchError::Serialize("PNG 尺寸超出可处理范围".to_string()))?;
    let mut buf = vec![0u8; buffer_size];
    let info = reader
        .next_frame(&mut buf)
        .map_err(|e| AppCatchError::Serialize(format!("PNG 解码失败: {e}")))?;
    let written = info.buffer_size();

    let pixels = match info.color_type {
        png::ColorType::Rgba => buf[..written].to_vec(),
        png::ColorType::Rgb => {
            let mut rgba = Vec::with_capacity(written / 3 * 4);
            for px in buf[..written].chunks_exact(3) {
                rgba.extend_from_slice(&[px[0], px[1], px[2], 255]);
            }
            rgba
        }
        png::ColorType::GrayscaleAlpha => {
            let mut rgba = Vec::with_capacity(written * 2);
            for px in buf[..written].chunks_exact(2) {
                rgba.extend_from_slice(&[px[0], px[0], px[0], px[1]]);
            }
            rgba
        }
        png::ColorType::Grayscale => {
            let mut rgba = Vec::with_capacity(written * 4);
            for value in &buf[..written] {
                rgba.extend_from_slice(&[*value, *value, *value, 255]);
            }
            rgba
        }
        other => {
            return Err(AppCatchError::Serialize(format!(
                "不支持的 PNG 色彩类型: {other:?}"
            )))
        }
    };

    RgbaImage::new(info.width, info.height, pixels)
        .ok_or_else(|| AppCatchError::Serialize("PNG 像素数据长度与尺寸不匹配".to_string()))
}

/// 读取并解码一个 PNG 文件；任何失败都返回 `None`（绝不冒泡）。
pub fn load_png_file(path: &Path) -> Option<RgbaImage> {
    let bytes = std::fs::read(path).ok()?;
    decode_png(&bytes).ok()
}

/// 判断一段字节是否以 PNG 魔数开头。
pub fn looks_like_png(bytes: &[u8]) -> bool {
    bytes.len() >= PNG_MAGIC.len() && bytes[..PNG_MAGIC.len()] == PNG_MAGIC
}


// ---------------------------------------------------------------------------
// 抓取链路（来源抽象 + 缓存策略）
// ---------------------------------------------------------------------------

/// 图标来源的「档位」——决定这张图可信到什么程度，进而决定它能不能覆盖已有缓存。
///
/// 顺序即优先级：`Custom > Window > Exe`。
/// 缓存里记录档位的意义是**只许升级、不许降级**：
/// 窗口档位抓到的真图标，绝不能被下一次「exe 档位」的通用图标覆盖掉。
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub enum IconTier {
    /// 从 exe 资源或 Shell 抠到的图标。开发态下往往是解释器/脚本的通用图标，可信度最低。
    Exe = 1,
    /// 从运行中窗口的 `WM_GETICON` / 窗口类图标抓到的图标。应用自己设置的，可信度高。
    Window = 2,
    /// 用户在配置里显式指定的图标。始终优先，且永不被自动抓取覆盖。
    Custom = 3,
}

impl IconTier {
    /// 写进 sidecar 的稳定字符串（不要随意改，改了等于让旧缓存全部失效）。
    pub const fn tag(self) -> &'static str {
        match self {
            Self::Exe => "exe",
            Self::Window => "window",
            Self::Custom => "custom",
        }
    }

    /// 从 sidecar 内容解析档位；无法识别时返回 `None`（按「未知档位」处理）。
    pub fn from_tag(raw: &str) -> Option<Self> {
        match raw.trim() {
            "exe" => Some(Self::Exe),
            "window" => Some(Self::Window),
            "custom" => Some(Self::Custom),
            _ => None,
        }
    }
}

/// 一次抓取的结果：位图 + 它来自哪一档。
#[derive(Clone, PartialEq, Eq)]
pub struct CapturedIcon {
    /// 原始位图（尚未规范化）。
    pub image: RgbaImage,
    /// 这张图来自哪一档，决定它能否覆盖已有缓存。
    pub tier: IconTier,
}

impl CapturedIcon {
    /// 用位图与档位构造。
    pub fn new(image: RgbaImage, tier: IconTier) -> Self {
        Self { image, tier }
    }
}

impl std::fmt::Debug for CapturedIcon {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("CapturedIcon")
            .field("image", &self.image)
            .field("tier", &self.tier)
            .finish()
    }
}

/// 图标来源：只负责「从目标里抠出一张原始位图」，**不负责**缓存与规范化。
///
/// 把「从哪拿图标」（平台相关）与「拿不到怎么办、缓存怎么算」
/// （平台无关）拆开，好处有二：
/// 1. 缓存策略可以在任意平台单测，不需要真实 Win32；
/// 2. 「缓存命中就不再抓」这条规则可以被**计数验证**——
///    用一个记账的假来源调用两次，断言它只被问过一次。
pub trait IconSource {
    /// 从目标里抠图标；拿不到返回 `None`（不报错）。
    ///
    /// 返回值带上 [`IconTier`]：调用方据此判断这张图是否值得替换已有缓存。
    fn produce(&self, target: &IconTarget) -> Option<CapturedIcon>;
}

/// 抓取并缓存的完整链路（与平台无关的那一半）。
///
/// 顺序：**缓存命中 -> 让来源产图 -> 规范化 -> 落盘**。
///
/// 返回 `None` 的含义是「没有图标」，调用方应回落到前端内置默认图——
/// 这不是错误，Python 服务类应用（无窗口、exe 是解释器）就是这种情况。
///
/// # 档位规则（M3 修订）
///
/// 光有「文件在就命中」会导致**错图标被永久固化**：某次在应用还没起来时抓到了
/// `pnpm.cmd` 的通用批处理图标，写进缓存后源文件 mtime 永远比缓存旧，
/// 于是失效判定永远是 `Ready`，运行时窗口的真图标再也没机会写进来。
///
/// 所以缓存旁边多存一个 sidecar（`<app_id>.tier`）记录**这张图来自哪一档**：
///
/// | 已有缓存档位 | 本次有无 pid（能否查窗口） | 行为 |
/// | --- | --- | --- |
/// | `Window` / `Custom` | 任意 | 直接复用，不再抓 |
/// | `Exe` / 未知 | 否 | 直接复用（不可能升级） |
/// | `Exe` / 未知 | 是 | **重抓一次**，抓到更高档位才覆盖 |
///
/// 并且**只升不降**：本次抓到的档位不高于已有缓存时保留旧图，
/// 避免「窗口恰好没取到」把真图标降级成解释器图标。
pub fn resolve_and_cache(
    source: &dyn IconSource,
    icons_dir: &Path,
    target: &IconTarget,
) -> Option<PathBuf> {
    // 没有 app_id 就没有缓存身份：直接走兜底，不猜文件名
    let app_id = target.app_id.as_ref()?;
    let cache = cache_path(icons_dir, app_id);
    let tier_file = tier_path(icons_dir, app_id);

    let state = cache_state(&cache, target.executable.as_deref());
    let cached_tier = read_tier(&tier_file);
    // 只有带 pid 时才可能从窗口档位拿到更好的图
    let can_improve = target.pid.is_some();

    if state == CacheState::Ready {
        // 已经是最高档位，或本次根本没法问窗口：直接复用
        let top_tier = matches!(cached_tier, Some(IconTier::Window) | Some(IconTier::Custom));
        if top_tier || !can_improve {
            // 例外：旧版本写下的缓存没有档位信息，而它的来源命令现在被判定为脚本。
            // 这种缓存**几乎必然是**当年把 .cmd 当 exe 抽出来的通用图标，
            // 留着就是继续骗用户；删掉并通过返回 None 让前端显示内置默认图。
            if cached_tier.is_none() && target.executable.is_none() {
                let _ = std::fs::remove_file(&cache);
                let _ = std::fs::remove_file(&tier_file);
                return None;
            }
            return Some(cache);
        }
    }

    let Some(captured) = source.produce(target) else {
        // 抓不到：保留已有缓存（可能是好的，只是这次没抓到），
        // 但如果是「无档位 + 无 exe」这种可疑组合，上面的分支已经处理掉了。
        return if state == CacheState::Ready { Some(cache) } else { None };
    };
    if captured.image.is_fully_transparent() {
        // 全透明的「图标」等于没有图标，不落盘，避免把空图当缓存固化下来
        return None;
    }

    // 只升不降：缓存仍然新鲜、且本次档位没有更高时保留旧图。
    // （缓存 Stale 说明源文件更新过，此时同档位也应该重写。）
    if state == CacheState::Ready {
        if let Some(existing) = cached_tier {
            if existing >= captured.tier {
                return Some(cache);
            }
        }
    }

    let written = write_cache(&cache, &captured.image.normalize())?;
    // sidecar 写失败不影响图标可用性，只是下次会退化成「未知档位」
    write_tier(&tier_file, captured.tier);
    Some(written)
}

/// 忽略缓存、强制重新抓一次（[`resolve_and_cache`] 的 `force` 版本）。
///
/// 语义差别只有一条：**不看 `CacheState`、也不做档位比较**，一定去问来源。
/// 但仍然保留「只升不降」的最后一层保护之外的所有优化都没有了——
/// 这是给「窗口刚起来」那一刻用的：此时我们明确知道有新信息可拿。
///
/// 抓到的档位若低于已有缓存，仍然保留旧图（宁可保守，不要用更差的结果覆盖好的）。
pub fn resolve_and_cache_forced(
    source: &dyn IconSource,
    icons_dir: &Path,
    target: &IconTarget,
) -> Option<PathBuf> {
    let app_id = target.app_id.as_ref()?;
    let cache = cache_path(icons_dir, app_id);
    let tier_file = tier_path(icons_dir, app_id);
    let cached_tier = read_tier(&tier_file);

    let captured = source.produce(target)?;
    if captured.image.is_fully_transparent() {
        return None;
    }
    if let Some(existing) = cached_tier {
        if existing > captured.tier {
            // 已有更高档位的图（例如用户自定义图标）：不要被这次结果降级
            return Some(cache);
        }
    }
    let written = write_cache(&cache, &captured.image.normalize())?;
    write_tier(&tier_file, captured.tier);
    Some(written)
}

// ---------------------------------------------------------------------------
// 缓存
// ---------------------------------------------------------------------------

/// 某个应用的图标缓存文件路径：`<icons_dir>/<app_id>.png`。
pub fn cache_path(icons_dir: &Path, app_id: &AppId) -> PathBuf {
    icons_dir.join(format!("{app_id}{ICON_FILE_SUFFIX}"))
}

/// 缓存档位 sidecar 的扩展名。
pub const ICON_TIER_EXT: &str = "tier";

/// 档位 sidecar 的路径：`<icons_dir>/<app_id>.tier`。
///
/// 单独存一个文件而不是塞进 PNG 的 tEXt 块：PNG 元数据要用 `png` crate 的
/// 编码器 API 才能读写，而 sidecar 一个 `fs::write` 就够了；
/// 更重要的是**旧版本写下的缓存没有 sidecar**，此时读出来是 `None`，
/// 语义正好是「未知档位」——老缓存会被当作可升级，自动修好历史遗留的错图。
pub fn tier_path(icons_dir: &Path, app_id: &AppId) -> PathBuf {
    icons_dir.join(format!("{app_id}.{ICON_TIER_EXT}"))
}

/// 读取缓存档位；文件缺失或内容无法识别都返回 `None`（未知档位）。
pub fn read_tier(path: &Path) -> Option<IconTier> {
    let raw = std::fs::read_to_string(path).ok()?;
    IconTier::from_tag(&raw)
}

/// 写入缓存档位；失败静默忽略（档位只是优化，不该影响图标可用性）。
pub fn write_tier(path: &Path, tier: IconTier) {
    if let Some(parent) = path.parent() {
        let _ = std::fs::create_dir_all(parent);
    }
    let _ = std::fs::write(path, tier.tag());
}

/// 缓存文件的就绪状态。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CacheState {
    /// 文件存在且非空，可以直接用。
    Ready,
    /// 文件不存在或为空（读失败、写坏都会落到这里）。
    Missing,
    /// 文件存在，但比租约里的源文件旧，需要重抓。
    Stale,
}

/// 判断缓存是否可用。
///
/// 规则（与 `docs/ICONS.md` 的「缓存与失效」一致）：
/// 1. 文件不存在或长度为 0 → [`CacheState::Missing`]；
/// 2. 给了 `source` 且它存在、且它的修改时间**晚于**缓存 → [`CacheState::Stale`]
///    （应用更新换了 exe，旧图标就必须重抓）；
/// 3. 其余情况 → [`CacheState::Ready`]。
///
/// 拿不到修改时间（文件系统不支持、权限不足）时按「不过期」处理：
/// 图标是可再生资源，宁可复用旧图，也不要在每次启动时反复跑一遍 Shell API。
pub fn cache_state(cache: &Path, source: Option<&Path>) -> CacheState {
    let Ok(meta) = std::fs::metadata(cache) else {
        return CacheState::Missing;
    };
    if !meta.is_file() || meta.len() == 0 {
        return CacheState::Missing;
    }

    let Some(source) = source else {
        return CacheState::Ready;
    };
    let Ok(source_meta) = std::fs::metadata(source) else {
        // 源文件不在了（例如 exe 被卸载）：缓存仍然可用
        return CacheState::Ready;
    };

    match (meta.modified(), source_meta.modified()) {
        (Ok(cache_mtime), Ok(source_mtime)) if source_mtime > cache_mtime => CacheState::Stale,
        _ => CacheState::Ready,
    }
}

/// 把规范化后的图标写进缓存目录；成功返回缓存路径。
///
/// 写盘走同目录临时文件 + rename，避免并发读到半截图。
/// 任何一步失败都返回 `None`（图标是可再生资源，不该为此报错中断流程）。
pub fn write_cache(cache: &Path, image: &RgbaImage) -> Option<PathBuf> {
    let png = encode_png(image).ok()?;
    let parent = cache.parent()?;
    std::fs::create_dir_all(parent).ok()?;

    let unique = SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_nanos())
        .unwrap_or(0);
    let tmp = parent.join(format!(
        "{}.tmp-{}-{}",
        cache.file_name()?.to_string_lossy(),
        std::process::id(),
        unique
    ));

    if std::fs::write(&tmp, &png).is_err() {
        let _ = std::fs::remove_file(&tmp);
        return None;
    }
    if std::fs::rename(&tmp, cache).is_err() {
        let _ = std::fs::remove_file(&tmp);
        return None;
    }
    Some(cache.to_path_buf())
}

/// 把字节编码成 `data:image/png;base64,...`。
///
/// 为什么走 data URL 而不是 Tauri 的 `asset:` 协议，见 `docs/ICONS.md`：
/// 图标本身只有几 KB，走 base64 省掉了 asset 协议的 scope 配置与 capability 授权，
/// 少一个运行时才暴露的配置错误面。
pub fn to_data_url(png_bytes: &[u8]) -> String {
    format!("data:image/png;base64,{}", base64_encode(png_bytes))
}

/// 最小编码器：标准 base64（带 `=` 填充），不引入额外依赖。
fn base64_encode(input: &[u8]) -> String {
    const TABLE: &[u8; 64] = b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789+/";
    let mut out = String::with_capacity(input.len().div_ceil(3) * 4);
    for chunk in input.chunks(3) {
        let b0 = chunk[0] as u32;
        let b1 = *chunk.get(1).unwrap_or(&0) as u32;
        let b2 = *chunk.get(2).unwrap_or(&0) as u32;
        let triple = (b0 << 16) | (b1 << 8) | b2;
        out.push(TABLE[(triple >> 18) as usize & 0x3f] as char);
        out.push(TABLE[(triple >> 12) as usize & 0x3f] as char);
        out.push(if chunk.len() > 1 {
            TABLE[(triple >> 6) as usize & 0x3f] as char
        } else {
            '='
        });
        out.push(if chunk.len() > 2 {
            TABLE[triple as usize & 0x3f] as char
        } else {
            '='
        });
    }
    out
}


// ---------------------------------------------------------------------------
// 会话可执行文件解析（供图标抓取用）
// ---------------------------------------------------------------------------

/// 这些扩展名**不是图标来源**：Windows 对脚本文件没有内嵌图标资源，
/// Shell 只会返回「通用批处理/脚本」图标（那张「窗口+齿轮」的灰白图）。
///
/// 实测（2026-10-25）：`pnpm.cmd` 经 `SHGetFileInfoW` / `ExtractAssociatedIcon`
/// 得到的就是这张通用图，与 `node.exe`（绿色六边形）、`electron.exe`（原子标志）
/// 的真实图标完全不同。所以脚本不能当成 exe 去抽图标——
/// 应当视为「没有 exe」，让调用方退到窗口档位或用内置默认图。
const SCRIPT_EXTENSIONS: [&str; 6] = ["cmd", "bat", "ps1", "vbs", "js", "sh"];

/// 路径是否是「脚本」而不是可执行文件（按扩展名判断）。
pub fn is_script(path: &Path) -> bool {
    path.extension()
        .and_then(|ext| ext.to_str())
        .map(|ext| SCRIPT_EXTENSIONS.contains(&ext.to_ascii_lowercase().as_str()))
        .unwrap_or(false)
}

/// 从应用配置里推测「真正的可执行文件路径」。
///
/// 图标抓取需要 exe 才能取资源图标，但用户填的命令形态很杂：
///
/// | 用户填的命令 | 期望解析结果 |
/// | --- | --- |
/// | \`C:\Program Files\App\app.exe\` | 原样（绝对路径且存在） |
/// | \`notepad\` | \`C:\Windows\System32\notepad.exe\` |
/// | \`python\` | \`...\Python311\python.exe\` |
///
/// 解析不出来的形态（相对路径、脚本名、解释器参数里的路径）一并返回 \`None\`：
/// 图标是可再生资源，猜错比猜不出来的代价更大。
pub fn resolve_executable(command: &str) -> Option<PathBuf> {
    let trimmed = command.trim();
    if trimmed.is_empty() {
        return None;
    }
    let path = Path::new(trimmed);

    // 1) 已经是存在的文件：直接用
    //    **脚本除外**：Windows 不为 .cmd/.bat 提供图标资源，抽出来只会是通用图标。
    if path.is_file() {
        return if is_script(path) {
            None
        } else {
            Some(path.to_path_buf())
        };
    }

    // 2) 带路径分隔符但文件不存在：不再猜测（可能是相对路径或写错了）
    if trimmed.contains('\\') || trimmed.contains('/') {
        return None;
    }

    // 3) 纯命令名：按 Windows 的搜索顺序在 PATH 与两个系统目录里找
    let with_ext = if path.extension().is_some() {
        trimmed.to_string()
    } else {
        format!("{trimmed}.exe")
    };

    // PATH 里找到的也可能是个脚本（pnpm.cmd 就在这里命中）：
    // 同样不能当成图标来源，直接返回 None 让调用方退到窗口档位/兜底图。
    if let Some(found) = search_in_path(&with_ext) {
        return if is_script(&found) { None } else { Some(found) };
    }
    for dir in ["C:/Windows/System32", "C:/Windows"] {
        let candidate = Path::new(dir).join(&with_ext);
        if candidate.is_file() && !is_script(&candidate) {
            return Some(candidate);
        }
    }
    None
}

/// 在 \`PATH\` 各目录里找同名文件。
fn search_in_path(file_name: &str) -> Option<PathBuf> {
    let path_var = std::env::var_os("PATH")?;
    for dir in std::env::split_paths(&path_var) {
        if dir.as_os_str().is_empty() {
            continue;
        }
        let candidate = dir.join(file_name);
        if candidate.is_file() {
            return Some(candidate);
        }
    }
    None
}

#[cfg(test)]
mod executable_tests {
    use super::*;

    #[test]
    fn absolute_existing_file_is_returned_as_is() {
        let dir = tempfile::tempdir().expect("临时目录失败");
        let exe = dir.path().join("app.exe");
        std::fs::write(&exe, b"stub").expect("写桩失败");

        let resolved = resolve_executable(&exe.to_string_lossy()).expect("应解析出路径");
        assert_eq!(resolved, exe);
    }

    #[test]
    fn bare_command_with_separator_but_missing_is_not_guessed() {
        assert!(resolve_executable("C:/no/such/dir/app.exe").is_none());
        assert!(resolve_executable("relative/app.exe").is_none());
    }

    #[test]
    fn empty_command_has_no_executable() {
        assert!(resolve_executable("").is_none());
        assert!(resolve_executable("   ").is_none());
    }

    /// 脚本文件（.cmd/.bat/.ps1…）不是图标来源：必须解析成 None。
    ///
    /// 守住实测 bug：`pnpm.cmd` 被当成 exe，抽出来的是 Windows 通用批处理图标，
    /// 导致三个 Electron 应用显示同一张「窗口+齿轮」图。
    #[test]
    fn script_files_are_never_icon_sources() {
        for name in ["pnpm.cmd", "run.bat", "build.ps1", "x.vbs", "y.sh", "a.CMD"] {
            assert!(is_script(Path::new(name)), "{name} 应被识别为脚本");
        }
        for name in ["app.exe", "notepad.exe", "lib.dll", "noext"] {
            assert!(!is_script(Path::new(name)), "{name} 不应被识别为脚本");
        }
    }

    #[test]
    fn existing_script_file_resolves_to_none() {
        let dir = tempfile::tempdir().expect("临时目录失败");
        let cmd = dir.path().join("pnpm.cmd");
        std::fs::write(&cmd, b"@echo off").expect("写脚本桩失败");
        assert!(
            resolve_executable(&cmd.to_string_lossy()).is_none(),
            "存在的 .cmd 文件不应被当成图标来源"
        );

        // 同样内容换成 .exe 就可以：证明过滤的是扩展名，不是文件内容
        let exe = dir.path().join("app.exe");
        std::fs::write(&exe, b"stub").expect("写 exe 桩失败");
        assert!(resolve_executable(&exe.to_string_lossy()).is_some());
    }
    /// Windows 上 \`cmd\` 一定能在 System32 或 PATH 里找到。
    #[cfg(windows)]
    #[test]
    fn bare_windows_command_resolves_through_path() {
        let resolved = resolve_executable("cmd").expect("cmd 应能在 PATH 里找到");
        assert!(resolved.is_file());
        assert!(
            resolved
                .file_name()
                .map(|n| n.to_string_lossy().to_lowercase().starts_with("cmd."))
                .unwrap_or(false),
            "应解析到 cmd.exe，实际: {}",
            resolved.display()
        );
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// 构造一张 4×4 的不透明红色图。
    fn red_4x4() -> RgbaImage {
        RgbaImage::solid(4, 4, [255, 0, 0, 255]).expect("构造纯色图失败")
    }

    #[test]
    fn rgba_image_rejects_mismatched_pixel_count() {
        assert!(RgbaImage::new(2, 2, vec![0; 15]).is_none(), "长度不对应被拒绝");
        assert!(RgbaImage::new(2, 2, vec![0; 16]).is_some());
        assert!(RgbaImage::new(0, 2, Vec::new()).is_none(), "零尺寸应被拒绝");
    }

    /// PNG 编码路径：编出来的必须是带正确宽高的合法 PNG，且能无损读回。
    #[test]
    fn encode_png_emits_valid_png_with_expected_size() {
        let image = red_4x4();
        let bytes = encode_png(&image).expect("编码 PNG 失败");

        assert!(looks_like_png(&bytes), "应带 PNG 魔数");
        // IHDR 紧跟 8 字节魔数 + 4 字节长度 + "IHDR"，宽高在大端第 16/20 字节
        assert_eq!(&bytes[12..16], b"IHDR", "第一个 chunk 必须是 IHDR");
        let width = u32::from_be_bytes([bytes[16], bytes[17], bytes[18], bytes[19]]);
        let height = u32::from_be_bytes([bytes[20], bytes[21], bytes[22], bytes[23]]);
        assert_eq!((width, height), (4, 4), "PNG 头里的尺寸应与输入一致");

        // 往返解码后像素必须逐字节相同
        let back = decode_png(&bytes).expect("解码自己编出的 PNG 失败");
        assert_eq!(back.width, 4);
        assert_eq!(back.height, 4);
        assert_eq!(back.pixels, image.pixels, "RGBA 数据应无损往返");
    }

    #[test]
    fn decode_png_rejects_garbage() {
        assert!(decode_png(b"not a png at all").is_err());
        assert!(!looks_like_png(b"not a png at all"));
    }

    #[test]
    fn normalize_never_upscales_small_icons() {
        let small = RgbaImage::solid(32, 32, [1, 2, 3, 255]).expect("构造失败");
        let normalized = small.normalize();
        assert_eq!(
            (normalized.width, normalized.height),
            (32, 32),
            "32×32 的图标必须原样保留，绝不放大到 256"
        );
    }

    #[test]
    fn normalize_scales_down_large_icons_keeping_aspect() {
        let wide = RgbaImage::solid(1024, 512, [9, 9, 9, 255]).expect("构造失败");
        let normalized = wide.normalize();
        assert_eq!(normalized.width, ICON_MAX_SIZE);
        assert_eq!(normalized.height, ICON_MAX_SIZE / 2, "宽高比应保持不变");
        assert_eq!(
            normalized.pixels.len(),
            (normalized.width as usize) * (normalized.height as usize) * 4
        );
    }

    #[test]
    fn resample_keeps_uniform_color_uniform() {
        let image = RgbaImage::solid(64, 64, [10, 20, 30, 255]).expect("构造失败");
        let small = image.resample(8, 8);
        assert_eq!((small.width, small.height), (8, 8));
        for px in small.pixels.chunks_exact(4) {
            assert_eq!(px, &[10, 20, 30, 255], "纯色图缩放后仍应是同一颜色");
        }
    }

    #[test]
    fn fully_transparent_detection_works() {
        let blank = RgbaImage::solid(2, 2, [0, 0, 0, 0]).expect("构造失败");
        assert!(blank.is_fully_transparent());
        assert!(!red_4x4().is_fully_transparent());
    }

    #[test]
    fn base64_matches_known_vectors() {
        assert_eq!(base64_encode(b""), "");
        assert_eq!(base64_encode(b"f"), "Zg==");
        assert_eq!(base64_encode(b"fo"), "Zm8=");
        assert_eq!(base64_encode(b"foo"), "Zm9v");
        assert_eq!(base64_encode(b"foobar"), "Zm9vYmFy");
        // PNG 魔数的 base64 前缀（用于断言 data URL 可被浏览器识别）
        assert!(to_data_url(&PNG_MAGIC).starts_with("data:image/png;base64,iVBORw0KGgo"));
    }

    #[test]
    fn cache_path_uses_app_id() {
        let dir = Path::new("C:/home/icons");
        let id = AppId::new("gpt-sovits").expect("合法 id");
        assert_eq!(
            cache_path(dir, &id),
            Path::new("C:/home/icons/gpt-sovits.png")
        );
    }

    #[test]
    fn cache_state_reports_missing_for_absent_or_empty_file() {
        let dir = tempfile::tempdir().expect("临时目录失败");
        let cache = dir.path().join("a.png");
        assert_eq!(cache_state(&cache, None), CacheState::Missing);

        std::fs::write(&cache, b"").expect("写空文件失败");
        assert_eq!(
            cache_state(&cache, None),
            CacheState::Missing,
            "空文件视为未缓存"
        );
    }

    #[test]
    fn cache_state_is_ready_without_source() {
        let dir = tempfile::tempdir().expect("临时目录失败");
        let cache = dir.path().join("a.png");
        std::fs::write(&cache, b"x").expect("写失败");
        assert_eq!(cache_state(&cache, None), CacheState::Ready);
    }

    #[test]
    fn cache_state_marks_stale_when_source_is_newer() {
        let dir = tempfile::tempdir().expect("临时目录失败");
        let cache = dir.path().join("a.png");
        let source = dir.path().join("app.exe");

        std::fs::write(&cache, b"x").expect("写缓存失败");
        std::fs::write(&source, b"y").expect("写源文件失败");

        // 把缓存的修改时间往前拨一小时，模拟「exe 更新过」
        let old = SystemTime::now() - std::time::Duration::from_secs(3600);
        set_mtime(&cache, old);

        assert_eq!(
            cache_state(&cache, Some(&source)),
            CacheState::Stale,
            "源文件更新后缓存必须失效"
        );

        // 反过来：缓存比源文件新 -> 命中
        set_mtime(&source, old - std::time::Duration::from_secs(60));
        assert_eq!(cache_state(&cache, Some(&source)), CacheState::Ready);
    }

    #[test]
    fn cache_state_is_ready_when_source_disappeared() {
        let dir = tempfile::tempdir().expect("临时目录失败");
        let cache = dir.path().join("a.png");
        std::fs::write(&cache, b"x").expect("写缓存失败");
        assert_eq!(
            cache_state(&cache, Some(&dir.path().join("gone.exe"))),
            CacheState::Ready,
            "源文件消失时保留既有缓存"
        );
    }

    #[test]
    fn write_cache_produces_decodable_png_and_no_temp_leftovers() {
        let dir = tempfile::tempdir().expect("临时目录失败");
        let cache = dir.path().join("icons").join("demo.png");

        let image = RgbaImage::solid(16, 16, [0, 128, 255, 255]).expect("构造失败");
        let written = write_cache(&cache, &image).expect("写缓存失败");
        assert_eq!(written, cache);
        assert!(cache.is_file());

        let back = load_png_file(&cache).expect("读回缓存的 PNG 失败");
        assert_eq!((back.width, back.height), (16, 16));

        let leftovers: Vec<String> = std::fs::read_dir(cache.parent().expect("有父目录"))
            .expect("读目录失败")
            .filter_map(|e| e.ok())
            .map(|e| e.file_name().to_string_lossy().into_owned())
            .filter(|n| n.contains(".tmp-"))
            .collect();
        assert!(leftovers.is_empty(), "不应残留临时文件: {leftovers:?}");
    }


    // -----------------------------------------------------------------------
    // 抓取链路：缓存命中 / 失败不缓存 / 规范化入库
    // -----------------------------------------------------------------------

    /// 记账用的假来源：记录被调用次数，返回固定位图与档位。
    struct CountingSource {
        calls: std::sync::atomic::AtomicUsize,
        image: Option<RgbaImage>,
        tier: IconTier,
    }

    impl CountingSource {
        fn new(image: Option<RgbaImage>) -> Self {
            Self::with_tier(image, IconTier::Exe)
        }

        fn with_tier(image: Option<RgbaImage>, tier: IconTier) -> Self {
            Self {
                calls: std::sync::atomic::AtomicUsize::new(0),
                image,
                tier,
            }
        }

        fn calls(&self) -> usize {
            self.calls.load(std::sync::atomic::Ordering::Relaxed)
        }
    }

    impl IconSource for CountingSource {
        fn produce(&self, _target: &IconTarget) -> Option<CapturedIcon> {
            self.calls
                .fetch_add(1, std::sync::atomic::Ordering::Relaxed);
            self.image.clone().map(|image| CapturedIcon::new(image, self.tier))
        }
    }

    fn demo_target() -> IconTarget {
        IconTarget::for_app(AppId::new("demo").expect("合法 id"))
    }

    /// 核心契约：同一 app_id 二次调用**不再问来源**，直接命中缓存。
    #[test]
    fn second_capture_hits_cache_without_asking_source() {
        let dir = tempfile::tempdir().expect("临时目录失败");
        let icons = dir.path().join("icons");
        let image = RgbaImage::solid(64, 64, [12, 34, 56, 255]).expect("构造失败");
        let source = CountingSource::new(Some(image));

        let first = resolve_and_cache(&source, &icons, &demo_target()).expect("首次应抓到");
        assert_eq!(source.calls(), 1, "首次必须真的问一次来源");
        assert!(first.is_file());

        // 二次调用：来源不应再被调用
        let second = resolve_and_cache(&source, &icons, &demo_target()).expect("二次应命中缓存");
        assert_eq!(first, second, "两次应返回同一个缓存路径");
        assert_eq!(source.calls(), 1, "缓存命中时不得重复抓取");

        // 三次同理：证明不是「只缓存一次」的偶然
        resolve_and_cache(&source, &icons, &demo_target()).expect("三次应命中缓存");
        assert_eq!(source.calls(), 1);
    }

    /// 抓不到时：不写缓存、返回 None，二次调用仍会再试一次（不会缓存「失败」）。
    #[test]
    fn failed_capture_caches_nothing_and_retries_next_time() {
        let dir = tempfile::tempdir().expect("临时目录失败");
        let icons = dir.path().join("icons");
        let source = CountingSource::new(None);

        assert!(resolve_and_cache(&source, &icons, &demo_target()).is_none());
        assert_eq!(source.calls(), 1);
        assert!(!icons.join("demo.png").exists(), "失败不应留下缓存文件");

        // 失败不缓存：下次还会再试（窗口可能这会儿才建出来）
        assert!(resolve_and_cache(&source, &icons, &demo_target()).is_none());
        assert_eq!(source.calls(), 2, "失败后应允许重试");
    }

    /// 全透明的「图标」不落盘：等于没有图标。
    #[test]
    fn fully_transparent_source_result_is_not_cached() {
        let dir = tempfile::tempdir().expect("临时目录失败");
        let icons = dir.path().join("icons");
        let blank = RgbaImage::solid(32, 32, [0, 0, 0, 0]).expect("构造失败");
        let source = CountingSource::new(Some(blank));

        assert!(resolve_and_cache(&source, &icons, &demo_target()).is_none());
        assert!(!icons.join("demo.png").exists());
    }

    /// 没有 app_id 时直接返回 None，连来源都不问（没有缓存身份就不抓）。
    #[test]
    fn target_without_app_id_skips_source_entirely() {
        let dir = tempfile::tempdir().expect("临时目录失败");
        let icons = dir.path().join("icons");
        let image = RgbaImage::solid(8, 8, [1, 1, 1, 255]).expect("构造失败");
        let source = CountingSource::new(Some(image));

        let target = IconTarget::from_executable("C:/x/app.exe");
        assert!(resolve_and_cache(&source, &icons, &target).is_none());
        assert_eq!(source.calls(), 0, "无 app_id 时不应尝试抓取");
    }

    /// 缓存写入的是**规范化后**的图：大图入库后应是 256 以内。
    #[test]
    fn cache_stores_normalized_size() {
        let dir = tempfile::tempdir().expect("临时目录失败");
        let icons = dir.path().join("icons");
        let huge = RgbaImage::solid(1024, 1024, [7, 8, 9, 255]).expect("构造失败");
        let source = CountingSource::new(Some(huge));

        let path = resolve_and_cache(&source, &icons, &demo_target()).expect("应抓到");
        let stored = load_png_file(&path).expect("缓存应是可解码 PNG");
        assert_eq!(stored.width, ICON_MAX_SIZE);
        assert_eq!(stored.height, ICON_MAX_SIZE);
        assert!(looks_like_png(&std::fs::read(&path).expect("读缓存失败")));
    }

    /// 源文件更新后（mtime 变新）缓存失效并重新抓取。
    #[test]
    fn newer_source_invalidates_cache_and_refetches() {
        let dir = tempfile::tempdir().expect("临时目录失败");
        let icons = dir.path().join("icons");
        let exe = dir.path().join("app.exe");
        std::fs::write(&exe, b"stub").expect("写 exe 桩失败");

        let image = RgbaImage::solid(16, 16, [3, 3, 3, 255]).expect("构造失败");
        let source = CountingSource::new(Some(image));
        let target = IconTarget::for_app(AppId::new("demo").expect("合法 id"))
            .with_executable(Some(exe.clone()));

        resolve_and_cache(&source, &icons, &target).expect("首次应抓到");
        assert_eq!(source.calls(), 1);

        // 把缓存改老，模拟「exe 之后又被更新过」
        let cache = icons.join("demo.png");
        let old = SystemTime::now() - std::time::Duration::from_secs(7200);
        let file = std::fs::OpenOptions::new()
            .write(true)
            .open(&cache)
            .expect("打开缓存失败");
        file.set_modified(old).expect("设置 mtime 失败");

        resolve_and_cache(&source, &icons, &target).expect("应重新抓取");
        assert_eq!(source.calls(), 2, "源文件更新后必须重抓");
    }

    /// 设置修改时间（测试专用；失败时直接 panic，因为测试环境应当支持）。
    fn set_mtime(path: &Path, when: SystemTime) {
        let file = std::fs::OpenOptions::new()
            .write(true)
            .open(path)
            .expect("打开文件失败");
        file.set_modified(when).expect("设置修改时间失败");
    }
}
#[cfg(test)]
mod tier_tests {
    use super::*;
    use std::sync::atomic::{AtomicUsize, Ordering};

    /// 记账来源：可指定档位，并记录调用次数。
    struct TierSource {
        calls: AtomicUsize,
        tier: IconTier,
        color: [u8; 4],
    }

    impl TierSource {
        fn new(tier: IconTier, color: [u8; 4]) -> Self {
            Self {
                calls: AtomicUsize::new(0),
                tier,
                color,
            }
        }

        fn calls(&self) -> usize {
            self.calls.load(Ordering::Relaxed)
        }
    }

    impl IconSource for TierSource {
        fn produce(&self, _target: &IconTarget) -> Option<CapturedIcon> {
            self.calls.fetch_add(1, Ordering::Relaxed);
            Some(CapturedIcon::new(
                RgbaImage::solid(8, 8, self.color).expect("构造失败"),
                self.tier,
            ))
        }
    }

    fn target(app: &str, pid: Option<u32>) -> IconTarget {
        IconTarget::for_app(AppId::new(app).expect("合法 id")).with_pid(pid)
    }

    /// 核心修复：低档位图先入库，之后有窗口可查时必须能升档覆盖。
    ///
    /// 复现的正是实测 bug 的顺序：应用未运行时抓到 exe 档位（.cmd 通用图标），
    /// 之后应用跑起来、窗口出现，必须允许窗口档位把它换掉。
    #[test]
    fn higher_tier_replaces_cached_lower_tier() {
        let dir = tempfile::tempdir().expect("临时目录失败");
        let icons = dir.path().join("icons");

        // 第一步：没有 pid（应用还没起），只能拿到 exe 档位
        let exe_source = TierSource::new(IconTier::Exe, [10, 10, 10, 255]);
        let cold = target("att", None);
        let path = resolve_and_cache(&exe_source, &icons, &cold).expect("应抓到");
        assert_eq!(read_tier(&tier_path(&icons, &AppId::new("att").unwrap())), Some(IconTier::Exe));
        let low = load_png_file(&path).expect("读缓存");
        assert_eq!(low.pixels[0..4], [10, 10, 10, 255]);

        // 第二步：应用跑起来了（有 pid），窗口档位应覆盖它
        let window_source = TierSource::new(IconTier::Window, [200, 30, 40, 255]);
        let warm = target("att", Some(4242));
        let path2 = resolve_and_cache(&window_source, &icons, &warm).expect("应抓到");
        assert_eq!(path2, path, "缓存路径不变");
        assert_eq!(window_source.calls(), 1, "有窗口时必须真的去问一次");
        let high = load_png_file(&path2).expect("读缓存");
        assert_eq!(high.pixels[0..4], [200, 30, 40, 255], "真图标必须覆盖通用图标");
        assert_eq!(read_tier(&tier_path(&icons, &AppId::new("att").unwrap())), Some(IconTier::Window));
    }

    /// 已经是窗口档位：再有 pid 也不重复抓（避免每次打开列表都跑 Shell API）。
    #[test]
    fn window_tier_cache_is_not_refetched_even_with_pid() {
        let dir = tempfile::tempdir().expect("临时目录失败");
        let icons = dir.path().join("icons");
        let source = TierSource::new(IconTier::Window, [1, 2, 3, 255]);
        let t = target("demo", Some(77));

        resolve_and_cache(&source, &icons, &t).expect("首次应抓到");
        assert_eq!(source.calls(), 1);

        resolve_and_cache(&source, &icons, &t).expect("二次应命中");
        resolve_and_cache(&source, &icons, &t).expect("三次应命中");
        assert_eq!(source.calls(), 1, "窗口档位已缓存时不应重复抓取");
    }

    /// 没有 pid 时，已有的 exe 档位缓存直接复用（没有窗口可问，不可能升级）。
    #[test]
    fn exe_tier_cache_is_reused_when_no_window_can_exist() {
        let dir = tempfile::tempdir().expect("临时目录失败");
        let icons = dir.path().join("icons");
        let source = TierSource::new(IconTier::Exe, [5, 5, 5, 255]);
        let t = target("pyapp", None);

        resolve_and_cache(&source, &icons, &t).expect("首次应抓到");
        resolve_and_cache(&source, &icons, &t).expect("二次应命中");
        assert_eq!(source.calls(), 1);
    }

    /// 只升不降：窗口档位已经入库后，即使这次只拿到 exe 档位也不能降级覆盖。
    #[test]
    fn lower_tier_never_overwrites_higher_tier() {
        let dir = tempfile::tempdir().expect("临时目录失败");
        let icons = dir.path().join("icons");
        let app = AppId::new("keep").expect("合法 id");

        let good = TierSource::new(IconTier::Window, [9, 9, 9, 255]);
        let path = resolve_and_cache(&good, &icons, &target("keep", Some(1))).expect("应抓到");

        // 这次窗口没取到（比如窗口刚好在重建），只能拿到 exe 档位
        let worse = TierSource::new(IconTier::Exe, [0, 0, 0, 255]);
        resolve_and_cache(&worse, &icons, &target("keep", Some(1))).expect("应返回已有缓存");

        let kept = load_png_file(&path).expect("读缓存");
        assert_eq!(kept.pixels[0..4], [9, 9, 9, 255], "好图不应被差图覆盖");
        assert_eq!(read_tier(&tier_path(&icons, &app)), Some(IconTier::Window));
    }

    /// 老缓存没有 sidecar：视为「未知档位」，一旦有窗口可查就允许升级。
    /// 这条保证升级后不需要用户手动删缓存。
    #[test]
    fn legacy_cache_without_sidecar_can_be_upgraded() {
        let dir = tempfile::tempdir().expect("临时目录失败");
        let icons = dir.path().join("icons");
        let app = AppId::new("legacy").expect("合法 id");

        // 手工造一张旧版本写下的缓存（没有 .tier 文件）
        std::fs::create_dir_all(&icons).expect("建目录");
        let stale = RgbaImage::solid(8, 8, [3, 3, 3, 255]).expect("构造失败");
        write_cache(&cache_path(&icons, &app), &stale).expect("写旧缓存");
        assert_eq!(read_tier(&tier_path(&icons, &app)), None, "旧缓存没有档位信息");

        let better = TierSource::new(IconTier::Window, [250, 250, 0, 255]);
        let path = resolve_and_cache(&better, &icons, &target("legacy", Some(9))).expect("应升级");
        assert_eq!(better.calls(), 1, "未知档位必须重抓一次");
        let now = load_png_file(&path).expect("读缓存");
        assert_eq!(now.pixels[0..4], [250, 250, 0, 255]);
    }

    /// forced 版本：即使缓存已经是窗口档位，也照抓不误（用于启动瞬间）。
    #[test]
    fn forced_recapture_ignores_cache_entirely() {
        let dir = tempfile::tempdir().expect("临时目录失败");
        let icons = dir.path().join("icons");
        let first = TierSource::new(IconTier::Window, [1, 1, 1, 255]);
        let t = target("force", Some(3));

        resolve_and_cache(&first, &icons, &t).expect("首次应抓到");
        assert_eq!(first.calls(), 1);

        let second = TierSource::new(IconTier::Window, [2, 2, 2, 255]);
        let path = resolve_and_cache_forced(&second, &icons, &t).expect("强制应重抓");
        assert_eq!(second.calls(), 1, "forced 必须真的重抓");
        assert_eq!(load_png_file(&path).expect("读缓存").pixels[0..4], [2, 2, 2, 255]);
    }

    /// forced 版本仍然不许降级：用户自定义的图标不能被 exe 档位覆盖。
    #[test]
    fn forced_recapture_does_not_downgrade_custom_icon() {
        let dir = tempfile::tempdir().expect("临时目录失败");
        let icons = dir.path().join("icons");
        let app = AppId::new("custom").expect("合法 id");

        let custom = TierSource::new(IconTier::Custom, [7, 7, 7, 255]);
        let path = resolve_and_cache(&custom, &icons, &target("custom", None)).expect("应抓到");

        let bad = TierSource::new(IconTier::Exe, [0, 0, 0, 255]);
        resolve_and_cache_forced(&bad, &icons, &target("custom", Some(5))).expect("应返回旧图");
        assert_eq!(load_png_file(&path).expect("读缓存").pixels[0..4], [7, 7, 7, 255]);
        assert_eq!(read_tier(&tier_path(&icons, &app)), Some(IconTier::Custom));
    }

    /// 旧版本留下的 `.cmd` 通用图标缓存（无档位 + 现在解析不出 exe）必须被丢弃，
    /// 而不是继续当成有效图标返回。
    ///
    /// 这是用户现场的直接修复：三个 Electron 应用的缓存都是 64×64 齿轮图、
    /// 没有 sidecar，而 `pnpm.cmd` 现在解析成 None。不清理的话它们会一直错下去。
    #[test]
    fn legacy_cache_from_script_source_is_discarded() {
        let dir = tempfile::tempdir().expect("临时目录失败");
        let icons = dir.path().join("icons");
        let app = AppId::new("legacy-script").expect("合法 id");
        std::fs::create_dir_all(&icons).expect("建目录");

        // 模拟旧缓存：有 png、没有 .tier
        let bogus = RgbaImage::solid(64, 64, [1, 2, 3, 255]).expect("构造失败");
        write_cache(&cache_path(&icons, &app), &bogus).expect("写旧缓存");

        // 没有 pid、也没有 exe（因为 pnpm.cmd 被过滤掉了）：不可能抓到更好的
        let empty = TierSource::new(IconTier::Window, [0, 0, 0, 255]);
        let target = IconTarget::for_app(app.clone());

        assert!(
            resolve_and_cache(&empty, &icons, &target).is_none(),
            "无档位且无 exe 的旧缓存应被丢弃"
        );
        assert!(!cache_path(&icons, &app).exists(), "缓存文件应被删除");
        assert_eq!(empty.calls(), 0, "这个分支不应去问来源");
    }

    /// 反过来：有档位的正常缓存不受上面那条规则影响。
    #[test]
    fn tiered_cache_is_kept_even_without_executable() {
        let dir = tempfile::tempdir().expect("临时目录失败");
        let icons = dir.path().join("icons");
        let app = AppId::new("tiered").expect("合法 id");

        let good = TierSource::new(IconTier::Exe, [4, 5, 6, 255]);
        let t = target("tiered", None);
        let path = resolve_and_cache(&good, &icons, &t).expect("应抓到");
        assert_eq!(read_tier(&tier_path(&icons, &app)), Some(IconTier::Exe));

        // 二次调用：有档位，正常复用
        let again = resolve_and_cache(&good, &icons, &t).expect("应命中");
        assert_eq!(again, path);
        assert!(path.exists(), "有档位的缓存不应被删除");
    }

    /// 档位字符串是持久化契约：改了会让所有旧缓存失效。
    #[test]
    fn tier_tags_round_trip() {
        for tier in [IconTier::Exe, IconTier::Window, IconTier::Custom] {
            assert_eq!(IconTier::from_tag(tier.tag()), Some(tier));
        }
        assert_eq!(IconTier::from_tag("nonsense"), None);
        assert_eq!(IconTier::from_tag(""), None);
        assert!(IconTier::Custom > IconTier::Window);
        assert!(IconTier::Window > IconTier::Exe);
    }
}
