/**
 * 应用详情：图标 + 配置展示 + 操作按钮 + 该应用的活跃会话列表（多实例）。
 *
 * 表现层改动：
 *   - 键值表改用共享 KeyValueList，长路径与长命令行统一走 overflow-wrap；
 *   - 操作按钮加图标与更明确的 disabled 说明（title 保留原语义）；
 *   - 会话表包进 .table-scroll，窄窗口下横向滚动而不是挤压变形；
 *   - 会话 id 变成真按钮（键盘可选），行点击作为鼠标捷径继续保留。
 */
import type { AppConfig, SessionSnapshot } from "../lib/types";
import { formatDateTime, renderCommand, runStateLabel } from "../lib/logModel";
import { AppIcon } from "./AppIcon";
import { IconEdit, IconPlay, IconRestart, IconStop, IconTrash } from "./ui/Icons";
import { EmptyNotice, KeyValueList } from "./ui/Primitives";

interface Props {
  app: AppConfig;
  sessions: SessionSnapshot[];
  selectedSession: string | null;
  busy: boolean;
  onStart: () => void;
  onStop: (sessionId: string) => void;
  onRestart: (sessionId: string, appId: string) => void;
  onEdit: () => void;
  onDelete: () => void;
  onSelectSession: (sessionId: string) => void;
  /** 图标刷新触发器：会话启动后 +1，重新抓运行时图标。 */
  iconRefresh?: number;
  /** 刷新时是否强制穿透缓存（会话刚启动那一次为真）。 */
  iconForceRefresh?: boolean;
}

export function AppDetail({
  app,
  sessions,
  selectedSession,
  busy,
  onStart,
  onStop,
  onRestart,
  onEdit,
  onDelete,
  onSelectSession,
  iconRefresh = 0,
  iconForceRefresh = false,
}: Props) {
  const envEntries = Object.entries(app.env ?? {});
  const tags = app.tags ?? [];
  const runningCount = sessions.filter((session) => session.state === "running").length;
  const hasRunning = runningCount > 0;

  const kvItems = [
    {
      label: "启动命令",
      value: <code className="mono-block">{renderCommand(app.command, app.args)}</code>,
    },
    {
      label: "工作目录",
      value: app.cwd ? <code>{app.cwd}</code> : <span className="muted">继承启动器当前目录</span>,
    },
    {
      label: "环境变量",
      value:
        envEntries.length === 0 ? (
          <span className="muted">无</span>
        ) : (
          <ul className="env-list">
            {envEntries.map(([key, value]) => (
              <li key={key}>
                <code>
                  {key}={value}
                </code>
              </li>
            ))}
          </ul>
        ),
    },
    {
      label: "标签",
      value:
        tags.length === 0 ? (
          <span className="muted">无</span>
        ) : (
          <span className="tag-list">
            {tags.map((tag) => (
              <span key={tag} className="tag">
                {tag}
              </span>
            ))}
          </span>
        ),
    },
    {
      label: "自定义图标",
      value: app.icon ? (
        <code>{app.icon}</code>
      ) : (
        <span className="muted">未设置（M3 抓取运行时图标）</span>
      ),
    },
  ];

  return (
    <section className="panel" aria-labelledby="app-detail-title">
      <div className="panel-head">
        <div className="panel-head-main">
          <AppIcon
            appId={app.id}
            name={app.name}
            size="lg"
            refreshKey={iconRefresh}
            forceRefresh={iconForceRefresh}
          />
          <div className="panel-heading">
            <h2 className="panel-title" id="app-detail-title">
              {app.name}
            </h2>
            <p className="panel-sub">
              id <code>{app.id}</code>
            </p>
          </div>
        </div>
        <div className="panel-actions">
          <button type="button" className="btn btn-primary" onClick={onStart} disabled={busy}>
            <IconPlay size={11} />
            启动
          </button>
          <button
            type="button"
            className="btn"
            onClick={() => {
              const target = sessions.find((session) => session.state === "running");
              if (target) {
                onStop(target.session_id);
              }
            }}
            disabled={busy || !hasRunning}
            title={hasRunning ? "停止最近启动的运行中会话" : "没有运行中的会话"}
          >
            <IconStop size={11} />
            停止
          </button>
          <button
            type="button"
            className="btn"
            onClick={() => {
              const target =
                sessions.find((session) => session.session_id === selectedSession) ?? sessions[0];
              if (target) {
                onRestart(target.session_id, app.id);
              }
            }}
            disabled={busy || sessions.length === 0}
            title={sessions.length === 0 ? "该应用还没有会话" : "停止旧会话并用同一配置重启"}
          >
            <IconRestart size={12} />
            重启
          </button>
          <button type="button" className="btn btn-ghost" onClick={onEdit} disabled={busy}>
            <IconEdit size={12} />
            编辑
          </button>
          <button
            type="button"
            className="btn btn-ghost btn-danger-text"
            onClick={onDelete}
            disabled={busy}
          >
            <IconTrash size={12} />
            删除
          </button>
        </div>
      </div>

      <KeyValueList items={kvItems} />

      <div className="subhead">
        <h3>活跃会话</h3>
        <span className="count">{sessions.length}</span>
      </div>

      {sessions.length === 0 ? (
        <EmptyNotice>还没有会话。点「启动」会创建一个新的会话并开始捕获输出。</EmptyNotice>
      ) : (
        <div className="table-scroll">
          <table className="data-table session-table" aria-label="该应用的会话列表">
            <thead>
              <tr>
                <th scope="col">会话</th>
                <th scope="col">PID</th>
                <th scope="col">状态</th>
                <th scope="col">启动时间</th>
                <th scope="col">退出</th>
                <th scope="col">
                  <span className="sr-only">操作</span>
                </th>
              </tr>
            </thead>
            <tbody>
              {sessions.map((session) => {
                const selected = session.session_id === selectedSession;
                return (
                  <tr
                    key={session.session_id}
                    className={selected ? "is-selected" : ""}
                    onClick={() => onSelectSession(session.session_id)}
                  >
                    <td className="cell-id">
                      <button
                        type="button"
                        className="row-link"
                        title={session.session_id}
                        aria-pressed={selected}
                        onClick={(event) => {
                          event.stopPropagation();
                          onSelectSession(session.session_id);
                        }}
                      >
                        {session.session_id.slice(0, 8)}
                      </button>
                    </td>
                    <td className="mono">{session.pid}</td>
                    <td>
                      <span className={`badge badge-${session.state}`}>
                        {runStateLabel(session.state)}
                      </span>
                    </td>
                    <td className="mono small">{formatDateTime(session.started_at)}</td>
                    <td className="mono small">
                      {session.last_exit
                        ? `${session.last_exit.reason}${
                            session.last_exit.code === null ? "" : ` (${session.last_exit.code})`
                          }`
                        : "—"}
                    </td>
                    <td className="cell-actions">
                      <button
                        type="button"
                        className="btn btn-mini"
                        disabled={busy || session.state !== "running"}
                        onClick={(event) => {
                          event.stopPropagation();
                          onStop(session.session_id);
                        }}
                      >
                        停止
                      </button>
                    </td>
                  </tr>
                );
              })}
            </tbody>
          </table>
        </div>
      )}
    </section>
  );
}