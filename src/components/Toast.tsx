/**
 * Toast：命令结果的中文提示。
 *
 * 相比原实现的改动（全部属于表现层）：
 *   - 同一文案合并：轮询失败时原来每秒叠一条一模一样的 toast（实测同时 3 条）；
 *     现在相同文案只保留一条并重置计时。
 *   - 区分语气：成功用 role="status"（polite），错误用 role="alert"（assertive），
 *     读屏能区分"做成了"和"没做成"。
 *   - 悬停/聚焦暂停倒计时：用户正要读的时候不会被抢走。TTL 仍是 3.5 秒。
 *   - 最多同时显示 3 条，超出丢弃最旧的。
 */
import { useCallback, useEffect, useRef, useState } from "react";

import { IconAlert, IconCheck, IconClose } from "./ui/Icons";

/** toast 语气。 */
export type ToastTone = "error" | "success";

export interface ToastMessage {
  id: number;
  message: string;
  tone: ToastTone;
}

interface Props {
  toasts: ToastMessage[];
  onDismiss: (id: number) => void;
}

/** 单条 toast 的存活时间（毫秒）。 */
const TOAST_TTL_MS = 3500;

function ToastItem({ toast, onDismiss }: { toast: ToastMessage; onDismiss: (id: number) => void }) {
  const [paused, setPaused] = useState(false);
  const remainingRef = useRef(TOAST_TTL_MS);
  const startedAtRef = useRef(0);

  useEffect(() => {
    if (paused) {
      return;
    }
    startedAtRef.current = Date.now();
    const timer = window.setTimeout(() => onDismiss(toast.id), remainingRef.current);
    return () => {
      window.clearTimeout(timer);
      // 暂停时把已过去的时间扣掉，恢复后继续剩余时长
      remainingRef.current = Math.max(0, remainingRef.current - (Date.now() - startedAtRef.current));
    };
  }, [onDismiss, toast.id, paused]);

  const pause = useCallback(() => setPaused(true), []);
  const resume = useCallback(() => setPaused(false), []);

  return (
    <div
      className={"toast-item toast-" + toast.tone}
      role={toast.tone === "error" ? "alert" : "status"}
      onMouseEnter={pause}
      onMouseLeave={resume}
      onFocus={pause}
      onBlur={resume}
    >
      <span className="toast-tone-icon">
        {toast.tone === "error" ? <IconAlert size={14} /> : <IconCheck size={14} />}
      </span>
      <span className="toast-message">{toast.message}</span>
      <button
        type="button"
        className="toast-close"
        onClick={() => onDismiss(toast.id)}
        aria-label="关闭提示"
      >
        <IconClose size={12} />
      </button>
    </div>
  );
}

export function Toast({ toasts, onDismiss }: Props) {
  if (toasts.length === 0) {
    return null;
  }
  return (
    <div className="toast-stack">
      {toasts.map((toast) => (
        <ToastItem key={toast.id} toast={toast} onDismiss={onDismiss} />
      ))}
    </div>
  );
}
