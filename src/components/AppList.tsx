/**
 * 左侧应用列表：应用图标 + 名称 + 标签 + 状态徽标，hover 显示「启动」按钮。
 *
 * 图标由 AppIcon 负责：抓不到就用内置默认图，绝不显示错误。
 *
 * 结构上的一次修正：原来行容器是 `div[role=button]`，里面又嵌了一个真 `button`
 * （启动按钮）。这是嵌套交互元素 —— 键盘 Tab 会在每一行停两次，读屏也会把
 * 「启动」念成行按钮名字的一部分。现在改成：
 *
 *   li > div.app-item
 *          ├ button.app-item-select  ← 覆盖整行的真按钮（负责选中）
 *          ├ 图标 / 名称 / 标签       ← 纯展示，pointer-events: none
 *          └ 徽标 + 启动按钮          ← 与选择按钮同级
 *
 * 抽屉折叠态：
 *   行变成 34×34 的方形图标位，名称与徽标收起，名称靠 Tooltip（portal）补。
 *   列表本身不换结构 —— 折叠只由 CSS 的 [data-collapsed] 决定，
 *   组件只负责把 Tooltip 的 enabled 切过去，避免两套 DOM 分支各自漂移。
 */
import type { AppConfig, SessionSnapshot } from "../lib/types";
import { runStateLabel } from "../lib/logModel";
import type { RunState } from "../lib/types";
import { AppIcon } from "./AppIcon";
import { IconPlay } from "./ui/Icons";
import { ListSkeleton } from "./ui/Primitives";
import { Tooltip } from "./ui/Tooltip";

interface Props {
  apps: AppConfig[];
  sessions: SessionSnapshot[];
  selectedId: string | null;
  onSelect: (app: AppConfig) => void;
  onStart: (app: AppConfig) => void;
  busy: boolean;
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

/** 折叠态的 tooltip 文案：名称 + 状态，一行说清。 */
export function railItemTooltip(app: AppConfig, sessions: SessionSnapshot[]): string {
  const state = aggregateState(app.id, sessions);
  const running = sessions.filter(
    (session) => session.app_id === app.id && session.state === "running"
  ).length;
  const suffix = running > 1 ? ` ×${running}` : "";
  return `${app.name} · ${STATE_LABEL[state]}${suffix}`;
}

export function AppList({
  apps,
  sessions,
  selectedId,
  onSelect,
  onStart,
  busy,
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
        {collapsed ? "暂无应用" : "暂无应用，点右上角「新建应用」录入第一个。"}
      </p>
    );
  }

  return (
    <ul className="app-list">
      {apps.map((app) => {
        const state = aggregateState(app.id, sessions);
        const runningCount = sessions.filter(
          (session) => session.app_id === app.id && session.state === "running"
        ).length;
        const selected = app.id === selectedId;
        return (
          <li key={app.id}>
            <Tooltip label={railItemTooltip(app, sessions)} enabled={collapsed}>
              <div
                className={"app-item" + (selected ? " is-selected" : "")}
                data-state={state}
              >
                {/*
                  选择按钮用的是"整行覆盖"的空按钮，它的可访问名由下面这句 sr-only 提供。
                  可视层整块 aria-hidden：否则读屏会把名称、状态、标签各念一遍
                  （按钮名 + 后面的文本节点 = 重复播报）。
                */}
                <button
                  type="button"
                  className="app-item-select"
                  onClick={() => onSelect(app)}
                  aria-current={selected ? "true" : undefined}
                >
                  <span className="sr-only">
                    {`${app.name}，${STATE_LABEL[state]}`}
                    {runningCount > 1 ? `（${runningCount} 个运行中会话）` : ""}
                    {(app.tags ?? []).length > 0 ? `，标签：${(app.tags ?? []).join("、")}` : ""}
                  </span>
                </button>

                <span className="app-item-icon">
                  <AppIcon
                    appId={app.id}
                    name={app.name}
                    refreshKey={iconRefresh}
                    forceRefresh={iconForceRefresh}
                  />
                  {/* 运行中的小状态点：折叠态下这是唯一的状态信号 */}
                  <span className={`app-item-dot app-item-dot-${state}`} aria-hidden="true" />
                </span>

                <span className="app-item-main" aria-hidden="true">
                  {/* 第一行：名称独占整行，尽量多显示几个字 */}
                  <span className="app-item-name" title={app.name}>
                    {app.name}
                  </span>
                  {/*
                    第二行：标签在左（放不下就裁），状态槽位在右。
                    状态徽标与「启动」按钮共用这一格（按钮绝对定位覆盖徽标），
                    hover 时只换显隐、不改变行宽，名称与标签因此不会跳动。
                  */}
                  <span className="app-item-line">
                    <span className="app-item-meta">
                      {(app.tags ?? []).length > 0 ? (
                        (app.tags ?? []).slice(0, 3).map((tag) => (
                          <span key={tag} className="tag" title={tag}>
                            {tag}
                          </span>
                        ))
                      ) : (
                        <span className="tag tag-muted">无标签</span>
                      )}
                    </span>
                    <span className="app-item-status">
                      <span className={`badge badge-${state}`} title={STATE_LABEL[state]}>
                        {STATE_LABEL[state]}
                        {runningCount > 1 ? ` ×${runningCount}` : ""}
                      </span>
                      <button
                        type="button"
                        className="btn btn-mini app-start"
                        disabled={busy}
                        onClick={(event) => {
                          // 阻止冒泡：点启动不应同时切换选中项
                          event.stopPropagation();
                          onStart(app);
                        }}
                        aria-label={`启动 ${app.name}`}
                      >
                        <IconPlay size={10} />
                        启动
                      </button>
                    </span>
                  </span>
                </span>
              </div>
            </Tooltip>
          </li>
        );
      })}
    </ul>
  );
}
