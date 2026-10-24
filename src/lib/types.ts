/**
 * 命令层 API 的 TypeScript 类型镜像。
 *
 * 与 `src-tauri/src/dto.rs` 一一对应：字段名保持 snake_case，
 * 与 Rust 侧序列化结果直接对齐，避免在中间做一次容易漂移的字段重命名。
 * 枚举值（RunState / ExitReason / Encoding）是已经稳定的 wire 契约。
 */

/** 会话运行状态。 */
export type RunState = "running" | "exited" | "crashed";

/** 退出原因。 */
export type ExitReason = "normal" | "killed" | "crashed";

/** 输出行原始编码标记；`lossy` 表示原始字节已不可还原。 */
export type OutputEncoding = "utf-8" | "gbk" | "lossy";

/** 输出流类型。 */
export type StreamKind = "stdout" | "stderr";

/** 应用配置，与 launcher-core 的 AppConfig 同形。 */
export interface AppConfig {
  id: string;
  name: string;
  command: string;
  args?: string[];
  cwd?: string;
  env?: Record<string, string>;
  icon?: string | null;
  tags?: string[];
}

/** 新建/编辑表单提交的载荷（cwd/icon 用空串表示未设置）。 */
export interface AppConfigInput {
  id: string;
  name: string;
  command: string;
  args: string[];
  cwd: string | null;
  env: Record<string, string>;
  icon: string | null;
  tags: string[];
}

/** 退出信息。 */
export interface ExitInfo {
  code: number | null;
  reason: ExitReason;
}

/** 状态采集结果。 */
export interface StatusSnapshot {
  window_title: string | null;
  ports: number[];
}

/** 会话快照。 */
export interface SessionSnapshot {
  session_id: string;
  app_id: string;
  pid: number;
  state: RunState;
  started_at: string;
  last_exit: ExitInfo | null;
  latest_status: StatusSnapshot | null;
}

/** 会话事件（与 launcher-core 的 SessionEvent 判别联合一致）。 */
export type SessionEvent =
  | {
      event: "start";
      v: number;
      ts: string;
      session_id: string;
      app_id: string;
      cmd: string;
      cwd: string;
      argv: string[];
      pid: number;
      ppid: number;
      start_time?: number;
    }
  | {
      event: "output";
      v: number;
      ts: string;
      stream: StreamKind;
      line: string;
      encoding?: OutputEncoding;
    }
  | {
      event: "status";
      v: number;
      ts: string;
      state: RunState;
      window_title?: string | null;
      ports?: number[] | null;
    }
  | {
      event: "exit";
      v: number;
      ts: string;
      code?: number | null;
      reason: ExitReason;
    }
  | { event: "overflow"; v: number; ts: string; dropped: number };

/** 一次分页读取的结果。 */
export interface SessionEventsPage {
  events: SessionEvent[];
  next_line: number;
  has_more: boolean;
  skipped_lines: number;
}

/** 命令层错误（前端统一形状）。 */
export interface CommandError {
  kind: string;
  message: string;
}

/** 日志写入配置。 */
export interface WriterConfig {
  max_file_bytes: number;
  max_shards: number;
  flush_every: number;
  flush_interval_ms: number;
}

/** 捕获配置。 */
export interface CaptureConfig {
  channel_capacity: number;
  max_line_bytes: number;
  status_interval_ms: number | null;
  max_status_events: number | null;
  writer: WriterConfig;
}

/** 启动期恢复报告。 */
export interface BootReport {
  home: string;
  recovered: SessionSnapshot[];
  error: string | null;
}
