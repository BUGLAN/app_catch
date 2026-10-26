/**
 * 侧边栏（抽屉）：折叠状态 + 可拖拽宽度，双持久化。
 *
 * 宽度语义只有一个：`width` 是**展开态**的宽度。折叠时 CSS 走
 * `--rail-collapsed-width`，展开时走这里写出的内联变量。
 * 这样"折叠 → 展开"能回到用户上次拖出来的宽度，不需要记两份。
 *
 * 拖拽由 `Sidebar` 组件通过 `beginResize` 接管：
 * 指针事件与"是否正在拖拽"放在这里，是为了让 CSS 能在拖拽期间关掉过渡
 * （`data-resizing`）——否则每一帧都会补间，看上去像在追着指针跑。
 */
import { useCallback, useEffect, useRef, useState } from "react";

import { readJson, writeJson, STORAGE_KEYS } from "../lib/preferences";

/** 宽度约束（与 tokens.css 的 --rail-min-width / --rail-max-width 保持一致）。 */
export const RAIL_MIN_WIDTH = 56;
export const RAIL_MAX_WIDTH = 320;
/** 展开态默认宽度。 */
export const RAIL_DEFAULT_WIDTH = 220;
/** 折叠态宽度（只放图标）。 */
export const RAIL_COLLAPSED_WIDTH = 56;

/** 把任意输入夹到合法区间并取整；NaN 退回默认值。 */
export function clampRailWidth(value: number): number {
  if (!Number.isFinite(value)) {
    return RAIL_DEFAULT_WIDTH;
  }
  const rounded = Math.round(value);
  if (rounded < RAIL_MIN_WIDTH) {
    return RAIL_MIN_WIDTH;
  }
  if (rounded > RAIL_MAX_WIDTH) {
    return RAIL_MAX_WIDTH;
  }
  return rounded;
}

/** 持久化载荷。单独定义是为了能对它做向后兼容的演进（缺字段用默认值补齐）。 */
interface RailPrefs {
  collapsed?: boolean;
  width?: number;
}

/** 从 localStorage 恢复；任何异常/非法值都退回默认。 */
export function readRailPrefs(): { collapsed: boolean; width: number } {
  const raw = readJson<RailPrefs>(STORAGE_KEYS.rail);
  return {
    collapsed: raw?.collapsed === true,
    width: clampRailWidth(typeof raw?.width === "number" ? raw.width : RAIL_DEFAULT_WIDTH),
  };
}

export interface UseSidebarResult {
  collapsed: boolean;
  width: number;
  /** 当前渲染宽度（折叠时为折叠宽度），供需要它的组件（如拖拽起点计算）使用。 */
  renderedWidth: number;
  resizing: boolean;
  toggle: () => void;
  setWidth: (width: number) => void;
  /** 开始拖拽：记录起点，之后由 move/end 更新。 */
  beginResize: () => void;
  resizeBy: (deltaX: number) => void;
  endResize: () => void;
}

export function useSidebar(): UseSidebarResult {
  const initial = useRef(readRailPrefs()).current;
  const [collapsed, setCollapsed] = useState(initial.collapsed);
  const [width, setWidthState] = useState(initial.width);
  const [resizing, setResizing] = useState(false);

  // 持久化：拖拽过程中每帧写 localStorage 会明显卡顿，这里节流到"停下 200ms 后"
  useEffect(() => {
    const timer = window.setTimeout(() => {
      writeJson(STORAGE_KEYS.rail, { collapsed, width } satisfies RailPrefs);
    }, 200);
    return () => window.clearTimeout(timer);
  }, [collapsed, width]);

  const toggle = useCallback(() => setCollapsed((value) => !value), []);

  const setWidth = useCallback((next: number) => setWidthState(clampRailWidth(next)), []);

  /** 拖拽起点宽度：必须在按下那一刻取，之后 deltaX 累加才有稳定基准。 */
  const startWidth = useRef(RAIL_DEFAULT_WIDTH);

  const beginResize = useCallback(() => {
    setResizing(true);
    // 折叠态下拖拽右边缘 = 想展开：先把宽度锚到折叠宽度，拖动即可自然展开
    if (collapsed) {
      setCollapsed(false);
      setWidthState((current) => Math.max(current, RAIL_MIN_WIDTH));
    }
  }, [collapsed]);

  const resizeBy = useCallback((deltaX: number) => {
    setWidthState(clampRailWidth(startWidth.current + deltaX));
  }, []);

  const endResize = useCallback(() => setResizing(false), []);

  // 每次开始拖拽都重新锚定基准宽度
  useEffect(() => {
    if (resizing) {
      startWidth.current = width;
    }
    // 只在进入拖拽时锚定一次；拖拽中的 width 变化不应重置基准
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [resizing]);

  return {
    collapsed,
    width,
    renderedWidth: collapsed ? RAIL_COLLAPSED_WIDTH : width,
    resizing,
    toggle,
    setWidth,
    beginResize,
    resizeBy,
    endResize,
  };
}