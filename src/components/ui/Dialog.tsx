/**
 * 模态对话框：焦点陷阱 + Esc 关闭 + 背景滚动锁定 + 焦点还原。
 *
 * 原来的删除确认弹窗只是个 .modal-backdrop div：
 *   - 按 Esc 关不掉（实测弹窗仍在）；
 *   - 打开后焦点仍留在背景的「+ 新建应用」上，Tab 会跑到背景去；
 *   - 没有 aria-labelledby，读屏只会播报一个"对话框"；
 *   - 打开时背后的列表仍能滚动。
 *
 * 这些都属于"表现层与可访问性"，不影响任何业务流程：确认/取消仍走原来的回调。
 */
import { useEffect, useId, useRef, type ReactNode } from "react";

interface DialogProps {
  title: ReactNode;
  children: ReactNode;
  /** 底部操作区（按钮）。 */
  actions: ReactNode;
  /** 关闭回调：Esc / 点击遮罩 / 取消按钮都走它。 */
  onClose: () => void;
  /** 是否禁用关闭（例如提交中）。 */
  busy?: boolean;
  /** 首焦点落在哪个元素上：默认第一个可聚焦元素。 */
  initialFocus?: "first" | "cancel";
}

const FOCUSABLE =
  'button:not(:disabled), [href], input:not(:disabled), select:not(:disabled), textarea:not(:disabled), [tabindex]:not([tabindex="-1"])';

export function Dialog({ title, children, actions, onClose, busy = false, initialFocus = "first" }: DialogProps) {
  const titleId = useId();
  const bodyId = useId();
  const panelRef = useRef<HTMLDivElement | null>(null);
  // 打开前的焦点，用来关闭后还原
  const restoreRef = useRef<HTMLElement | null>(null);

  // 首焦点 + 焦点还原 + 背景滚动锁定 + Esc
  useEffect(() => {
    restoreRef.current = document.activeElement as HTMLElement | null;
    const panel = panelRef.current;
    if (panel) {
      const nodes = Array.from(panel.querySelectorAll<HTMLElement>(FOCUSABLE));
      const target =
        initialFocus === "cancel"
          ? nodes.find((node) => node.dataset.variant === "cancel") ?? nodes[0]
          : nodes[0];
      target?.focus();
    }

    const previousOverflow = document.body.style.overflow;
    document.body.style.overflow = "hidden";

    const onKeyDown = (event: KeyboardEvent) => {
      if (event.key === "Escape" && !busy) {
        event.stopPropagation();
        onClose();
        return;
      }
      if (event.key !== "Tab") {
        return;
      }
      const current = panelRef.current;
      if (!current) {
        return;
      }
      const nodes = Array.from(current.querySelectorAll<HTMLElement>(FOCUSABLE));
      if (nodes.length === 0) {
        event.preventDefault();
        return;
      }
      const first = nodes[0]!;
      const last = nodes[nodes.length - 1]!;
      if (event.shiftKey && document.activeElement === first) {
        event.preventDefault();
        last.focus();
      } else if (!event.shiftKey && document.activeElement === last) {
        event.preventDefault();
        first.focus();
      }
    };

    document.addEventListener("keydown", onKeyDown, true);
    return () => {
      document.removeEventListener("keydown", onKeyDown, true);
      document.body.style.overflow = previousOverflow;
      // 关闭后把焦点还给触发它的元素，键盘用户不会"丢失位置"。
      // body / documentElement 不是有意义的落点，此时不要抢焦点（否则焦点会跳到页面根）。
      const restore = restoreRef.current;
      if (restore && restore !== document.body && restore !== document.documentElement) {
        restore.focus?.();
      }
    };
    // onClose 由父组件用 useCallback 稳定；busy 变化不需要重建监听
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, []);

  return (
    <div
      className="modal-backdrop"
      onMouseDown={(event) => {
        // 只有点在遮罩本身（不是弹窗内部）才关闭
        if (event.target === event.currentTarget && !busy) {
          onClose();
        }
      }}
    >
      <div
        className="modal"
        role="dialog"
        aria-modal="true"
        aria-labelledby={titleId}
        aria-describedby={bodyId}
        ref={panelRef}
      >
        <h3 id={titleId}>{title}</h3>
        <div id={bodyId}>{children}</div>
        <div className="modal-actions">{actions}</div>
      </div>
    </div>
  );
}