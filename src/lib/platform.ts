/**
 * 平台识别：只用于"标题栏长什么样"，不参与任何业务判断。
 *
 * 为什么要自己识别而不是等 Rust 传：
 * - 首帧就要决定 macOS 是否给红黄绿留安全区、是否渲染自绘窗口按钮，
 *   等一次 IPC 会先闪一帧错位布局；
 * - WebView 的 UA 稳定包含系统名（Windows NT / Macintosh / X11），足够可靠。
 *
 * 结果写在 `<html data-platform>` 上，CSS 用 `[data-platform="macos"]` 区分：
 * - macos：预留 `--titlebar-mac-inset`，不显示自绘窗口按钮；
 * - windows / linux：自绘最小化 / 最大化 / 关闭。
 */

/** 平台标识（写进 `data-platform` 的取值）。 */
export type Platform = "windows" | "macos" | "linux";

/** 从 UA 推断平台；无法识别时按 Windows 处理（本项目的主目标平台）。 */
export function detectPlatform(): Platform {
  if (typeof navigator === "undefined") {
    return "windows";
  }
  const ua = navigator.userAgent;
  if (/Macintosh|Mac OS X|iPhone|iPad/i.test(ua)) {
    return "macos";
  }
  if (/Linux|X11/i.test(ua) && !/Android/i.test(ua)) {
    return "linux";
  }
  return "windows";
}

/** macOS 需要给原生红黄绿按钮让位，不渲染自绘按钮。 */
export function usesNativeWindowControls(platform: Platform): boolean {
  return platform === "macos";
}

/**
 * 首屏把平台写到 `<html>` 上。
 *
 * `index.html` 里有一段等价的内联脚本先跑一次（防止首帧闪错位），
 * 这里再写一次是幂等的：热更新或测试环境下直接刷新页面时也能保证属性存在。
 */
export function applyPlatform(platform: Platform = detectPlatform()): Platform {
  if (typeof document !== "undefined") {
    document.documentElement.dataset.platform = platform;
  }
  return platform;
}
