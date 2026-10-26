/**
 * 一个最小的 matchMedia 订阅 Hook。
 *
 * 为什么需要它：侧栏的"图标态"有两个来源 ——
 *   1. 用户主动折叠（`useSidebar.collapsed`）；
 *   2. 窗口窄到放不下抽屉（CSS 在 ≤900px 强制收成 56px）。
 * 第 2 种只有 CSS 知道，React 拿不到，于是图标没有 tooltip —— 用户会看到
 * 一排没有名字的图标。这里把同一条断点同步进 React，让两者共用一套渲染逻辑。
 */
import { useEffect, useState } from "react";

/** 订阅一条媒体查询，返回当前是否匹配。 */
export function useMediaQuery(query: string): boolean {
  const [matches, setMatches] = useState(() => {
    if (typeof window === "undefined" || typeof window.matchMedia !== "function") {
      return false;
    }
    return window.matchMedia(query).matches;
  });

  useEffect(() => {
    if (typeof window === "undefined" || typeof window.matchMedia !== "function") {
      return;
    }
    const media = window.matchMedia(query);
    const onChange = (event: MediaQueryListEvent) => setMatches(event.matches);
    setMatches(media.matches);
    media.addEventListener("change", onChange);
    return () => media.removeEventListener("change", onChange);
  }, [query]);

  return matches;
}

/**
 * 侧栏是否被窗口宽度强制成图标态。
 *
 * 900px 必须与 `src/styles/chrome.css` 里的 `@media (max-width: 900px)` 一致；
 * 两处都是"抽屉能否与主区共存"的同一条判断，抄成常量便于对照修改。
 */
export const RAIL_COMPACT_QUERY = "(max-width: 900px)";

export function useRailCompact(): boolean {
  return useMediaQuery(RAIL_COMPACT_QUERY);
}
