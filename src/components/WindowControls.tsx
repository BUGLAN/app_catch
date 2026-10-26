/**
 * 窗口控制按钮组：最小化 / 最大化（还原）/ 关闭。
 *
 * 跨平台：
 * - Windows / Linux：渲染三个自绘按钮（主进程侧 `decorations: false`）；
 * - macOS：`display: none`（CSS 里按 `[data-platform="macos"]` 隐藏），
 *   红黄绿由系统绘制。DOM 仍保留，是为了让键盘快捷键与读屏结构在两端一致。
 *
 * 图标跟随最大化状态：状态来自两处 ——
 *   1. 挂载时查一次 `is_maximized`（刷新页面时窗口可能已经是最大化）；
 *   2. 之后由 Rust 侧 `window:maximized-changed` 事件推送（拖拽缩放也会触发）。
 * 点击最大化按钮时用命令的返回值立即更新，不等事件绕一圈，避免图标"慢半拍"。
 *
 * 不在 Tauri 宿主里（浏览器预览）时按钮置灰：点了也没窗口可控制，
 * 弹一个错误 toast 只会让人以为程序坏了。
 */
import { useCallback, useEffect, useState } from "react";

import { isHosted, onMaximizedChange, toCommandError, windowApi } from "../lib/api";
import type { CommandError } from "../lib/types";
import {
  IconWindowClose,
  IconWindowMaximize,
  IconWindowMinimize,
  IconWindowRestore,
} from "./ui/Icons";

interface Props {
  /** 窗口操作失败时的提示回调（复用 App 的 toast）。 */
  onError?: (error: CommandError) => void;
}

export function WindowControls({ onError }: Props) {
  const hosted = isHosted();
  const [maximized, setMaximized] = useState(false);
  /** 首帧还没问到状态时不要闪一下"最大化"图标。 */
  const [ready, setReady] = useState(!hosted);

  // 首屏同步一次真实状态
  useEffect(() => {
    if (!hosted) {
      return;
    }
    let cancelled = false;
    windowApi
      .isMaximized()
      .then((value) => {
        if (!cancelled) {
          setMaximized(value);
        }
      })
      .catch(() => {
        // 查不到就按"未最大化"渲染：图标错一次比报错友好
      })
      .finally(() => {
        if (!cancelled) {
          setReady(true);
        }
      });
    return () => {
      cancelled = true;
    };
  }, [hosted]);

  // 主进程推送的状态变化
  useEffect(() => {
    if (!hosted) {
      return;
    }
    let disposed = false;
    let unlisten: (() => void) | undefined;
    onMaximizedChange((value) => setMaximized(value))
      .then((fn) => {
        if (disposed) {
          fn();
          return;
        }
        unlisten = fn;
      })
      .catch(() => {
        // 监听失败不致命：点击按钮时的返回值仍会更新图标
      });
    return () => {
      disposed = true;
      unlisten?.();
    };
  }, [hosted]);

  const handle = useCallback(
    (action: () => Promise<unknown>) => {
      action().catch((raw) => onError?.(toCommandError(raw)));
    },
    [onError]
  );

  const toggleMaximize = useCallback(() => {
    windowApi
      .toggleMaximize()
      .then((value) => setMaximized(value))
      .catch((raw) => onError?.(toCommandError(raw)));
  }, [onError]);

  const maximizeLabel = maximized ? "还原" : "最大化";

  return (
    <div className="window-controls">
      <button
        type="button"
        className="window-control"
        onClick={() => handle(windowApi.minimize)}
        disabled={!hosted}
        aria-label="最小化"
        title="最小化"
      >
        <IconWindowMinimize size={12} />
      </button>
      <button
        type="button"
        className="window-control"
        onClick={toggleMaximize}
        disabled={!hosted || !ready}
        aria-label={maximizeLabel}
        title={maximizeLabel}
      >
        {maximized ? <IconWindowRestore size={12} /> : <IconWindowMaximize size={12} />}
      </button>
      <button
        type="button"
        className="window-control window-control-close"
        onClick={() => handle(windowApi.close)}
        disabled={!hosted}
        aria-label="关闭窗口"
        title="关闭"
      >
        <IconWindowClose size={12} />
      </button>
    </div>
  );
}
