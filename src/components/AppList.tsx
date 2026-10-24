/**
 * 左侧应用列表：图标占位块 + 名称 + 标签 + 状态徽标，hover 显示「启动」按钮。
 */
import type { AppConfig, SessionSnapshot } from "../lib/types";
import { initialOf, runStateLabel } from "../lib/logModel";
import type { RunState } from "../lib/types";

interface Props {
  apps: AppConfig[];
  sessions: SessionSnapshot[];
  selectedId: string | null;
  onSelect: (app: AppConfig) => void;
  onStart: (app: AppConfig) => void;
  busy: boolean;
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

export function AppList({ apps, sessions, selectedId, onSelect, onStart, busy }: Props) {
  if (apps.length === 0) {
    return <p className="sidebar-hint">暂无应用，点右上角「新建应用」录入第一个。</p>;
  }

  return (
    <ul className="app-list">
      {apps.map((app) => {
        const state = aggregateState(app.id, sessions);
        const runningCount = sessions.filter(
          (session) => session.app_id === app.id && session.state === "running"
        ).length;
        return (
          <li key={app.id}>
            <div
              className={`app-item${app.id === selectedId ? " is-selected" : ""}`}
              role="button"
              tabIndex={0}
              onClick={() => onSelect(app)}
              onKeyDown={(event) => {
                if (event.key === "Enter" || event.key === " ") {
                  event.preventDefault();
                  onSelect(app);
                }
              }}
            >
              <span className="app-avatar" aria-hidden="true">
                {initialOf(app.name)}
              </span>
              <span className="app-item-main">
                <span className="app-item-name" title={app.name}>
                  {app.name}
                </span>
                <span className="app-item-meta">
                  {(app.tags ?? []).length > 0 ? (
                    (app.tags ?? []).slice(0, 3).map((tag) => (
                      <span key={tag} className="tag">
                        {tag}
                      </span>
                    ))
                  ) : (
                    <span className="tag tag-muted">无标签</span>
                  )}
                </span>
              </span>
              <span className="app-item-side">
                <span className={`badge badge-${state}`}>
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
                >
                  启动
                </button>
              </span>
            </div>
          </li>
        );
      })}
    </ul>
  );
}
