/**
 * 左侧抽屉：折叠 / 展开 + 拖拽调宽。
 *
 * 结构：
 *   ┌ rail-scroll 应用列表（滚动区，紧贴标题栏）
 *   └ rail-foot   主题 / 设置 / 宿主状态
 *
 * 折叠开关在标题栏（Titlebar），不再在侧栏顶部独占一行。
 *
 * 状态由 App 持有的 `useSidebar` 提供（折叠与宽度要同时影响主区骨架与
 * AppList 的 tooltip，放在这一层会逼出"回调向上传状态"，容易在渲染期做副作用）。
 *
 * 拖拽用 Pointer Events + `setPointerCapture` 而不是 mousemove：
 * 捕获后指针移出侧栏、甚至移出窗口仍能收到 move/up，否则拖太快就会"卡住"。
 */
import { useCallback, useRef } from "react";

import type { UseSidebarResult } from "../hooks/useSidebar";
import { RAIL_MAX_WIDTH, RAIL_MIN_WIDTH } from "../hooks/useSidebar";
import { nextThemeMode, type ThemeMode } from "../hooks/useTheme";
import { IconMonitor, IconMoon, IconSettings, IconSun } from "./ui/Icons";
import { Tooltip } from "./ui/Tooltip";

interface Props {
  /** 侧栏状态：折叠、宽度、拖拽。 */
  rail: UseSidebarResult;
  /** 主题模式与切换。 */
  themeMode: ThemeMode;
  /** 三态轮转主题（跟随系统 → 浅色 → 深色）。 */
  onCycleTheme: () => void;
  /** 是否运行在 Tauri 宿主里（底部状态点）。 */
  hosted: boolean;
  /** 打开设置页（关于 / 版本 / 更新）。 */
  onOpenSettings: () => void;
  /** 设置页是否处于打开状态（高亮入口）。 */
  settingsActive?: boolean;
  /** 应用列表（由 App 传入，避免这一层依赖数据层）。 */
  children: React.ReactNode;
}

const THEME_LABEL: Record<ThemeMode, string> = {
  system: "跟随系统",
  light: "浅色",
  dark: "深色",
};

export function Sidebar({
  rail,
  themeMode,
  onCycleTheme,
  hosted,
  onOpenSettings,
  settingsActive = false,
  children,
}: Props) {
  const { collapsed, renderedWidth, resizing, toggle, setWidth, beginResize, resizeBy, endResize } = rail;

  /** 拖拽起点：pointerdown 时的 clientX。 */
  const originX = useRef(0);

  const handlePointerDown = useCallback(
    (event: React.PointerEvent<HTMLButtonElement>) => {
      // 只响应主键：右键/中键不该触发调宽
      if (event.button !== 0) {
        return;
      }
      event.preventDefault();
      originX.current = event.clientX;
      // 指针捕获：移出侧栏/移出窗口后依然能收到 move 与 up
      event.currentTarget.setPointerCapture(event.pointerId);
      beginResize();
    },
    [beginResize]
  );

  const handlePointerMove = useCallback(
    (event: React.PointerEvent<HTMLButtonElement>) => {
      if (!resizing) {
        return;
      }
      resizeBy(event.clientX - originX.current);
    },
    [resizing, resizeBy]
  );

  const handlePointerUp = useCallback(
    (event: React.PointerEvent<HTMLButtonElement>) => {
      if (event.currentTarget.hasPointerCapture(event.pointerId)) {
        event.currentTarget.releasePointerCapture(event.pointerId);
      }
      endResize();
    },
    [endResize]
  );

  /** 键盘调宽：焦点在热区上时方向键微调（Shift 加速），Home/End 到两端。 */
  const handleKeyDown = useCallback(
    (event: React.KeyboardEvent<HTMLButtonElement>) => {
      const step = event.shiftKey ? 32 : 16;
      if (event.key === "ArrowRight") {
        event.preventDefault();
        setWidth(renderedWidth + step);
      } else if (event.key === "ArrowLeft") {
        event.preventDefault();
        setWidth(renderedWidth - step);
      } else if (event.key === "Home") {
        event.preventDefault();
        setWidth(RAIL_MIN_WIDTH);
      } else if (event.key === "End") {
        event.preventDefault();
        setWidth(RAIL_MAX_WIDTH);
      } else if (event.key === "Enter") {
        // Enter = 切换折叠（与按钮同义）
        event.preventDefault();
        toggle();
      }
    },
    [renderedWidth, setWidth, toggle]
  );

  /** 下一次点击会切到哪个模式：写进 title 让用户点之前就知道结果。 */
  const nextMode = nextThemeMode(themeMode);
  const ThemeIcon = themeMode === "system" ? IconMonitor : themeMode === "light" ? IconSun : IconMoon;

  return (
    <aside
      className="sidebar"
      aria-label="应用列表"
      data-collapsed={collapsed ? "true" : "false"}
      data-resizing={resizing ? "true" : "false"}
      style={{ "--rail-current-width": renderedWidth + "px" } as React.CSSProperties}
    >
      <div className="rail-scroll">{children}</div>

      <div className="rail-foot">
        <Tooltip label={`主题：${THEME_LABEL[themeMode]}`} enabled={collapsed}>
          <button
            type="button"
            className="rail-action"
            onClick={onCycleTheme}
            title={`主题：${THEME_LABEL[themeMode]}（点击切换为${THEME_LABEL[nextMode]}）`}
          >
            <span className="rail-action-icon">
              <ThemeIcon size={15} />
            </span>
            <span className="rail-action-label">主题</span>
            <span className="rail-action-value">{THEME_LABEL[themeMode]}</span>
          </button>
        </Tooltip>

        <Tooltip label="设置" enabled={collapsed}>
          <button
            type="button"
            className="rail-action"
            title="设置（关于 / 版本 / 更新）"
            aria-current={settingsActive ? "page" : undefined}
            data-active={settingsActive ? "true" : "false"}
            onClick={onOpenSettings}
          >
            <span className="rail-action-icon">
              <IconSettings size={15} />
            </span>
            <span className="rail-action-label">设置</span>
          </button>
        </Tooltip>

        <Tooltip label={hosted ? "已连接 Tauri 宿主" : "浏览器预览模式"} enabled={collapsed}>
          <div className="rail-action rail-action-static">
            <span className="rail-action-icon">
              <span className="rail-host-dot" data-hosted={hosted ? "true" : "false"} />
            </span>
            <span className="rail-action-label">{hosted ? "已连接宿主" : "预览模式"}</span>
          </div>
        </Tooltip>
      </div>

      {/*
        右边缘拖拽热区：4px 视觉宽度 + 两侧各 4px 命中区。
        用 button 而不是 div：它能进 Tab 顺序，键盘用户也能调宽度。
      */}
      <button
        type="button"
        className="rail-handle"
        data-resizing={resizing ? "true" : "false"}
        aria-label="拖拽调整侧栏宽度"
        title="拖拽调整宽度"
        onPointerDown={handlePointerDown}
        onPointerMove={handlePointerMove}
        onPointerUp={handlePointerUp}
        onPointerCancel={handlePointerUp}
        onKeyDown={handleKeyDown}
      />
    </aside>
  );
}