/**
 * 应用详情：图标 + 配置展示 + 操作按钮 + 该应用的会话列表（多实例）。
 *
 * 会话列表包含运行中的会话和最近退出的几个（按启动先后排序），
 * 进程退出后用户仍能选中它、查看退出码与最后的输出。
 *
 * 进行中的操作按应用 / 按会话记录：启动 A 时只禁用 A 的「启动」，
 * 别的应用、别的会话照常可操作（before hook 可能要跑很久，例如 pnpm install）。
 *
 * 表现层改动：
 *   - 键值表改用共享 KeyValueList，长路径与长命令行统一走 overflow-wrap；
 *   - 操作按钮加图标与更明确的 disabled 说明（title 保留原语义）；
 *   - 会话表包进 .table-scroll，窄窗口下横向滚动而不是挤压变形；
 *   - 会话 id 变成真按钮（键盘可选），行点击作为鼠标捷径继续保留。
 */
import type { AppConfig, SessionSnapshot } from "../lib/types";
import { formatDateTime, renderCommand, runStateLabel } from "../lib/logModel";
import { latestRunning } from "../lib/sessions";
import { AppIcon } from "./AppIcon";
import { IconEdit, IconPlay, IconRestart, IconStop, IconTrash } from "./ui/Icons";
import { EmptyNotice, KeyValueList } from "./ui/Primitives";

interface Props {
  app: AppConfig;
  sessions: SessionSnapshot[];
  selectedSession: string | null;
  /** 本应用的启动 / 重启请求是否在进行中。 */
  starting: boolean;
  /** 正在停止的会话 id。 */
  stoppingIds: ReadonlySet<string>;
  /** 应用配置正在保存或删除（编辑 / 删除按钮随之禁用）。 */
  saving: boolean;
  onStart: () => void;
  onStop: (sessionId: string) => void;
  onRestart: (sessionId: string, appId: string) => void;
  onEdit: () => void;
  onDelete: () => void;
  onSelectSession: (sessionId: string) => void;
  /** 本应用的图标版本（会话启动并重抓图标后递增），见 AppIcon 的 `version`。 */
  iconVersion?: number;
}

export function AppDetail({
  app,
  sessions,
  selectedSession,
  starting,
  stoppingIds,
  saving,
  onStart,
  onStop,
  onRestart,
  onEdit,
  onDelete,
  onSelectSession,
  iconVersion = 0,
}: Props) {
  const envEntries = Object.entries(app.env ?? {});
  const tags = app.tags ?? [];
  const stopTarget = latestRunning(sessions);
  const stopTargetBusy = stopTarget !== null && stoppingIds.has(stopTarget.session_id);
  const restartTarget =
    sessions.find((session) => session.session_id === selectedSession) ??
    sessions[sessions.length - 1] ??
    null;

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
            version={iconVersion}
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
          <button
            type="button"
            className="btn btn-primary"
            onClick={onStart}
            disabled={starting}
            aria-busy={starting}
          >
            <IconPlay size={11} />
            {starting ? "启动中…" : "启动"}
          </button>
          <button
            type="button"
            className="btn"
            onClick={() => {
              if (stopTarget) {
                onStop(stopTarget.session_id);
              }
            }}
            disabled={stopTarget === null || stopTargetBusy}
            aria-busy={stopTargetBusy}
            title={stopTarget ? "停止最近启动的运行中会话" : "没有运行中的会话"}
          >
            <IconStop size={11} />
            {stopTargetBusy ? "停止中…" : "停止"}
          </button>
          <button
            type="button"
            className="btn"
            onClick={() => {
              if (restartTarget) {
                onRestart(restartTarget.session_id, app.id);
              }
            }}
            disabled={starting || restartTarget === null}
            title={restartTarget === null ? "该应用还没有会话" : "停止选中的会话并用同一配置重启"}
          >
            <IconRestart size={12} />
            重启
          </button>
          <button type="button" className="btn btn-ghost" onClick={onEdit} disabled={saving}>
            <IconEdit size={12} />
            编辑
          </button>
          <button
            type="button"
            className="btn btn-ghost btn-danger-text"
            onClick={onDelete}
            disabled={saving || starting}
          >
            <IconTrash size={12} />
            删除
          </button>
        </div>
      </div>

      <KeyValueList items={kvItems} />

      <div className="subhead">
        <h3>会话</h3>
        <span className="count">{sessions.length}</span>
      </div>

      {sessions.length === 0 ? (
        <EmptyNotice>还没有会话。点「启动」会创建一个新的会话并开始捕获输出。</EmptyNotice>
      ) : (
        <div className="table-scroll">
          <table className="data-table session-table" aria-label="该应用的会话列表">
            {/* 列宽与下方会话状态条共用一套（见 apps.css「会话列与状态条」） */}
            <colgroup>
              <col className="col-session" />
              <col className="col-pid" />
              <col className="col-state" />
              <col className="col-started" />
              <col />
              <col className="col-actions" />
            </colgroup>
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
                const stopping = stoppingIds.has(session.session_id);
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
                        disabled={stopping || session.state !== "running"}
                        aria-busy={stopping}
                        onClick={(event) => {
                          event.stopPropagation();
                          onStop(session.session_id);
                        }}
                      >
                        {stopping ? "停止中…" : "停止"}
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