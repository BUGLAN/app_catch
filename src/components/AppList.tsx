/**
 * 左侧应用列表：只负责"选应用"，一行 = 图标 + 名称。
 *
 * 侧栏刻意保持极简：标签、状态文字、启动按钮都只在右侧详情出现，
 * 这里唯一的状态信号是图标右下角的小圆点（运行中 / 崩溃，其余不显示）。
 *
 * 图标由 AppIcon 负责：抓不到就用内置默认图，绝不显示错误。
 *
 *   li > div.app-item
 *          ├ button.app-item-select  ← 覆盖整行的真按钮（负责选中）
 *          └ 图标 / 名称               ← 纯展示，pointer-events: none
 *
 * 抽屉折叠态：
 *   名称收起，只剩居中的图标位，名称靠 Tooltip（portal）补。
 *   列表本身不换结构 —— 折叠只由 CSS 的 [data-collapsed] 决定，
 *   组件只负责把 Tooltip 的 enabled 切过去，避免两套 DOM 分支各自漂移。
 */
import type { AppConfig, SessionSnapshot } from "../lib/types";
import { runStateLabel } from "../lib/logModel";
import type { RunState } from "../lib/types";
import { AppIcon } from "./AppIcon";
import { NEW_APP_SHORTCUT } from "./Titlebar";
import { ListSkeleton } from "./ui/Primitives";
import { Tooltip } from "./ui/Tooltip";

interface Props {
  apps: AppConfig[];
  sessions: SessionSnapshot[];
  selectedId: string | null;
  onSelect: (app: AppConfig) => void;
  /** 图标刷新触发器：会话启动后 +1，让列表重新抓运行时图标。 */
  iconRefresh?: number;
  /** 刷新时是否强制穿透缓存（会话刚启动那一次为真）。 */
  iconForceRefresh?: boolean;
  /** 首屏加载中：渲染骨架而不是「暂无应用」。 */
  loading?: boolean;
  /** 抽屉是否折叠：折叠时只显示图标并启用 tooltip。 */
  collapsed?: boolean;
}

/** 一个应用的综合状态：有 running 会话即运行中，否则看最近一次退出结果。 */
export function aggregateState(
  appId: string,
  sessions: SessionSnapshot[]
): RunState | "idle" {
  const own = sessions.filter((session) => session.app_id === appId);
  if (own.some((session) => session.state === "running")) {
    return "running";
  }
  if (own.some((session) => session.state === "crashed")) {
    return "crashed";
  }
  if (own.length > 0) {
    return "exited";
  }
  return "idle";
}

const STATE_LABEL: Record<RunState | "idle", string> = {
  idle: "未运行",
  running: runStateLabel("running"),
  exited: runStateLabel("exited"),
  crashed: runStateLabel("crashed"),
};

export function AppList({
  apps,
  sessions,
  selectedId,
  onSelect,
  iconRefresh = 0,
  iconForceRefresh = false,
  loading = false,
  collapsed = false,
}: Props) {
  if (loading) {
    return <ListSkeleton collapsed={collapsed} />;
  }

  if (apps.length === 0) {
    return (
      <p className="rail-hint">
        {collapsed ? "暂无应用" : `暂无应用，点右上角 + 或按 ${NEW_APP_SHORTCUT} 新建。`}
      </p>
    );
  }

  return (
    <ul className="app-list">
      {apps.map((app, index) => {
        const state = aggregateState(app.id, sessions);
        const selected = app.id === selectedId;
        return (
          // 错峰入场的序号（motion.css 封顶 8 项）
          <li key={app.id} style={{ "--stagger-index": index } as React.CSSProperties}>
            <Tooltip label={app.name} enabled={collapsed}>
              <div className={"app-item" + (selected ? " is-selected" : "")} data-state={state}>
                {/*
                  选择按钮是"整行覆盖"的空按钮，可访问名由 sr-only 提供（名称 + 状态）；
                  可视层整块 aria-hidden，避免读屏重复播报。
                */}
                <button
                  type="button"
                  className="app-item-select"
                  onClick={() => onSelect(app)}
                  aria-current={selected ? "true" : undefined}
                >
                  <span className="sr-only">{`${app.name}，${STATE_LABEL[state]}`}</span>
                </button>

                <span className="app-item-icon" aria-hidden="true">
                  <AppIcon
                    appId={app.id}
                    name={app.name}
                    refreshKey={iconRefresh}
                    forceRefresh={iconForceRefresh}
                  />
                  {/* 侧栏唯一的状态信号：只有运行中 / 崩溃才出点 */}
                  {(state === "running" || state === "crashed") && (
                    <span className={`app-item-dot app-item-dot-${state}`} />
                  )}
                </span>

                <span className="app-item-name" aria-hidden="true" title={app.name}>
                  {app.name}
                </span>
              </div>
            </Tooltip>
          </li>
        );
      })}
    </ul>
  );
}
