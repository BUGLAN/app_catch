/** 顶部 toast：命令报错时的中文提示，3.5 秒后自动消失。 */
import { useEffect } from "react";

export interface ToastMessage {
  id: number;
  message: string;
}

interface Props {
  toasts: ToastMessage[];
  onDismiss: (id: number) => void;
}

/** 单条 toast 的存活时间（毫秒）。 */
const TOAST_TTL_MS = 3500;

function ToastItem({ toast, onDismiss }: { toast: ToastMessage; onDismiss: (id: number) => void }) {
  useEffect(() => {
    const timer = window.setTimeout(() => onDismiss(toast.id), TOAST_TTL_MS);
    return () => window.clearTimeout(timer);
  }, [onDismiss, toast.id]);

  return (
    <div className="toast-item" role="status">
      <span>{toast.message}</span>
      <button type="button" className="toast-close" onClick={() => onDismiss(toast.id)} aria-label="关闭">
        ×
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
