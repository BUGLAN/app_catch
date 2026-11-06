/**
 * Toast：命令结果的中文提示。
 *
 * 相比原实现的改动（全部属于表现层）：
 *   - 同一文案合并：轮询失败时原来每秒叠一条一模一样的 toast（实测同时 3 条）；
 *     现在相同文案只保留一条并重置计时。
 *   - 区分语气：成功用 role="status"（polite），错误用 role="alert"（assertive），
 *     读屏能区分"做成了"和"没做成"。
 *   - 位置在窗口底部居中：右上角正好压在详情页的「启动 / 停止 / 编辑」按钮上，
 *     用户去点按钮时鼠标落进 toast，悬停暂停导致它永远不消失。
 *   - 成功提示 2.4 秒、不可暂停也不带关闭按钮——它只是确认，不需要用户处理；
 *     错误提示 5 秒，悬停/聚焦时暂停倒计时，可手动关闭，方便读完长报错。
 *   - 黑白风格：中性底 + 描边，不再用绿/红描边；只有错误图标保留状态色。
 *   - 最多同时显示 3 条，超出丢弃最旧的。
 *   - 退场动画：到时或点关闭后先进入 leaving 态播放收起（--duration-fast，加速曲线），
 *     再通知父组件移除。用定时器而不是 animationend 决定何时移除——事件不保证触发。
 */
import { useCallback, useEffect, useRef, useState } from "react";

import { exitDuration } from "../lib/motion";
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

/** 单条 toast 的存活时间（毫秒）：成功只是确认，短一些；错误要留时间读。 */
const TOAST_TTL_MS: Record<ToastTone, number> = { success: 2400, error: 5000 };

function ToastItem({ toast, onDismiss }: { toast: ToastMessage; onDismiss: (id: number) => void }) {
  const [paused, setPaused] = useState(false);
  const [leaving, setLeaving] = useState(false);
  const remainingRef = useRef(TOAST_TTL_MS[toast.tone]);
  /** 只有错误提示可悬停暂停：成功提示可能恰好出现在鼠标下面，暂停会让它赖着不走。 */
  const pausable = toast.tone === "error";
  const startedAtRef = useRef(0);

  /** 开始退场：播完收起动画再真正移除。重复调用无副作用。 */
  const leave = useCallback(() => setLeaving(true), []);

  useEffect(() => {
    if (!leaving) {
      return;
    }
    const timer = window.setTimeout(() => onDismiss(toast.id), exitDuration());
    return () => window.clearTimeout(timer);
  }, [leaving, onDismiss, toast.id]);

  useEffect(() => {
    if (paused || leaving) {
      return;
    }
    startedAtRef.current = Date.now();
    const timer = window.setTimeout(leave, remainingRef.current);
    return () => {
      window.clearTimeout(timer);
      // 暂停时把已过去的时间扣掉，恢复后继续剩余时长
      remainingRef.current = Math.max(0, remainingRef.current - (Date.now() - startedAtRef.current));
    };
  }, [leave, paused, leaving]);

  const pause = useCallback(() => {
    if (pausable) {
      setPaused(true);
    }
  }, [pausable]);
  const resume = useCallback(() => setPaused(false), []);

  return (
    <div
      className={"toast-item toast-" + toast.tone}
      role={toast.tone === "error" ? "alert" : "status"}
      data-state={leaving ? "leaving" : "open"}
      onMouseEnter={pause}
      onMouseLeave={resume}
      onFocus={pause}
      onBlur={resume}
    >
      <span className="toast-tone-icon">
        {toast.tone === "error" ? <IconAlert size={14} /> : <IconCheck size={14} />}
      </span>
      <span className="toast-message">{toast.message}</span>
      {pausable ? (
        <button type="button" className="toast-close" onClick={leave} aria-label="关闭提示">
          <IconClose size={12} />
        </button>
      ) : null}
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
