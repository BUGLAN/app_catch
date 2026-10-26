/**
 * 轻量 Tooltip：折叠态侧栏只显示图标，名称靠它补。
 *
 * 为什么用 portal + fixed 定位，而不是纯 CSS 的绝对定位气泡：
 * 气泡若挂在 `.rail-scroll`（`overflow: auto`）里面，会被滚动容器裁掉右侧一半。
 * portal 到 body 后不再受任何祖先的 overflow 影响。
 *
 * 无障碍：
 * - 气泡 `role="tooltip"` 且 `aria-hidden`：触发按钮已有 `aria-label` 与可见文本，
 *   读屏不需要念两遍；
 * - 键盘聚焦同样触发（onFocus/onBlur），不只是鼠标。
 *
 * 行为：
 * - 悬停 / 聚焦 260ms 后出现，扫过列表时不会一路弹气泡；
 * - 指针移出立刻收起（不等延迟），否则快速划过多行会留下残影；
 * - 滚动 / 改尺寸时立即收起：气泡是 fixed 定位，不会跟着列表滚。
 */
import { useCallback, useEffect, useRef, useState, type ReactNode } from "react";
import { createPortal } from "react-dom";

/** 出现延迟（毫秒）。 */
const SHOW_DELAY_MS = 260;
/** 与触发元素的水平间距。 */
const OFFSET_PX = 8;

interface TooltipProps {
  /** 气泡文字。 */
  label: ReactNode;
  /** 只在需要时启用（例如侧栏折叠态）。 */
  enabled?: boolean;
  /** 触发元素：能接收焦点/悬停的元素。 */
  children: ReactNode;
  /** 触发元素外层容器的额外类名。 */
  className?: string;
}

interface Position {
  top: number;
  left: number;
}

export function Tooltip({ label, enabled = true, children, className }: TooltipProps) {
  const triggerRef = useRef<HTMLDivElement | null>(null);
  const timerRef = useRef<number | undefined>(undefined);
  const [position, setPosition] = useState<Position | null>(null);

  const hide = useCallback(() => {
    if (timerRef.current !== undefined) {
      window.clearTimeout(timerRef.current);
      timerRef.current = undefined;
    }
    setPosition(null);
  }, []);

  const scheduleShow = useCallback(() => {
    if (!enabled) {
      return;
    }
    if (timerRef.current !== undefined) {
      window.clearTimeout(timerRef.current);
    }
    timerRef.current = window.setTimeout(() => {
      const element = triggerRef.current;
      if (!element) {
        return;
      }
      const rect = element.getBoundingClientRect();
      setPosition({ top: rect.top + rect.height / 2, left: rect.right + OFFSET_PX });
      timerRef.current = undefined;
    }, SHOW_DELAY_MS);
  }, [enabled]);

  // 禁用 / 卸载时清定时器，避免对已卸载组件 setState
  useEffect(() => {
    if (!enabled) {
      hide();
    }
    return () => {
      if (timerRef.current !== undefined) {
        window.clearTimeout(timerRef.current);
        timerRef.current = undefined;
      }
    };
  }, [enabled, hide]);

  // 气泡是 fixed：滚动或改尺寸时必须立刻收起，否则会飘在错误的位置
  useEffect(() => {
    if (!position) {
      return;
    }
    const onMove = () => hide();
    window.addEventListener("scroll", onMove, true);
    window.addEventListener("resize", onMove);
    return () => {
      window.removeEventListener("scroll", onMove, true);
      window.removeEventListener("resize", onMove);
    };
  }, [position, hide]);

  return (
    <div
      className={className ? "tooltip-trigger " + className : "tooltip-trigger"}
      ref={triggerRef}
      onMouseEnter={scheduleShow}
      onMouseLeave={hide}
      onFocus={scheduleShow}
      onBlur={hide}
    >
      {children}
      {enabled && position
        ? createPortal(
            <span
              role="tooltip"
              aria-hidden="true"
              className="rail-tooltip"
              style={{ top: position.top, left: position.left }}
            >
              {label}
            </span>,
            document.body
          )
        : null}
    </div>
  );
}