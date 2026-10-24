/**
 * Tauri 命令客户端：唯一的前端 -> Rust 调用出口。
 *
 * 两点约定：
 * - **错误形状统一**：任何命令失败都会被归一成 {@link CommandError}，
 *   界面层只需读 `kind` 与 `message`，不用再兼容字符串/对象的两种形态；
 * - **非 Tauri 环境降级**：浏览器里跑 `pnpm dev` 时没有 Tauri 宿主，
 *   `invoke` 会拒绝。这里转成可识别的错误，让界面显示「未连接到宿主」
 *   而不是白屏——M0 里已经有过同样的处理。
 */
import { invoke } from "@tauri-apps/api/core";
import type {
  AppConfig,
  AppConfigInput,
  BootReport,
  CaptureConfig,
  CommandError,
  SessionEventsPage,
  SessionSnapshot,
} from "./types";

/** 非 Tauri 环境的固定错误文案。 */
export const NOT_HOSTED_MESSAGE = "未连接到 Tauri 宿主（请用 pnpm tauri dev 启动）";

/** 把任意抛出值归一成 CommandError。 */
export function toCommandError(raw: unknown): CommandError {
  if (typeof raw === "object" && raw !== null) {
    const candidate = raw as Partial<CommandError>;
    if (typeof candidate.message === "string") {
      return {
        kind: typeof candidate.kind === "string" ? candidate.kind : "unknown",
        message: candidate.message,
      };
    }
  }
  if (typeof raw === "string") {
    return { kind: "unknown", message: raw };
  }
  if (raw instanceof Error) {
    return { kind: "unknown", message: raw.message };
  }
  return { kind: "unknown", message: String(raw) };
}

/** 判断一次失败是否源于「不在 Tauri 宿主里」。 */
export function isNotHosted(error: CommandError): boolean {
  return error.kind === "not_hosted";
}

async function call<T>(command: string, args?: Record<string, unknown>): Promise<T> {
  try {
    return await invoke<T>(command, args);
  } catch (raw) {
    const error = toCommandError(raw);
    // Tauri 未注入时 invoke 抛的是 "window.__TAURI_INTERNALS__ is undefined" 一类文案，
    // 归一成专门的 kind，界面据此显示引导而不是报错红条。
    if (/__TAURI_INTERNALS__|not a function|Cannot read propert/i.test(error.message)) {
      throw { kind: "not_hosted", message: NOT_HOSTED_MESSAGE } satisfies CommandError;
    }
    throw error;
  }
}

/** 命令层 API 清单（与 src-tauri/src/commands.rs 一一对应）。 */
export const api = {
  /** 列出全部应用。 */
  listApps: () => call<AppConfig[]>("list_apps"),
  /** 读取单个应用。 */
  getApp: (id: string) => call<AppConfig>("get_app", { id }),
  /** 新建应用。 */
  createApp: (config: AppConfigInput) => call<AppConfig>("create_app", { config }),
  /** 更新应用。 */
  updateApp: (config: AppConfigInput) => call<AppConfig>("update_app", { config }),
  /** 删除应用。 */
  deleteApp: (id: string) => call<void>("delete_app", { id }),

  /** 启动应用，返回新会话快照。 */
  startApp: (id: string) => call<SessionSnapshot>("start_app", { id }),
  /** 停止会话。 */
  stopSession: (sessionId: string) => call<void>("stop_session", { sessionId }),
  /** 重启会话。 */
  restartSession: (sessionId: string, appId: string) =>
    call<SessionSnapshot>("restart_session", { sessionId, appId }),
  /** 列出运行中的会话。 */
  listActiveSessions: () => call<SessionSnapshot[]>("list_active_sessions"),
  /** 查询会话状态。 */
  getSessionStatus: (sessionId: string) =>
    call<SessionSnapshot>("get_session_status", { sessionId }),
  /** 增量读取会话事件。 */
  readSessionEvents: (sessionId: string, fromLine: number, maxLines: number) =>
    call<SessionEventsPage>("read_session_events", { sessionId, fromLine, maxLines }),

  /** 读取捕获配置。 */
  getCaptureConfig: () => call<CaptureConfig>("get_capture_config"),
  /** 写入捕获配置。 */
  setCaptureConfig: (config: CaptureConfig) =>
    call<CaptureConfig>("set_capture_config", { config }),

  /** 启动期恢复残留会话。 */
  recoverSessions: () => call<BootReport>("recover_sessions"),
};
