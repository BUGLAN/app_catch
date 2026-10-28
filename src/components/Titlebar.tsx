/**
 * 自绘标题栏（应用内 Header）。
 *
 * 布局：
 *   Logo + 应用名 → 侧栏开关 → 全局搜索 → （可拖拽留白） → 新建（+） → 窗口控制
 *
 * 侧栏开关放在这里而不是侧栏顶部：原来它在侧栏里独占一行，只为一个 15px 图标让整列下移 38px。
 * 紧跟应用名，读起来是"这个应用的侧栏"；logo 在两个平台上都保持第一位。
 *
 * 新建用图标按钮（+）而不是带字的主色按钮：新建是低频操作，
 * 不该成为全界面最醒目的色块、抢走详情页「启动」的视觉重心。名称由 aria-label / title 提供，
 * 快捷键 ⌘N / Ctrl+N 由 App 统一监听。
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
import { detectPlatform } from "../lib/platform";
import type { CommandError } from "../lib/types";
import {
  BrandMark,
  IconClose,
  IconPlus,
  IconSearch,
  IconSidebarCollapse,
  IconSidebarExpand,
} from "./ui/Icons";
import { WindowControls } from "./WindowControls";

interface Props {
  /** 全局搜索：按名称 / 标签 / id 过滤左侧列表。 */
  query: string;
  onQueryChange: (value: string) => void;
  /** 命中数量；未搜索时为 null（不显示计数）。 */
  matchCount: number | null;
  onNewApp: () => void;
  busy: boolean;
  /** 侧栏是否折叠（决定开关的图标与文案）。 */
  sidebarCollapsed: boolean;
  onToggleSidebar: () => void;
  /** 窗口太窄、侧栏被 CSS 强制成图标态：开关此时无意义，置灰并说明原因。 */
  sidebarForced?: boolean;
  /** 窗口操作失败时的 toast 回调。 */
  onWindowError?: (error: CommandError) => void;
}

/** 新建应用的快捷键文案：与 App 里的监听保持一致。 */
export const NEW_APP_SHORTCUT = detectPlatform() === "macos" ? "⌘N" : "Ctrl+N";

export function Titlebar({
  query,
  onQueryChange,
  matchCount,
  onNewApp,
  busy,
  sidebarCollapsed,
  onToggleSidebar,
  sidebarForced = false,
  onWindowError,
}: Props) {
  const searching = query.trim().length > 0;
  const toggleLabel = sidebarForced
    ? "窗口太窄，侧栏已收成图标"
    : sidebarCollapsed
      ? "展开侧栏"
      : "收起侧栏";

  return (
    <header className="titlebar" aria-label="应用标题栏" data-tauri-drag-region="deep">
      <div className="titlebar-brand">
        <span className="titlebar-mark" aria-hidden="true">
          <BrandMark size={22} />
        </span>
        <span className="titlebar-name">app_catch</span>
      </div>

      <button
        type="button"
        className="titlebar-button rail-toggle"
        onClick={onToggleSidebar}
        aria-label={toggleLabel}
        aria-expanded={!sidebarCollapsed}
        title={toggleLabel}
        disabled={sidebarForced}
      >
        {sidebarCollapsed || sidebarForced ? (
          <IconSidebarExpand size={16} />
        ) : (
          <IconSidebarCollapse size={16} />
        )}
      </button>

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
          className="titlebar-button titlebar-new"
          onClick={onNewApp}
          disabled={busy}
          aria-label="新建应用"
          aria-keyshortcuts={NEW_APP_SHORTCUT === "⌘N" ? "Meta+N" : "Control+N"}
          title={`新建应用（${NEW_APP_SHORTCUT}）`}
        >
          <IconPlus size={16} />
        </button>
      </div>

      <WindowControls onError={onWindowError} />
    </header>
  );
}