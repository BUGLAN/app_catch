/**
 * 应用详情：配置展示 + 操作按钮 + 该应用的活跃会话列表（多实例）。
 */
import type { AppConfig, SessionSnapshot } from "../lib/types";
import { formatDateTime, renderCommand, runStateLabel } from "../lib/logModel";

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
}: Props) {
  const envEntries = Object.entries(app.env ?? {});
  const runningCount = sessions.filter((session) => session.state === "running").length;
  const hasRunning = runningCount > 0;

  return (
    <section className="panel">
      <div className="panel-head">
        <div>
          <h2 className="panel-title">{app.name}</h2>
          <p className="panel-sub">
            id <code>{app.id}</code>
          </p>
        </div>
        <div className="panel-actions">
          <button type="button" className="btn btn-primary" onClick={onStart} disabled={busy}>
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
            停止
          </button>
          <button
            type="button"
            className="btn"
            onClick={() => {
              const target =
                sessions.find((session) => session.session_id === selectedSession) ??
                sessions[0];
              if (target) {
                onRestart(target.session_id, app.id);
              }
            }}
            disabled={busy || sessions.length === 0}
            title={sessions.length === 0 ? "该应用还没有会话" : "停止旧会话并用同一配置重启"}
          >
            重启
          </button>
          <button type="button" className="btn btn-ghost" onClick={onEdit} disabled={busy}>
            编辑
          </button>
          <button type="button" className="btn btn-ghost btn-danger-text" onClick={onDelete} disabled={busy}>
            删除
          </button>
        </div>
      </div>

      <dl className="kv">
        <dt>启动命令</dt>
        <dd>
          <code className="mono-block">{renderCommand(app.command, app.args)}</code>
        </dd>
        <dt>工作目录</dt>
        <dd>{app.cwd ? <code>{app.cwd}</code> : <span className="muted">继承启动器当前目录</span>}</dd>
        <dt>环境变量</dt>
        <dd>
          {envEntries.length === 0 ? (
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
          )}
        </dd>
        <dt>标签</dt>
        <dd>
          {(app.tags ?? []).length === 0 ? (
            <span className="muted">无</span>
          ) : (
            (app.tags ?? []).map((tag) => (
              <span key={tag} className="tag">
                {tag}
              </span>
            ))
          )}
        </dd>
        <dt>自定义图标</dt>
        <dd>{app.icon ? <code>{app.icon}</code> : <span className="muted">未设置（M3 抓取运行时图标）</span>}</dd>
      </dl>

      <div className="subhead">
        <h3>活跃会话</h3>
        <span className="count">{sessions.length}</span>
      </div>
      {sessions.length === 0 ? (
        <p className="muted small">还没有会话。点「启动」会创建一个新的会话并开始捕获输出。</p>
      ) : (
        <table className="session-table">
          <thead>
            <tr>
              <th>会话</th>
              <th>PID</th>
              <th>状态</th>
              <th>启动时间</th>
              <th>退出</th>
              <th aria-label="操作" />
            </tr>
          </thead>
          <tbody>
            {sessions.map((session) => (
              <tr
                key={session.session_id}
                className={session.session_id === selectedSession ? "is-selected" : ""}
                onClick={() => onSelectSession(session.session_id)}
              >
                <td>
                  <code className="short-id" title={session.session_id}>
                    {session.session_id.slice(0, 8)}
                  </code>
                </td>
                <td className="mono">{session.pid}</td>
                <td>
                  <span className={`badge badge-${session.state}`}>{runStateLabel(session.state)}</span>
                </td>
                <td className="mono small">{formatDateTime(session.started_at)}</td>
                <td className="mono small">
                  {session.last_exit
                    ? `${session.last_exit.reason}${session.last_exit.code === null ? "" : ` (${session.last_exit.code})`}`
                    : "—"}
                </td>
                <td>
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
            ))}
          </tbody>
        </table>
      )}
    </section>
  );
}
