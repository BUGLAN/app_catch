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
import { listen } from "@tauri-apps/api/event";

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

/** 最大化状态变化事件名：与 `src-tauri/src/window.rs` 的 MAXIMIZED_EVENT 一致。 */
export const MAXIMIZED_EVENT = "window:maximized-changed";

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

/**
 * 当前是否运行在 Tauri 宿主里。
 *
 * 只看 `__TAURI_INTERNALS__`：这是 Tauri 2 注入的稳定标记，
 * 比 UA 或"试着 invoke 一次"都可靠，而且是同步的（首帧就要用来决定按钮可否点）。
 */
export function isHosted(): boolean {
  return typeof window !== "undefined" && "__TAURI_INTERNALS__" in window;
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

  /**
   * 取应用图标，返回 data URL；`null` 表示没有图标（用内置默认图）。
   *
   * 走 data URL 而不是 `asset:` 协议：省掉 assetProtocol scope 与 capability 配置，
   * 少一个只在运行时才暴露的「图裂」故障面（详见 docs/ICONS.md）。
   *
   * `force` 为真时忽略命令层的磁盘缓存强制重抓——只用在「会话刚启动」，
   * 那一刻窗口才出现，窗口档位第一次可能拿到真图标。
   */
  getAppIcon: (appId: string, force = false) =>
    call<string | null>("get_app_icon", { appId, force }),
  /** 内置默认图标（PNG data URL），永远可用。 */
  defaultIcon: () => call<string>("default_icon"),
};

/**
 * 窗口控制：自绘标题栏用（与 `src-tauri/src/window.rs` 一一对应）。
 *
 * 单独成一组（而不是塞进 `api`）：它们只在 Tauri 宿主里存在，
 * 浏览器预览时调用会得到 `not_hosted`，界面据此把按钮置灰而不是弹错误。
 */
export const windowApi = {
  minimize: () => call<void>("window_minimize"),
  maximize: () => call<void>("window_maximize"),
  unmaximize: () => call<void>("window_unmaximize"),
  /** 切换最大化，返回切换后的状态（true = 已最大化）。 */
  toggleMaximize: () => call<boolean>("window_toggle_maximize"),
  isMaximized: () => call<boolean>("window_is_maximized"),
  close: () => call<void>("window_close"),
};

/**
 * 监听最大化状态变化，返回取消监听的函数。
 *
 * 非宿主环境下直接返回空操作：`listen` 在浏览器里会抛错，
 * 而"窗口没有标题栏按钮"本来就不需要这个事件。
 */
export async function onMaximizedChange(
  handler: (maximized: boolean) => void
): Promise<() => void> {
  if (!isHosted()) {
    return () => {};
  }
  try {
    const unlisten = await listen<boolean>(MAXIMIZED_EVENT, (event) => {
      handler(event.payload === true);
    });
    return unlisten;
  } catch {
    return () => {};
  }
}
