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
 *   - 输出行不做严重程度着色：stderr 不等于出错，按内容猜又必然误判（见 logModel 的 LogRow.level）。
 *     来源流只用中性的 stdout / stderr 小标签标注，找错误交给日志搜索（⌘F / Ctrl+F）。
 */
import { useEffect, useLayoutEffect, useMemo, useRef, useState } from "react";

import { useSessionLog } from "../hooks/useSessionLog";
import {
  describeExit,
  exitReasonLabel,
  formatClock,
  findMatches,
  formatDateTime,
  runStateLabel,
  shouldAutoScroll,
  splitMatches,
  stepMatch,
  type LogRow,
} from "../lib/logModel";
import { detectPlatform } from "../lib/platform";
import type { SessionEvent, SessionSnapshot } from "../lib/types";
import { IconChevronDown, IconChevronUp, IconClose, IconSearch, IconTerminal } from "./ui/Icons";
import { Banner, EmptyNotice, Segmented, TabPanel } from "./ui/Primitives";

interface Props {
  session: SessionSnapshot;
  allSessions: SessionSnapshot[];
}

/** 视图切换：日志流 / 结构化事件列表。 */
type ViewMode = "log" | "events";

/** 聚焦日志搜索的快捷键文案（与标题栏 ⌘N 同一套平台判定）。 */
const FIND_SHORTCUT = detectPlatform() === "macos" ? "⌘F" : "Ctrl+F";

export function SessionDetail({ session, allSessions }: Props) {
  const { rows, recentEvents, skippedLines, truncated, gone, error, snapshot, clear } =
    useSessionLog(session);
  const [mode, setMode] = useState<ViewMode>("log");
  const [autoScroll, setAutoScroll] = useState(true);

  const [query, setQuery] = useState("");
  const [onlyMatches, setOnlyMatches] = useState(false);
  /** 当前命中在 matches 里的位置；-1 表示还没定位到任何一个。 */
  const [current, setCurrent] = useState(-1);
  /** 每次「定位到某个命中」自增一次，驱动滚动（current 不变时也要能重新滚过去）。 */
  const [jump, setJump] = useState(0);

  const logRef = useRef<HTMLDivElement | null>(null);
  const searchRef = useRef<HTMLInputElement | null>(null);
  const matches = useMemo(() => findMatches(rows, query), [rows, query]);
  const searching = query.length > 0;
  const currentRow = current >= 0 && current < matches.length ? matches[current]! : -1;
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

  // 切换会话时搜索状态归零：上一个会话的查询对新会话没有意义
  useEffect(() => {
    setQuery("");
    setOnlyMatches(false);
    setCurrent(-1);
  }, [session.session_id]);

  // 定位到命中：滚到视口中间，并暂停自动滚底，否则下一批输出会把视图拽走
  useEffect(() => {
    if (jump === 0) {
      return;
    }
    const target = logRef.current?.querySelector<HTMLElement>('[data-current-match="true"]');
    if (target) {
      setAutoScroll(false);
      target.scrollIntoView({ block: "center" });
    }
  }, [jump]);

  const goTo = (index: number) => {
    setCurrent(index);
    if (index >= 0) {
      setJump((value) => value + 1);
    }
  };

  const changeQuery = (next: string) => {
    setQuery(next);
    // 输入时定位到最新的命中：日志贴底阅读，最近的那条最可能是要找的
    const count = findMatches(rows, next).length;
    goTo(count > 0 ? count - 1 : -1);
  };

  const step = (direction: 1 | -1) => goTo(stepMatch(current, matches.length, direction));

  /*
   * ⌘F / Ctrl+F 聚焦日志搜索（WebView 没有可依赖的原生页内查找，且日志只保留最近 MAX_LOG_ROWS 行）。
   * 弹窗打开时不响应：焦点被困在弹窗里，抢走焦点会破坏它的键盘约定。
   */
  useEffect(() => {
    const isMac = document.documentElement.dataset.platform === "macos";
    const onKeyDown = (event: KeyboardEvent) => {
      const primary = isMac ? event.metaKey && !event.ctrlKey : event.ctrlKey && !event.metaKey;
      if (!primary || event.shiftKey || event.altKey || event.key.toLowerCase() !== "f") {
        return;
      }
      if (document.querySelector('[aria-modal="true"]')) {
        return;
      }
      event.preventDefault();
      setMode("log");
      // 切到日志视图后输入框才渲染，等一帧再聚焦
      window.requestAnimationFrame(() => {
        searchRef.current?.focus();
        searchRef.current?.select();
      });
    };
    window.addEventListener("keydown", onKeyDown);
    return () => window.removeEventListener("keydown", onKeyDown);
  }, []);

  const visibleRows: { row: LogRow; index: number }[] =
    searching && onlyMatches
      ? matches.map((index) => ({ row: rows[index]!, index }))
      : rows.map((row, index) => ({ row, index }));

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

  const describeExit = (exit: NonNullable<typeof lastExit>) =>
    `${exit.code === null ? "无退出码" : `码 ${exit.code}`} · ${exitReasonLabel(exit.reason)}`;

  // 字段顺序与上方「活跃会话」表格一致（会话 / PID / 状态 / 启动时间 / 退出），列宽也共用一套
  const statusItems = [
    {
      label: "会话",
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
      label: "退出",
      value: lastExit ? (
        <span className="mono" title={describeExit(lastExit)}>
          {describeExit(lastExit)}
        </span>
      ) : (
        <span className="muted">—</span>
      ),
    },
    {
      label: "窗口标题",
      value: windowTitle ? (
        <span title={windowTitle}>{windowTitle}</span>
      ) : (
        <span className="muted">未采集到</span>
      ),
    },
    {
      label: "监听端口",
      value: (
        <span className="mono">
          {ports.length > 0 ? ports.join(", ") : <span className="muted">无</span>}
        </span>
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
          <div className="status-cell" key={item.label}>
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
            <div className="log-search" role="search">
              <span className="log-search-icon" aria-hidden="true">
                <IconSearch size={13} />
              </span>
              <input
                ref={searchRef}
                type="search"
                className="log-search-input"
                value={query}
                onChange={(event) => changeQuery(event.target.value)}
                onKeyDown={(event) => {
                  if (event.key === "Enter") {
                    event.preventDefault();
                    step(event.shiftKey ? -1 : 1);
                  } else if (event.key === "Escape") {
                    event.preventDefault();
                    if (searching) {
                      changeQuery("");
                    } else {
                      event.currentTarget.blur();
                    }
                  }
                }}
                placeholder={`搜索日志（${FIND_SHORTCUT}）`}
                aria-label="搜索日志"
                aria-keyshortcuts={FIND_SHORTCUT === "⌘F" ? "Meta+F" : "Control+F"}
                spellCheck={false}
                autoComplete="off"
              />
              {searching ? (
                <>
                  <span className="log-search-count" aria-live="polite">
                    {matches.length === 0
                      ? "无结果"
                      : `${currentRow >= 0 ? current + 1 : "–"} / ${matches.length}`}
                  </span>
                  <button
                    type="button"
                    className="btn btn-icon"
                    onClick={() => step(-1)}
                    disabled={matches.length === 0}
                    aria-label="上一个匹配"
                    title="上一个（Shift+Enter）"
                  >
                    <IconChevronUp size={13} />
                  </button>
                  <button
                    type="button"
                    className="btn btn-icon"
                    onClick={() => step(1)}
                    disabled={matches.length === 0}
                    aria-label="下一个匹配"
                    title="下一个（Enter）"
                  >
                    <IconChevronDown size={13} />
                  </button>
                  <button
                    type="button"
                    className="btn btn-icon"
                    onClick={() => changeQuery("")}
                    aria-label="清空搜索"
                    title="清空搜索（Esc）"
                  >
                    <IconClose size={11} />
                  </button>
                </>
              ) : null}
            </div>
          ) : null}
          {mode === "log" && searching ? (
            <label className="checkbox">
              <input
                type="checkbox"
                checked={onlyMatches}
                onChange={(event) => setOnlyMatches(event.target.checked)}
              />
              只看匹配行
            </label>
          ) : null}
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
        ) : searching && onlyMatches && visibleRows.length === 0 ? (
          <EmptyNotice icon={<IconSearch size={14} />}>没有包含「{query}」的行。</EmptyNotice>
        ) : (
          visibleRows.map(({ row, index }) => (
            <LogLine
              key={row.key}
              row={row}
              query={query}
              isCurrent={index === currentRow}
            />
          ))
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
function LogLine({ row, query, isCurrent }: { row: LogRow; query: string; isCurrent: boolean }) {
  const matchAttr = isCurrent ? { "data-current-match": "true" } : {};
  const currentClass = isCurrent ? " is-current-match" : "";

  if (row.kind === "divider") {
    return (
      <div className={`log-divider log-divider-${row.level}${currentClass}`} {...matchAttr}>
        <span className="log-divider-time">{formatClock(row.ts)}</span>
        <span className="log-divider-text">
          <Highlighted text={row.text} query={query} />
        </span>
      </div>
    );
  }

  if (row.kind === "notice") {
    return (
      <div className={`log-notice${currentClass}`} {...matchAttr}>
        <span className="log-notice-time">{formatClock(row.ts)}</span>
        <span className="log-notice-text">
          <Highlighted text={row.text} query={query} />
        </span>
      </div>
    );
  }

  const stream = row.stream ?? "stdout";

  return (
    <div
      className={`log-line${row.lossy ? " is-lossy" : ""}${currentClass}`}
      {...matchAttr}
    >
      <span className="log-time">{formatClock(row.ts)}</span>
      {/* 来源流只是事实标注，不带颜色语义：stderr 不等于出错 */}
      <span className="log-stream" title={stream === "stderr" ? "来自标准错误流" : "来自标准输出流"}>
        {stream}
      </span>
      {row.lossy ? (
        <span
          className="log-lossy"
          title="该行原始字节既不是合法 UTF-8 也无法按 GBK 解码，内容已被替换字符替代，不可还原"
        >
          编码损坏
        </span>
      ) : null}
      <span className="log-text">
        <Highlighted text={row.text} query={query} />
      </span>
    </div>
  );
}

/** 把搜索命中渲染成 <mark>；没有查询时直接输出原文。 */
function Highlighted({ text, query }: { text: string; query: string }) {
  if (query.length === 0) {
    return <>{text}</>;
  }
  return (
    <>
      {splitMatches(text, query).map((segment, index) =>
        segment.match ? (
          <mark className="log-match" key={index}>
            {segment.text}
          </mark>
        ) : (
          segment.text
        )
      )}
    </>
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