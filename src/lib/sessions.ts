/**
 * 会话列表的纯逻辑：默认选中、合并、应用状态聚合、推送事件匹配。
 *
 * 会话列表来自 `list_sessions`：全部运行中的会话 + 每个应用最近退出的几个，
 * **按启动先后排序**（最后一个是最近启动的）。下面的规则都依赖这个顺序。
 */
import type { RunState, SessionSnapshot } from "./types";

/** 后端推送事件的载荷（与 `src-tauri/src/notices.rs` 的 NoticePayload 一致）。 */
export interface NoticePayload {
  /** 涉及的会话 id；`null` 表示通知有积压丢失，所有会话都可能变了。 */
  session_ids: string[] | null;
}

/** 一条推送是否与某个会话有关。 */
export function noticeTouches(payload: NoticePayload, sessionId: string): boolean {
  return payload.session_ids === null || payload.session_ids.includes(sessionId);
}

/**
 * 应用的默认选中会话：最近启动的运行中会话；都退出了就选最近启动的那个。
 *
 * 这样进程刚崩溃时，界面停在它的日志上，而不是跳走或清空。
 */
export function pickDefaultSession(sessions: SessionSnapshot[]): string | null {
  for (let index = sessions.length - 1; index >= 0; index -= 1) {
    if (sessions[index]!.state === "running") {
      return sessions[index]!.session_id;
    }
  }
  return sessions.length > 0 ? sessions[sessions.length - 1]!.session_id : null;
}

/** 最近启动的运行中会话（「停止」按钮的目标）。 */
export function latestRunning(sessions: SessionSnapshot[]): SessionSnapshot | null {
  for (let index = sessions.length - 1; index >= 0; index -= 1) {
    if (sessions[index]!.state === "running") {
      return sessions[index]!;
    }
  }
  return null;
}

/**
 * 把刚启动的会话并进列表（已存在就不重复追加）。
 *
 * 推送触发的列表刷新可能比 `start_app` 的返回值先到，直接 concat 会出现重复 key。
 */
export function upsertSession(
  sessions: SessionSnapshot[],
  snapshot: SessionSnapshot
): SessionSnapshot[] {
  return sessions.some((session) => session.session_id === snapshot.session_id)
    ? sessions
    : sessions.concat(snapshot);
}

/**
 * 一个应用的综合状态：有运行中会话即运行中，否则看**最近一次**会话怎么结束的。
 *
 * 只看最近一次：更早的会话崩溃过、但最近一次正常退出时，应用不该一直挂着「已崩溃」。
 */
export function aggregateState(
  appId: string,
  sessions: SessionSnapshot[]
): RunState | "idle" {
  const own = sessions.filter((session) => session.app_id === appId);
  if (own.length === 0) {
    return "idle";
  }
  if (own.some((session) => session.state === "running")) {
    return "running";
  }
  return own[own.length - 1]!.state;
}
