/**
 * 会话详情：顶部状态条 + 实时日志流 + 最近事件列表。
 *
 * 两条「不静默」的硬要求都在这里体现：
 * - `encoding === "lossy"` 的行带醒目的「编码损坏」标记；
 * - `overflow` 事件渲染成一条独立提示（「有 N 行被丢弃」）。
 *
 * 日志里 `status` / `exit` 事件渲染成分隔行，不与进程输出混在一起。
 */
import { useEffect, useLayoutEffect, useMemo, useRef, useState } from "react";

import { useSessionLog } from "../hooks/useSessionLog";
import {
  describeExit,
  exitReasonLabel,
  formatClock,
  formatDateTime,
  runStateLabel,
  shouldAutoScroll,
} from "../lib/logModel";
import type { SessionEvent, SessionSnapshot } from "../lib/types";

interface Props {
  session: SessionSnapshot;
  allSessions: SessionSnapshot[];
}

/** 视图切换：日志流 / 结构化事件列表。 */
type ViewMode = "log" | "events";

export function SessionDetail({ session, allSessions }: Props) {
  const { rows, recentEvents, skippedLines, truncated, gone, error, snapshot, clear } =
    useSessionLog(session);
  const [mode, setMode] = useState<ViewMode>("log");
  const [autoScroll, setAutoScroll] = useState(true);

  const logRef = useRef<HTMLDivElement | null>(null);

  // 新行到达后：只有原本就在底部才跟随滚动
  useLayoutEffect(() => {
    const element = logRef.current;
    if (!element || !autoScroll) {
      return;
    }
    element.scrollTop = element.scrollHeight;
  }, [rows, autoScroll]);

  const handleScroll = () => {
    const element = logRef.current;
    if (!element) {
      return;
    }
    setAutoScroll(shouldAutoScroll(element.scrollTop, element.scrollHeight, element.clientHeight));
  };

  // 切回日志视图时如果用户仍在底部，重新贴底
  useEffect(() => {
    const element = logRef.current;
    if (mode === "log" && element && autoScroll) {
      element.scrollTop = element.scrollHeight;
    }
  }, [mode, autoScroll]);

  const ports = snapshot.latest_status?.ports ?? [];
  const windowTitle = snapshot.latest_status?.window_title ?? null;
  const lastExit = snapshot.last_exit;

  const live = useMemo(
    () => allSessions.some((item) => item.session_id === snapshot.session_id),
    [allSessions, snapshot.session_id]
  );

  return (
    <section className="panel session-panel">
      <div className="session-statusbar">
        <div className="status-cell">
          <span className="status-label">会话</span>
          <code className="status-value short-id" title={snapshot.session_id}>
            {snapshot.session_id}
          </code>
        </div>
        <div className="status-cell">
          <span className="status-label">PID</span>
          <span className="status-value mono">{snapshot.pid}</span>
        </div>
        <div className="status-cell">
          <span className="status-label">状态</span>
          <span className={`badge badge-${snapshot.state}`}>{runStateLabel(snapshot.state)}</span>
        </div>
        <div className="status-cell">
          <span className="status-label">启动时间</span>
          <span className="status-value mono">{formatDateTime(snapshot.started_at)}</span>
        </div>
        <div className="status-cell">
          <span className="status-label">窗口标题</span>
          <span className="status-value">{windowTitle ?? <span className="muted">未采集到</span>}</span>
        </div>
        <div className="status-cell">
          <span className="status-label">监听端口</span>
          <span className="status-value mono">
            {ports.length > 0 ? ports.join(", ") : <span className="muted">无</span>}
          </span>
        </div>
        <div className="status-cell">
          <span className="status-label">退出</span>
          <span className="status-value">
            {lastExit ? (
              <span className="mono">
                {lastExit.code === null ? "无退出码" : `码 ${lastExit.code}`} ·{" "}
                {exitReasonLabel(lastExit.reason)}
              </span>
            ) : (
              <span className="muted">—</span>
            )}
          </span>
        </div>
      </div>

      {gone ? (
        <p className="banner banner-warn">
          该会话已不在启动器的运行表中（可能是重启启动器前的旧会话）。下方日志来自已落盘的事件文件，
          进程状态与实时输出无法再接管。
        </p>
      ) : null}
      {!live && !gone && snapshot.state === "running" ? (
        <p className="banner banner-info">该会话已退出运行表，正在读取残留日志。</p>
      ) : null}
      {truncated ? (
        <p className="banner banner-warn">
          日志分片已被保留策略裁剪，较早的输出不再可读（当前展示的是裁剪后的事件）。
        </p>
      ) : null}
      {skippedLines > 0 ? (
        <p className="banner banner-warn">有 {skippedLines} 行事件无法解析（可能是进程被强杀留下的半行）。</p>
      ) : null}
      {error && !gone ? <p className="banner banner-error">轮询失败：{error.message}</p> : null}

      <div className="session-toolbar">
        <div className="segmented">
          <button
            type="button"
            className={`seg${mode === "log" ? " is-active" : ""}`}
            onClick={() => setMode("log")}
          >
            实时日志
          </button>
          <button
            type="button"
            className={`seg${mode === "events" ? " is-active" : ""}`}
            onClick={() => setMode("events")}
          >
            最近事件 ({recentEvents.length})
          </button>
        </div>
        <div className="toolbar-right">
          {mode === "log" ? (
            <label className="checkbox">
              <input
                type="checkbox"
                checked={autoScroll}
                onChange={(event) => {
                  const next = event.target.checked;
                  setAutoScroll(next);
                  const element = logRef.current;
                  if (next && element) {
                    element.scrollTop = element.scrollHeight;
                  }
                }}
              />
              自动滚底
            </label>
          ) : null}
          <button type="button" className="btn btn-mini btn-ghost" onClick={clear}>
            清空视图
          </button>
        </div>
      </div>

      {mode === "log" ? (
        <div className="log-view" ref={logRef} onScroll={handleScroll}>
          {rows.length === 0 ? (
            <p className="muted small log-empty">暂无输出。进程一旦写 stdout/stderr，这里会自动追加。</p>
          ) : (
            rows.map((row) => <LogLine key={row.key} row={row} />)
          )}
        </div>
      ) : (
        <div className="events-view">
          {recentEvents.length === 0 ? (
            <p className="muted small log-empty">暂无事件。</p>
          ) : (
            <table className="events-table">
              <thead>
                <tr>
                  <th>时间</th>
                  <th>事件</th>
                  <th>摘要</th>
                </tr>
              </thead>
              <tbody>
                {recentEvents.map((event, index) => (
                  <tr key={`${index}-${event.ts}`}>
                    <td className="mono small">{formatClock(event.ts)}</td>
                    <td>
                      <span className={`ev-tag ev-${event.event}`}>{event.event}</span>
                    </td>
                    <td className="small">{describeEvent(event)}</td>
                  </tr>
                ))}
              </tbody>
            </table>
          )}
        </div>
      )}
    </section>
  );
}

/** 单条日志行：分隔行 / 提示行 / 普通输出行三种形态。 */
function LogLine({ row }: { row: ReturnType<typeof useSessionLog>["rows"][number] }) {
  if (row.kind === "divider") {
    return (
      <div className={`log-divider log-divider-${row.level}`}>
        <span className="log-divider-time">{formatClock(row.ts)}</span>
        <span className="log-divider-text">{row.text}</span>
      </div>
    );
  }

  if (row.kind === "notice") {
    return (
      <div className="log-notice">
        <span className="log-notice-time">{formatClock(row.ts)}</span>
        <span className="log-notice-text">{row.text}</span>
      </div>
    );
  }

  return (
    <div className={`log-line log-${row.stream ?? "stdout"}${row.lossy ? " is-lossy" : ""}`}>
      <span className="log-time">{formatClock(row.ts)}</span>
      <span className={`log-stream log-stream-${row.stream ?? "stdout"}`}>
        {row.stream === "stderr" ? "ERR" : "OUT"}
      </span>
      {row.lossy ? (
        <span className="log-lossy" title="该行原始字节既不是合法 UTF-8 也无法按 GBK 解码，内容已被替换字符替代，不可还原">
          编码损坏
        </span>
      ) : null}
      <span className="log-text">{row.text}</span>
    </div>
  );
}

/** 结构化事件列表里的事件摘要。 */
function describeEvent(event: SessionEvent): string {
  switch (event.event) {
    case "start":
      return `pid ${event.pid} · ${event.cmd} · cwd ${event.cwd}`;
    case "output":
      return `[${event.stream}] ${event.line}${(event.encoding ?? "utf-8") === "lossy" ? "（编码损坏）" : ""}`;
    case "status":
      return `${runStateLabel(event.state)} · 窗口 ${event.window_title ?? "—"} · 端口 ${
        event.ports && event.ports.length > 0 ? event.ports.join(", ") : "—"
      }`;
    case "exit":
      return describeExit(event.code ?? null, event.reason);
    case "overflow":
      return `丢弃 ${event.dropped} 行`;
    default:
      return "";
  }
}
