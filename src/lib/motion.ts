/**
 * 动效的 JS 侧工具。
 *
 * CSS 能做的动效（hover、进场关键帧、循环）都留在样式表里；这里只处理 CSS 做不到的三件事：
 *   1. 「不重挂载组件」的视图切换 —— 用 Web Animations API 在已有节点上补一段进场；
 *   2. 退场动画的时长 —— 卸载要等动画播完，JS 必须知道要等多久；
 *   3. 主题切换的整屏交叉淡化 —— View Transitions API。
 *
 * 时长与缓动一律从 tokens.css 读，TS 不另写一份数字；读不到（测试环境、首帧前）时用回退值。
 * 所有入口都先判断 prefers-reduced-motion：偏好减弱动效时直接跳过，而不是播一段 0ms 动画。
 */

const REDUCED_MOTION_QUERY = "(prefers-reduced-motion: reduce)";

/** 用户是否偏好减弱动效（非浏览器环境视为是：没有可动的东西）。 */
export function prefersReducedMotion(): boolean {
  if (typeof window === "undefined" || typeof window.matchMedia !== "function") {
    return true;
  }
  return window.matchMedia(REDUCED_MOTION_QUERY).matches;
}

/**
 * 把 CSS 时长字符串解析成毫秒（纯函数，可单测）。
 *
 * 支持 `180ms` / `0.18s` / 带空白；无法解析时返回 null。
 */
export function parseDuration(raw: string): number | null {
  const value = raw.trim();
  const match = value.match(/^(-?\d*\.?\d+)(ms|s)$/);
  if (!match) {
    return null;
  }
  const amount = Number(match[1]);
  return match[2] === "s" ? amount * 1000 : amount;
}

/** 读取一个动效 token（自定义属性）的原始字符串。 */
function readToken(name: string): string {
  if (typeof document === "undefined" || typeof getComputedStyle !== "function") {
    return "";
  }
  return getComputedStyle(document.documentElement).getPropertyValue(name).trim();
}

/** 读取时长 token（毫秒）；读不到时用 `fallback`。 */
export function durationToken(name: string, fallback: number): number {
  return parseDuration(readToken(name)) ?? fallback;
}

/** 读取缓动 token；读不到时用 `fallback`。 */
export function easingToken(name: string, fallback: string): string {
  return readToken(name) || fallback;
}

/**
 * 退场动画需要等待的时长。偏好减弱动效时为 0：立即卸载。
 *
 * 退场统一用 `--duration-fast`（≈ 进场的 65%），与 CSS 里的 *-out 关键帧一致。
 */
export function exitDuration(): number {
  return prefersReducedMotion() ? 0 : durationToken("--duration-fast", 120);
}

/**
 * 视图进场：在**已存在**的节点上播一段淡入 + 轻微上浮。
 *
 * 为什么不用 key 重挂载 + CSS 关键帧：主区切换应用时 AppDetail / SessionDetail
 * 有自己的状态（日志订阅、图标请求），重挂载会白白丢掉并重跑。WAAPI 只改视觉，不碰组件树。
 *
 * 返回的 Animation 可被下一次切换 cancel，快速连点时不会叠加。
 */
export function playViewEnter(node: HTMLElement | null): Animation | null {
  if (!node || typeof node.animate !== "function" || prefersReducedMotion()) {
    return null;
  }
  const distance = readToken("--motion-distance-sm") || "4px";
  return node.animate(
    [
      { opacity: 0, transform: `translateY(${distance})` },
      { opacity: 1, transform: "none" },
    ],
    {
      duration: durationToken("--duration-base", 180),
      easing: easingToken("--ease-out", "cubic-bezier(0.16, 1, 0.3, 1)"),
    }
  );
}

interface ViewTransitionDocument {
  startViewTransition?: (update: () => void) => unknown;
}

/**
 * 在 View Transition 里执行一次同步的 DOM 更新（用于主题切换的整屏交叉淡化）。
 *
 * WebView 不支持、或偏好减弱动效时，直接执行更新 —— 行为完全一致，只是没有过渡。
 * `update` 必须同步完成 DOM 变化：浏览器在回调返回后立刻截取"新"画面。
 */
export function withViewTransition(update: () => void): void {
  const doc = (typeof document === "undefined" ? null : document) as
    | (Document & ViewTransitionDocument)
    | null;
  if (!doc || typeof doc.startViewTransition !== "function" || prefersReducedMotion()) {
    update();
    return;
  }
  doc.startViewTransition(update);
}
