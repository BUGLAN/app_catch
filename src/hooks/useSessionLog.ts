/**
 * 会话日志轮询 hook：增量游标 + 状态刷新 + 页面隐藏暂停。
 *
 * 为什么用 setTimeout 递归而不是 setInterval：
 * 一次请求可能比间隔更慢，setInterval 会堆积请求；递归调度保证
 * 「上一轮结束才开始下一轮」，并能在页面隐藏时真正停下来。
 */
import { useCallback, useEffect, useRef, useState } from "react";

import { api, toCommandError } from "../lib/api";
import {
  advanceCursor,
  appendRows,
  formatDateTime,
  INITIAL_CURSOR,
  isSessionGone,
  PAGE_LINES,
  pollInterval,
  type LogCursor,
  type LogRow,
} from "../lib/logModel";
import type { CommandError, SessionEvent, SessionSnapshot } from "../lib/types";

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
  const sessionIdRef = useRef(session.session_id);

  // 会话切换（同组件复用时）重置全部状态
  useEffect(() => {
    sessionIdRef.current = session.session_id;
    cursorRef.current = INITIAL_CURSOR;
    setRows([]);
    setRecentEvents([]);
    setSkippedLines(0);
    setTruncated(false);
    setGone(false);
    setError(null);
    setSnapshot(session);
  }, [session]);

  const clear = useCallback(() => {
    // 只清界面缓冲，不动游标：清空后新事件仍然从当前位置继续追加
    setRows([]);
    setRecentEvents([]);
  }, []);

  useEffect(() => {
    let timer: number | undefined;
    let cancelled = false;

    /** 读一页日志；返回是否还有积压需要立刻继续。 */
    const readOnce = async (sessionId: string): Promise<boolean> => {
      const from = cursorRef.current.nextLine;
      const page = await api.readSessionEvents(sessionId, from, PAGE_LINES);
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

    const tick = async () => {
      const sessionId = sessionIdRef.current;

      // 页面不可见时完全暂停：既不读日志也不查状态
      if (document.visibilityState !== "visible") {
        timer = window.setTimeout(tick, pollInterval(snapshot.state === "running" ? 1 : 0));
        return;
      }

      try {
        const fresh = await api.getSessionStatus(sessionId);
        if (cancelled) {
          return;
        }
        setSnapshot(fresh);
        setGone(false);
        setError(null);

        // 有积压就一直读到追平（受 MAX_PAGES_PER_TICK 保护，避免长时间占住主线程）
        let more = true;
        let pages = 0;
        while (more && pages < MAX_PAGES_PER_TICK && !cancelled) {
          more = await readOnce(sessionId);
          pages += 1;
        }
      } catch (raw) {
        const commandError = toCommandError(raw);
        if (cancelled) {
          return;
        }
        setError(commandError);
        // 会话已不在管理器里（重启启动器）：保留已落盘日志，停止轮询状态
        if (isSessionGone(commandError)) {
          setGone(true);
          try {
            let more = true;
            let pages = 0;
            while (more && pages < MAX_PAGES_PER_TICK && !cancelled) {
              more = await readOnce(sessionId);
              pages += 1;
            }
          } catch {
            // 连日志都读不到就保持现状，界面已有 gone 提示
          }
        }
      }

      if (cancelled) {
        return;
      }
      const running = snapshot.state === "running";
      timer = window.setTimeout(tick, pollInterval(running ? 1 : 0));
    };

    timer = window.setTimeout(tick, 0);
    return () => {
      cancelled = true;
      if (timer !== undefined) {
        window.clearTimeout(timer);
      }
    };
  }, [session.session_id, snapshot.state]);

  return { rows, recentEvents, skippedLines, truncated, gone, error, snapshot, clear };
}

/** 单次 tick 最多连读多少页，防止一次轮询把主线程占太久。 */
const MAX_PAGES_PER_TICK = 5;

/** 供界面展示的快照文案工具（避免组件里散落格式化逻辑）。 */
export function describeStartedAt(ts: string): string {
  return formatDateTime(ts);
}
