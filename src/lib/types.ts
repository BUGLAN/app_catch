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
  hooks?: AppHooks;
  /** 启动方式；缺省（不出现）即 direct。 */
  launch_mode?: LaunchMode;
}

/**
 * 启动方式：
 * - `direct`：app_catch 直接运行，捕获输出；macOS 上隐私权限记在 app_catch 名下；
 * - `open`：仅 macOS，交给系统打开 .app（等同点 Dock 图标），权限归应用自己，不捕获输出。
 */
export type LaunchMode = "direct" | "open";

/** 「选择应用程序」后后端识别出的信息。 */
export interface ApplicationInfo {
  /** 建议写进启动命令的路径（.app 包本身或可执行文件）。 */
  command: string;
  /** 建议的应用名称。 */
  name: string;
  /** 是否 macOS .app 包。 */
  is_bundle: boolean;
  /** .app 包里真正的可执行文件。 */
  executable: string | null;
}

/** 启动 hook：before 在主进程启动前执行（失败中止启动），after 在启动后后台执行。 */
export interface AppHooks {
  before?: string[];
  after?: string[];
  timeout_secs?: number | null;
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
  hooks?: AppHooks;
  launch_mode: LaunchMode;
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

/** 启动期恢复报告。 */
export interface BootReport {
  home: string;
  recovered: SessionSnapshot[];
  error: string | null;
}

/** 一条启动候选（与 launcher-core::project::CommandSuggestion 同形）。 */
export interface CommandSuggestion {
  /** 短标签，例如 `dev`、`main.py`。 */
  label: string;
  command: string;
  args: string[];
  /** 出处，例如 `package.json scripts.dev`。 */
  source: string;
}

/** 工作目录的项目识别结果。 */
export interface ProjectDetection {
  /** 项目类型，例如 `node:pnpm` / `python` / `rust`，按优先级排列。 */
  kinds: string[];
  /** 清单里声明的项目名（没有则为目录名）。 */
  name: string | null;
  /** 启动候选，第一条最推荐。 */
  suggestions: CommandSuggestion[];
}
