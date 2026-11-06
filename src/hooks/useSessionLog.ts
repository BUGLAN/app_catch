/**
 * 会话日志 hook：增量游标 + 状态刷新，由后端推送驱动，低频轮询兜底。
 *
 * - 收到 `session:log-appended`（本会话）→ 立刻增量读日志；
 * - 收到 `sessions:changed`（本会话）→ 刷新状态快照并读日志；
 * - 兜底轮询（{@link pollInterval}）只防漏事件，页面隐藏时暂停。
 *
 * 调度交给 {@link createPoller}：同一时刻最多一轮，推送在读取途中到达时
 * 合并成「读完再补一轮」，增量游标绝不会被两轮同时推进。
 */
import { useCallback, useEffect, useRef, useState } from "react";

import { api, sessionEvents, toCommandError } from "../lib/api";
import {
  advanceCursor,
  appendRows,
  INITIAL_CURSOR,
  isSessionGone,
  PAGE_LINES,
  pollInterval,
  type LogCursor,
  type LogRow,
} from "../lib/logModel";
import { createPoller } from "../lib/poller";
import { noticeTouches } from "../lib/sessions";
import type { CommandError, RunState, SessionEvent, SessionSnapshot } from "../lib/types";

export interface SessionLogState {
  /** 渲染用的日志行（已裁剪到上限）。 */
  rows: LogRow[];
  /** 最近事件（结构化视图用）。 */
  recentEvents: SessionEvent[];
  /** 已跳过的不可解析行数。 */
  skippedLines: number;
  /** 历史是否被裁剪（日志分片被保留策略删掉过）。 */
  truncated: boolean;
  /** 会话是否已不在管理器里（重启启动器后的历史会话）。 */
  gone: boolean;
  /** 最近一次轮询的错误。 */
  error: CommandError | null;
  /** 当前会话快照（实时）。 */
  snapshot: SessionSnapshot;
  /** 手动清空日志缓冲。 */
  clear: () => void;
}

/** 最近事件列表保留的条数。 */
const RECENT_EVENTS_LIMIT = 200;

export function useSessionLog(session: SessionSnapshot): SessionLogState {
  const [snapshot, setSnapshot] = useState<SessionSnapshot>(session);
  const [rows, setRows] = useState<LogRow[]>([]);
  const [recentEvents, setRecentEvents] = useState<SessionEvent[]>([]);
  const [skippedLines, setSkippedLines] = useState(0);
  const [truncated, setTruncated] = useState(false);
  const [gone, setGone] = useState(false);
  const [error, setError] = useState<CommandError | null>(null);

  // 游标放在 ref 里：轮询回调需要读到最新值，但不该因为游标变化触发重渲染
  const cursorRef = useRef<LogCursor>(INITIAL_CURSOR);
  /** 最新的运行状态：兜底轮询的间隔要读它，但它变化时不该重建订阅与调度。 */
  const stateRef = useRef<RunState>(session.state);

  // 会话切换时重置全部状态。
  //
  // 依赖必须是 `session.session_id` 而不是整个 `session` 对象：父组件每次刷新会话列表
  // 都会生成新的对象，若依赖对象本身，这里会在每次刷新时把日志清空一次。
  // 快照字段的变化由下面的推送 / 兜底轮询自己刷新，不需要在这里同步。
  useEffect(() => {
    cursorRef.current = INITIAL_CURSOR;
    stateRef.current = session.state;
    setRows([]);
    setRecentEvents([]);
    setSkippedLines(0);
    setTruncated(false);
    setGone(false);
    setError(null);
    setSnapshot(session);
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [session.session_id]);

  const clear = useCallback(() => {
    // 只清界面缓冲，不动游标：清空后新事件仍然从当前位置继续追加
    setRows([]);
    setRecentEvents([]);
  }, []);

  useEffect(() => {
    const sessionId = session.session_id;
    let cancelled = false;
    /**
     * 下一轮是否要顺带刷新状态快照。
     *
     * 日志推送很频繁（输出多的进程每 50ms 一批），每次都查状态是白费 IPC；
     * 只有「会话变化」推送、首轮、以及兜底轮询时才查。
     */
    let statusDue = true;
    let lastStatusAt = 0;

    /** 读一页日志；返回是否还有积压需要立刻继续。 */
    const readOnce = async (): Promise<boolean> => {
      const from = cursorRef.current.nextLine;
      const page = await api.readSessionEvents(sessionId, from, PAGE_LINES);
      if (cancelled) {
        return false;
      }
      const { cursor, rows: newRows, events } = advanceCursor(cursorRef.current, page, from);
      cursorRef.current = cursor;

      setRows((prev) => appendRows(prev, newRows));
      setSkippedLines(cursor.skippedLines);
      setTruncated(cursor.truncated);
      if (events.length > 0) {
        setRecentEvents((prev) => {
          const merged = prev.concat(events);
          return merged.length > RECENT_EVENTS_LIMIT
            ? merged.slice(merged.length - RECENT_EVENTS_LIMIT)
            : merged;
        });
      }
      return cursor.nextLine >= 0 && page.has_more;
    };

    /**
     * 有积压就一直读（单轮最多 MAX_PAGES_PER_TICK 页，避免长时间占住主线程）。
     * 返回读完这一轮后是否还有积压。
     */
    const readLogs = async (): Promise<boolean> => {
      let more = true;
      let pages = 0;
      while (more && pages < MAX_PAGES_PER_TICK && !cancelled) {
        more = await readOnce();
        pages += 1;
      }
      return more;
    };

    const run = async () => {
      const fallbackDue =
        Date.now() - lastStatusAt >= pollInterval(stateRef.current === "running" ? 1 : 0);
      if (statusDue || fallbackDue) {
        statusDue = false;
        lastStatusAt = Date.now();
        try {
          const fresh = await api.getSessionStatus(sessionId);
          if (cancelled) {
            return;
          }
          stateRef.current = fresh.state;
          setSnapshot(fresh);
          setGone(false);
          setError(null);
        } catch (raw) {
          if (cancelled) {
            return;
          }
          const commandError = toCommandError(raw);
          setError(commandError);
          // 会话已不在管理器里（重启启动器前的旧会话）：保留已落盘日志，照常读文件
          if (isSessionGone(commandError)) {
            setGone(true);
          } else {
            return;
          }
        }
      }

      try {
        const backlog = await readLogs();
        // 还有积压（例如刚打开一个长跑会话、或进程一口气输出了几万行）：
        // 让出一帧给界面渲染，然后立刻接着读，而不是等下一次推送或兜底轮询
        if (backlog && !cancelled) {
          window.setTimeout(() => poller.kick(), 0);
        }
      } catch (raw) {
        if (!cancelled) {
          setError(toCommandError(raw));
        }
      }
    };

    const poller = createPoller({
      run,
      interval: () => pollInterval(stateRef.current === "running" ? 1 : 0),
      isVisible: () => document.visibilityState === "visible",
    });

    // 推送：日志有新内容就读；会话本身变了（退出、端口变化）就连状态一起刷新
    const unsubscribers: Array<Promise<() => void>> = [
      sessionEvents.onLogAppended((payload) => {
        if (noticeTouches(payload, sessionId)) {
          poller.kick();
        }
      }),
      sessionEvents.onSessionsChanged((payload) => {
        if (noticeTouches(payload, sessionId)) {
          statusDue = true;
          poller.kick();
        }
      }),
    ];

    // 页面重新可见：隐藏期间被跳过的刷新立刻补上
    const onVisible = () => {
      if (document.visibilityState === "visible") {
        statusDue = true;
        poller.kick();
      }
    };
    document.addEventListener("visibilitychange", onVisible);

    return () => {
      cancelled = true;
      poller.stop();
      document.removeEventListener("visibilitychange", onVisible);
      for (const unsubscribe of unsubscribers) {
        void unsubscribe.then((off) => off());
      }
    };
  }, [session.session_id]);

  return { rows, recentEvents, skippedLines, truncated, gone, error, snapshot, clear };
}

/** 单次 tick 最多连读多少页，防止一次轮询把主线程占太久。 */
const MAX_PAGES_PER_TICK = 5;

