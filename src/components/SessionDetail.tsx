/**
 * 会话详情：顶部状态条 + 实时日志流 + 最近事件列表。
 *
 * 两条「不静默」的硬要求都在这里体现：
 * - `encoding === "lossy"` 的行带醒目的「编码损坏」标记；
 * - `overflow` 事件渲染成一条独立提示（「有 N 行被丢弃」）。
 *
 * 日志里 `status` / `exit` 事件渲染成分隔行，不与进程输出混在一起。
 *
 * 表现层改动：
 *   - 状态条用 grid 自适应 + 会话 id 占两列完整显示（原来被 ellipsis 截断）；
 *   - 徽标不再被 flex 拉伸成整格宽度；
 *   - 视图切换用共享 Segmented（tablist/tab 语义，方向键可用）；
 *   - 日志/事件两个视图共用 .scroll-surface（原来是两份一模一样的六行声明）；
 *   - 日志行按 level 分色：原来的 .log-error 类名从没被渲染过（只按 stream 上色），
 *     所以真实错误和普通 stderr 长得一样。现在用 logModel 已经算好的 level。
 */
import { useEffect, useLayoutEffect, useRef, useState } from "react";

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
import { IconTerminal } from "./ui/Icons";
import { Banner, EmptyNotice, Segmented, TabPanel } from "./ui/Primitives";

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
  const headingId = "session-heading";
  const panelPrefix = "session-view";

  // 新行到达后：只有原本就在底部才跟随滚动
  useLayoutEffect(() => {
    const element = logRef.current;
    if (!element || !autoScroll || mode !== "log") {
      return;
    }
    element.scrollTop = element.scrollHeight;
  }, [rows, autoScroll, mode]);

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

  const live = allSessions.some((item) => item.session_id === snapshot.session_id);

  const statusItems = [
    {
      label: "会话",
      wide: true,
      value: (
        <code className="session-id" title={snapshot.session_id}>
          {snapshot.session_id}
        </code>
      ),
    },
    {
      label: "PID",
      value: <span className="mono">{snapshot.pid}</span>,
    },
    {
      label: "状态",
      value: <span className={`badge badge-${snapshot.state}`}>{runStateLabel(snapshot.state)}</span>,
    },
    {
      label: "启动时间",
      value: <span className="mono">{formatDateTime(snapshot.started_at)}</span>,
    },
    {
      label: "窗口标题",
      value: windowTitle ?? <span className="muted">未采集到</span>,
    },
    {
      label: "监听端口",
      value: (
        <span className="mono">
          {ports.length > 0 ? ports.join(", ") : <span className="muted">无</span>}
        </span>
      ),
    },
    {
      label: "退出",
      value: lastExit ? (
        <span className="mono">
          {lastExit.code === null ? "无退出码" : `码 ${lastExit.code}`} ·{" "}
          {exitReasonLabel(lastExit.reason)}
        </span>
      ) : (
        <span className="muted">—</span>
      ),
    },
  ];

  return (
    <section className="panel session-panel" aria-labelledby={headingId}>
      <h3 className="sr-only" id={headingId}>
        会话详情
      </h3>

      <div className="session-statusbar">
        {statusItems.map((item) => (
          <div
            className={"status-cell" + (item.wide ? " status-cell-wide" : "")}
            key={item.label}
          >
            <span className="status-label">{item.label}</span>
            <span className="status-value">{item.value}</span>
          </div>
        ))}
      </div>

      {gone ? (
        <Banner tone="warn">
          该会话已不在启动器的运行表中（可能是重启启动器前的旧会话）。下方日志来自已落盘的事件文件，
          进程状态与实时输出无法再接管。
        </Banner>
      ) : null}
      {!live && !gone && snapshot.state === "running" ? (
        <Banner tone="info">该会话已退出运行表，正在读取残留日志。</Banner>
      ) : null}
      {truncated ? (
        <Banner tone="warn">
          日志分片已被保留策略裁剪，较早的输出不再可读（当前展示的是裁剪后的事件）。
        </Banner>
      ) : null}
      {skippedLines > 0 ? (
        <Banner tone="warn">
          有 {skippedLines} 行事件无法解析（可能是进程被强杀留下的半行）。
        </Banner>
      ) : null}
      {error && !gone ? <Banner tone="error">轮询失败：{error.message}</Banner> : null}

      <div className="session-toolbar">
        <Segmented<ViewMode>
          ariaLabel="日志视图"
          value={mode}
          onChange={setMode}
          panelId={panelPrefix}
          options={[
            { value: "log", label: "实时日志" },
            { value: "events", label: `最近事件 (${recentEvents.length})` },
          ]}
        />
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

      <TabPanel
        panelId={panelPrefix}
        value="log"
        active={mode === "log"}
        label="实时日志输出"
        className="scroll-surface log-view"
        ref={logRef}
        onScroll={handleScroll}
      >
        {rows.length === 0 ? (
          <EmptyNotice icon={<IconTerminal size={14} />}>
            暂无输出。进程一旦写 stdout/stderr，这里会自动追加。
          </EmptyNotice>
        ) : (
          rows.map((row) => <LogLine key={row.key} row={row} />)
        )}
      </TabPanel>

      <TabPanel
        panelId={panelPrefix}
        value="events"
        active={mode === "events"}
        label="最近结构化事件"
        className="scroll-surface events-view"
      >
          {recentEvents.length === 0 ? (
            <EmptyNotice>暂无事件。</EmptyNotice>
          ) : (
            <table className="events-table">
              <caption className="sr-only">最近 {recentEvents.length} 条会话事件</caption>
              <thead>
                <tr>
                  <th scope="col">时间</th>
                  <th scope="col">事件</th>
                  <th scope="col">摘要</th>
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
      </TabPanel>
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

  const stream = row.stream ?? "stdout";
  // level 由 logModel 按内容判定：error 才是真错误，普通 stderr 只是措辞提醒
  const levelClass = row.level === "error" ? " log-error" : "";

  return (
    <div className={`log-line log-${stream}${levelClass}${row.lossy ? " is-lossy" : ""}`}>
      <span className="log-time">{formatClock(row.ts)}</span>
      <span className={`log-stream log-stream-${stream}`}>{stream === "stderr" ? "ERR" : "OUT"}</span>
      {row.lossy ? (
        <span
          className="log-lossy"
          title="该行原始字节既不是合法 UTF-8 也无法按 GBK 解码，内容已被替换字符替代，不可还原"
        >
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
      return `[${event.stream}] ${event.line}${
        (event.encoding ?? "utf-8") === "lossy" ? "（编码损坏）" : ""
      }`;
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