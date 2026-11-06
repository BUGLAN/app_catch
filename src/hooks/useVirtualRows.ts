/**
 * 变高行的虚拟滚动：只渲染视口附近的行，其余用上下两块占位撑开滚动条。
 *
 * 行高不固定（长行折行、分隔行更高），做法是「测量 + 估计」：
 * - 每次渲染后读一遍已渲染行的真实高度，按行 key 缓存；
 * - 没渲染过的行用已测高度的平均值估计；
 * - 容器宽度变化会改变折行，此时作废全部测量，重新估计。
 *
 * 调用方约定：
 * - 每个渲染出的行用一个外层元素包住，并挂上 `data-vkey={key}`（测量靠它）；
 *   外层元素要能包住子元素的 margin（`display: flow-root`），否则测不准；
 * - 行前后分别渲染 `padTop` / `padBottom` 高度的占位元素；
 * - 传入的 `element` 是滚动容器本身（标签页切走时为 null）。
 */
import { useCallback, useEffect, useLayoutEffect, useMemo, useRef, useState } from "react";

import { buildOffsets, centeredScrollTop, visibleRange } from "../lib/virtualList";

/** 还没有任何测量时的行高估计（等宽小字号单行日志的实际高度附近）。 */
const DEFAULT_ROW_ESTIMATE = 22;

/** 视口上下各多渲染多少像素：快速滚动时不露白。 */
const OVERSCAN_PX = 480;

/** 测量值变化小于这个像素数就当没变，避免亚像素抖动反复重算。 */
const MEASURE_TOLERANCE = 0.5;

interface Viewport {
  top: number;
  height: number;
}

export interface VirtualRows {
  /** 渲染区间 `[start, end)`。 */
  start: number;
  end: number;
  /** 区间之前 / 之后的占位高度。 */
  padTop: number;
  padBottom: number;
  /** 全部行的（估计）总高度；变化时调用方可能需要重新贴底。 */
  totalHeight: number;
  /** 把第 index 行滚到视口中间（按当前测量 / 估计的位置）。 */
  scrollToIndex: (index: number) => void;
}

export function useVirtualRows<T>(
  items: readonly T[],
  getKey: (item: T) => string,
  element: HTMLElement | null
): VirtualRows {
  const heights = useRef(new Map<string, number>());
  /** 测量结果变化时递增，驱动偏移重算。 */
  const [measureVersion, setMeasureVersion] = useState(0);
  const [viewport, setViewport] = useState<Viewport>({ top: 0, height: 0 });
  /** 容器上内边距：scrollTop 包含它，行偏移不包含。 */
  const paddingTop = useRef(0);

  // 视口：跟踪滚动位置与容器尺寸
  useEffect(() => {
    if (!element) {
      return;
    }
    let lastWidth = element.clientWidth;
    const read = () => {
      const top = element.scrollTop;
      const height = element.clientHeight;
      setViewport((prev) => (prev.top === top && prev.height === height ? prev : { top, height }));
    };
    const observer = new ResizeObserver(() => {
      paddingTop.current = parseFloat(getComputedStyle(element).paddingTop) || 0;
      if (element.clientWidth !== lastWidth) {
        lastWidth = element.clientWidth;
        // 宽度变了，折行全变：旧测量作废
        heights.current.clear();
        setMeasureVersion((value) => value + 1);
      }
      read();
    });
    paddingTop.current = parseFloat(getComputedStyle(element).paddingTop) || 0;
    element.addEventListener("scroll", read, { passive: true });
    observer.observe(element);
    read();
    return () => {
      element.removeEventListener("scroll", read);
      observer.disconnect();
    };
  }, [element]);

  const offsets = useMemo(() => {
    const measured = heights.current;
    let sum = 0;
    for (const height of measured.values()) {
      sum += height;
    }
    const estimate = measured.size > 0 ? sum / measured.size : DEFAULT_ROW_ESTIMATE;
    return buildOffsets(items.length, (index) => measured.get(getKey(items[index]!)) ?? estimate);
    // measureVersion 是「缓存内容变了」的信号，必须参与依赖
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [items, getKey, measureVersion]);

  const contentTop = Math.max(0, viewport.top - paddingTop.current);
  const { start, end } = visibleRange(offsets, contentTop, viewport.height, OVERSCAN_PX);
  const totalHeight = offsets[offsets.length - 1] ?? 0;
  const padTop = offsets[start] ?? 0;
  const padBottom = Math.max(0, totalHeight - (offsets[end] ?? totalHeight));

  // 测量：每次渲染后读已渲染行的真实高度；有变化就重算偏移（通常一两轮即收敛）
  useLayoutEffect(() => {
    if (!element) {
      return;
    }
    const measured = heights.current;
    let changed = false;
    element.querySelectorAll<HTMLElement>("[data-vkey]").forEach((node) => {
      const key = node.dataset.vkey;
      const height = node.getBoundingClientRect().height;
      if (key === undefined || height <= 0) {
        return;
      }
      const previous = measured.get(key);
      if (previous === undefined || Math.abs(previous - height) > MEASURE_TOLERANCE) {
        measured.set(key, height);
        changed = true;
      }
    });
    // 旧行被裁掉后它们的测量也没用了：缓存明显大于行数时按当前行重建
    if (measured.size > items.length * 2 + 200) {
      const keep = new Set(items.map(getKey));
      for (const key of measured.keys()) {
        if (!keep.has(key)) {
          measured.delete(key);
        }
      }
    }
    if (changed) {
      setMeasureVersion((value) => value + 1);
    }
    // 渲染出的行只随这些值变化；measureVersion 让「宽度变化后清空缓存」也会重新测一遍，
    // 测量没有新变化时不再递增，因此最多多跑一轮就停
  }, [element, items, getKey, start, end, measureVersion]);

  const offsetsRef = useRef(offsets);
  useLayoutEffect(() => {
    offsetsRef.current = offsets;
  }, [offsets]);

  const scrollToIndex = useCallback(
    (index: number) => {
      if (!element) {
        return;
      }
      element.scrollTop =
        centeredScrollTop(offsetsRef.current, index, element.clientHeight) + paddingTop.current;
    },
    [element]
  );

  return { start, end, padTop, padBottom, totalHeight, scrollToIndex };
}
