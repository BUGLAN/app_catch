/**
 * 自绘标题栏（应用内 Header）。
 *
 * 布局（对应需求里的建议结构）：
 *   左侧 Logo + 应用名  →  中间全局搜索  →  右侧业务操作  →  窗口控制
 *
 * 拖拽：
 *   `data-tauri-drag-region="deep"` 让整条 Header（含子元素）都能拖动窗口。
 *   Tauri 的 window 插件同时实现了「双击 = 最大化 / 还原」，**不要**再自己绑双击处理
 *   —— 两套逻辑会各切一次，一次双击等于没切。
 *   可点击元素由 Tauri 自己识别（button / input / [tabindex]）并阻止拖拽；
 *   这里额外给它们标 `-webkit-app-region: no-drag`（Electron 语义，Tauri 下无副作用）。
 *
 * macOS：
 *   `[data-platform="macos"] .titlebar { padding-left: var(--titlebar-mac-inset) }`
 *   让出左上角红黄绿安全区；`[data-platform="macos"] .window-controls { display:none }`
 *   隐藏自绘按钮，位置与尺寸全部交给系统。
 *
 * 主题切换放在侧栏底部（设置区）而不是这里：
 *   标题栏右侧留给"业务操作 + 窗口控制"，主题属于偏好设置，
 *   与"设置"相邻更符合心智，也避免同一个人在两处找同一个开关。
 */
import type { CommandError } from "../lib/types";
import { BrandMark, IconClose, IconPlus, IconSearch } from "./ui/Icons";
import { WindowControls } from "./WindowControls";

interface Props {
  /** 全局搜索：按名称 / 标签 / id 过滤左侧列表。 */
  query: string;
  onQueryChange: (value: string) => void;
  /** 命中数量；未搜索时为 null（不显示计数）。 */
  matchCount: number | null;
  onNewApp: () => void;
  busy: boolean;
  /** 窗口操作失败时的 toast 回调。 */
  onWindowError?: (error: CommandError) => void;
}

export function Titlebar({ query, onQueryChange, matchCount, onNewApp, busy, onWindowError }: Props) {
  const searching = query.trim().length > 0;

  return (
    <header className="titlebar" aria-label="应用标题栏" data-tauri-drag-region="deep">
      <div className="titlebar-brand">
        <span className="titlebar-mark" aria-hidden="true">
          <BrandMark size={22} />
        </span>
        <span className="titlebar-wordmark">
          <span className="titlebar-name">app_catch</span>
          <span className="titlebar-sub">开发者向应用启动器</span>
        </span>
      </div>

      {/*
        搜索框位于拖拽区内：显式 data-tauri-drag-region="false"
        保证在它上面按下鼠标不会拖动窗口（Tauri 对 input 本来就会跳过，
        但显式声明能防止以后换成 div 内容时退化）。
      */}
      <label className="titlebar-search" data-tauri-drag-region="false">
        <span className="titlebar-search-icon" aria-hidden="true">
          <IconSearch size={13} />
        </span>
        <input
          type="search"
          className="titlebar-search-input"
          value={query}
          onChange={(event) => onQueryChange(event.target.value)}
          placeholder="搜索应用、标签或 id"
          aria-label="搜索应用"
          spellCheck={false}
          autoComplete="off"
        />
        {searching ? (
          <>
            {matchCount !== null ? (
              <span className="titlebar-search-count" aria-hidden="true">
                {matchCount}
              </span>
            ) : null}
            <button
              type="button"
              className="titlebar-search-clear"
              onClick={() => onQueryChange("")}
              aria-label="清空搜索"
              title="清空搜索"
            >
              <IconClose size={11} />
            </button>
          </>
        ) : null}
      </label>

      {/* 可拖拽的中间留白：把搜索与右侧操作自然分开 */}
      <div className="titlebar-drag" aria-hidden="true" />

      <div className="titlebar-actions">
        <button
          type="button"
          className="btn btn-primary btn-sm titlebar-new"
          onClick={onNewApp}
          disabled={busy}
          aria-label="新建应用"
        >
          <IconPlus size={13} />
          <span className="titlebar-new-label">新建应用</span>
        </button>
      </div>

      <WindowControls onError={onWindowError} />
    </header>
  );
}