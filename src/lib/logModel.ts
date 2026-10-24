/**
 * 会话日志的纯逻辑层：事件 -> 日志行、游标推进、滚动判定。
 *
 * 刻意与 React 解耦，方便直接单测（见 logModel.test.ts）：
 * 这里既不做 IO 也不碰 DOM，输入输出都是普通对象。
 */
import type {
  CommandError,
  ExitReason,
  RunState,
  SessionEvent,
  SessionEventsPage,
  StreamKind,
} from "./types";

/**
 * 日志行的展示形态。
 *
 * - `output`：一行进程输出；
 * - `divider`：状态/退出等控制事件，渲染成分隔行而不是日志行
 *   （需求要求「不要混在日志行里」）；
 * - `notice`：overflow / 无法解析等显式提示，必须可见、不能静默。
 */
export type LogRowKind = "output" | "divider" | "notice";

/** 渲染用的单条日志行。 */
export interface LogRow {
  /** 稳定 key：全局行号 + 事件序号，保证 React 列表复用不串行。 */
  key: string;
  kind: LogRowKind;
  /** 事件时间戳（RFC3339）。 */
  ts: string;
  /** 输出流（仅 output 行有）。 */
  stream?: StreamKind;
  /** 行内容或提示文案。 */
  text: string;
  /** 是否是有损解码（原始字节不可还原），需要醒目标记。 */
  lossy: boolean;
  /** 严重程度，决定配色。 */
  level: "normal" | "warn" | "error" | "info";
}

/** 增量读取的状态机快照。 */
export interface LogCursor {
  /** 下一次读取的全局行号。 */
  nextLine: number;
  /** 累计读到的原始事件数（用于「最近事件列表」截断）。 */
  totalEvents: number;
  /** 累计跳过的不可解析行数。 */
  skippedLines: number;
  /** 历史是否被裁剪（游标发生回退）。 */
  truncated: boolean;
}

/** 初始游标。 */
export const INITIAL_CURSOR: LogCursor = {
  nextLine: 0,
  totalEvents: 0,
  skippedLines: 0,
  truncated: false,
};

/** 日志缓冲区上限：超出后丢弃最旧的行，防止长跑会话把内存吃满。 */
export const MAX_LOG_ROWS = 4000;

/** 每次翻页请求的条数。 */
export const PAGE_LINES = 500;

/** 一次轮询的合并结果。 */
export interface LogPageResult {
  /** 新增的日志行（已按需求转换成 output/divider/notice）。 */
  rows: LogRow[];
  /** 推进后的游标。 */
  cursor: LogCursor;
  /** 本轮新增的原始事件（供「最近事件列表」使用）。 */
  events: SessionEvent[];
  /** 是否需要立刻再读一页（还有积压）。 */
  hasMore: boolean;
}

/**
 * 推进游标并产出一页日志行。
 *
 * 关键边界：若服务端返回的 `next_line` **小于**传入的游标，说明历史分片被
 * 保留策略裁剪、全局行号整体前移了。此时不能直接把游标往回拨——那会让界面
 * 反复重读旧的活跃片。正确做法是标记 `truncated` 并接受新的行号基准。
 */
export function advanceCursor(
  prev: LogCursor,
  page: SessionEventsPage,
  baseLine: number
): { cursor: LogCursor; rows: LogRow[]; events: SessionEvent[] } {
  const truncated = page.next_line < baseLine;
  const rows: LogRow[] = [];
  page.events.forEach((event, index) => {
    rows.push(...eventToRows(event, baseLine + index));
  });

  const cursor: LogCursor = {
    // 游标只前进（除非被裁剪）：防止服务端异常导致界面重复消费
    nextLine: truncated ? page.next_line : Math.max(prev.nextLine, page.next_line),
    totalEvents: prev.totalEvents + page.events.length,
    skippedLines: prev.skippedLines + page.skipped_lines,
    truncated: prev.truncated || truncated,
  };

  return { cursor, rows, events: page.events };
}

/** 把一条原始事件转换成若干日志行。 */
export function eventToRows(event: SessionEvent, lineNo: number): LogRow[] {
  switch (event.event) {
    case "output": {
      const lossy = (event.encoding ?? "utf-8") === "lossy";
      return [
        {
          key: `${lineNo}-${event.ts}`,
          kind: "output",
          ts: event.ts,
          stream: event.stream,
          text: event.line,
          lossy,
          level: lossy ? "warn" : event.stream === "stderr" ? "error" : "normal",
        },
      ];
    }
    case "status":
      return [
        {
          key: `${lineNo}-${event.ts}`,
          kind: "divider",
          ts: event.ts,
          text: describeStatus(event.state, event.window_title ?? null, event.ports ?? null),
          lossy: false,
          level: "info",
        },
      ];
    case "exit":
      return [
        {
          key: `${lineNo}-${event.ts}`,
          kind: "divider",
          ts: event.ts,
          text: describeExit(event.code ?? null, event.reason),
          lossy: false,
          level: event.reason === "normal" ? "info" : "warn",
        },
      ];
    case "overflow":
      // 必须显式展示：静默丢行是不可接受的
      return [
        {
          key: `${lineNo}-${event.ts}`,
          kind: "notice",
          ts: event.ts,
          text: `有 ${event.dropped} 行输出因写入滞后被丢弃`,
          lossy: false,
          level: "warn",
        },
      ];
    case "start":
      return [
        {
          key: `${lineNo}-${event.ts}`,
          kind: "divider",
          ts: event.ts,
          text: `会话开始 · pid ${event.pid} · ${event.cmd}`,
          lossy: false,
          level: "info",
        },
      ];
    default:
      return [];
  }
}

/** 状态事件的中文描述。 */
export function describeStatus(
  state: RunState,
  windowTitle: string | null,
  ports: number[] | null
): string {
  const parts: string[] = [`状态 ${runStateLabel(state)}`];
  if (windowTitle) {
    parts.push(`窗口「${windowTitle}」`);
  }
  if (ports && ports.length > 0) {
    parts.push(`端口 ${ports.join(", ")}`);
  }
  return parts.join(" · ");
}

/** 退出事件的中文描述。 */
export function describeExit(code: number | null, reason: ExitReason): string {
  const codeText = code === null ? "无退出码" : `退出码 ${code}`;
  return `进程已退出 · ${codeText} · ${exitReasonLabel(reason)}`;
}

/** 运行状态的中文标签。 */
export function runStateLabel(state: RunState): string {
  switch (state) {
    case "running":
      return "运行中";
    case "exited":
      return "已退出";
    case "crashed":
      return "已崩溃";
    default:
      return state;
  }
}

/** 退出原因的中文标签。 */
export function exitReasonLabel(reason: ExitReason): string {
  switch (reason) {
    case "normal":
      return "正常退出";
    case "killed":
      return "被停止";
    case "crashed":
      return "异常崩溃";
    default:
      return reason;
  }
}

/** 把日志行追加到缓冲区，并裁掉超出的最旧部分。 */
export function appendRows(existing: LogRow[], incoming: LogRow[]): LogRow[] {
  if (incoming.length === 0) {
    return existing;
  }
  const merged = existing.concat(incoming);
  if (merged.length <= MAX_LOG_ROWS) {
    return merged;
  }
  return merged.slice(merged.length - MAX_LOG_ROWS);
}

/**
 * 是否需要自动滚到底部。
 *
 * 规则：只有用户本来就在底部（容差 32px）时才自动跟随；
 * 用户手动上滚阅读历史时暂停，等他自己回到底部再恢复。
 */
export const AUTO_SCROLL_TOLERANCE = 32;

export function shouldAutoScroll(
  scrollTop: number,
  scrollHeight: number,
  clientHeight: number,
  tolerance: number = AUTO_SCROLL_TOLERANCE
): boolean {
  // 内容比容器还短：没有可滚动的空间，视为「在底部」
  if (scrollHeight <= clientHeight) {
    return true;
  }
  return scrollHeight - scrollTop - clientHeight <= tolerance;
}

/** 轮询间隔（毫秒）：有运行中会话时快，否则放慢。 */
export const POLL_FAST_MS = 1000;
export const POLL_IDLE_MS = 3000;

/** 根据是否有运行中会话决定下一次轮询间隔。 */
export function pollInterval(runningCount: number): number {
  return runningCount > 0 ? POLL_FAST_MS : POLL_IDLE_MS;
}

/**
 * 把事件时间戳格式化成 HH:MM:SS.mmm（本地时区）。
 *
 * 解析失败时原样返回，绝不因为一条脏时间戳让整块日志渲染失败。
 */
export function formatClock(ts: string): string {
  const date = new Date(ts);
  if (Number.isNaN(date.getTime())) {
    return ts;
  }
  const pad = (value: number, width: number): string => String(value).padStart(width, "0");
  return `${pad(date.getHours(), 2)}:${pad(date.getMinutes(), 2)}:${pad(
    date.getSeconds(),
    2
  )}.${pad(date.getMilliseconds(), 3)}`;
}

/** 把 RFC3339 时间戳格式化成 `YYYY-MM-DD HH:mm:ss`（本地时区）。 */
export function formatDateTime(ts: string): string {
  const date = new Date(ts);
  if (Number.isNaN(date.getTime())) {
    return ts;
  }
  const pad = (value: number): string => String(value).padStart(2, "0");
  return `${date.getFullYear()}-${pad(date.getMonth() + 1)}-${pad(date.getDate())} ${pad(
    date.getHours()
  )}:${pad(date.getMinutes())}:${pad(date.getSeconds())}`;
}

/** 判断错误是否为「会话已不在管理器里」（重启启动器后的历史会话）。 */
export function isSessionGone(error: CommandError): boolean {
  return error.kind === "not_found";
}

/** 把 command + args 渲染成可复制的命令行（与 Rust 侧 render_command 同规则）。 */
export function renderCommand(command: string, args: string[] | undefined): string {
  const parts = [command, ...(args ?? [])];
  return parts
    .map((part) => (part.includes(" ") || part.includes('"') ? `"${part.replace(/"/g, '\\"')}"` : part))
    .join(" ");
}

/** 应用名首字（用于占位头像块）：取第一个非空白字符。 */
export function initialOf(name: string): string {
  const trimmed = name.trim();
  if (trimmed.length === 0) {
    return "?";
  }
  // 用 Array.from 而不是 [0]：避免把 emoji 等代理对切成半个字符
  return Array.from(trimmed)[0]!.toUpperCase();
}
