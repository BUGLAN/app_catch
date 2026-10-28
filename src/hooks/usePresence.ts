/**
 * 退场动画的"存在态"：值变成 null 后，再保留上一个值一小段时间，让退场动画播完。
 *
 * 典型用法（弹窗）：
 *   const presence = usePresence(deleteTarget);
 *   presence.value && <Dialog closing={presence.leaving} … />
 *
 * 只延迟**卸载**，不延迟业务状态：调用方的 state 已经是 null，确认/取消回调照常执行；
 * 这段时间里组件处于 leaving 态，CSS 负责播退场并关掉指针事件。
 *
 * 不依赖 animationend 判断何时卸载（事件可能因元素被隐藏而不触发），统一用定时器；
 * 退场中途值又变回非 null（快速重开）时取消定时器，直接回到显示态。
 */
import { useEffect, useRef, useState } from "react";

import { exitDuration } from "../lib/motion";

interface Presence<T> {
  /** 应该渲染的值：显示中 = 当前值；退场中 = 最后一个非 null 值；已卸载 = null。 */
  value: T | null;
  /** 是否处于退场动画中。 */
  leaving: boolean;
}

export function usePresence<T>(current: T | null): Presence<T> {
  const [retained, setRetained] = useState<T | null>(current);
  const lastRef = useRef<T | null>(current);

  if (current !== null) {
    lastRef.current = current;
  }

  useEffect(() => {
    if (current !== null) {
      setRetained(current);
      return;
    }
    const delay = exitDuration();
    if (delay === 0) {
      setRetained(null);
      return;
    }
    const timer = window.setTimeout(() => setRetained(null), delay);
    return () => window.clearTimeout(timer);
  }, [current]);

  if (current !== null) {
    return { value: current, leaving: false };
  }
  return retained === null ? { value: null, leaving: false } : { value: lastRef.current, leaving: true };
}
