/**
 * 应用图标：抓取、缓存（前端侧）与加载失败静默降级。
 *
 * 三层缓存，各管一段：
 * 1. **磁盘缓存**（Rust 侧）：\`<home>/icons/<app_id>.png\`，跨进程复用；
 * 2. **兜底图缓存**（这里）：\`default_icon\` 只取一次，之后所有无图标的项共用它；
 * 3. **会话内缓存**（这里）：同一 app_id 在一次界面生命周期内只请求一次，
 *    列表滚动、切换选中都不会重复触发 IPC。
 *
 * 关键约束：**图标加载失败必须静默降级**。图标是装饰性信息，
 * 任何失败都应该退回默认图，绝不能让一次 IPC 失败弹 toast 或者让列表渲染中断。
 */
import { api } from "./api";

/** 已确定的图标结果：PNG data URL，或 null 表示「确认没有图标」。 */
export type IconState = string | null;

/** 界面看到的图标状态：undefined 表示还在加载中。 */
export type IconView = IconState | undefined;

/** app_id -> data URL（null 表示确认「没有图标」，也要缓存，避免反复请求）。 */
const resolved = new Map<string, IconState>();

/** 正在请求中的 app_id，用于合并并发请求。 */
const pending = new Map<string, Promise<IconState>>();

/** 内置默认图的 data URL 缓存（进程内只取一次）。 */
let defaultIconPromise: Promise<string> | null = null;

/**
 * 取内置默认图的 data URL。
 *
 * 命令失败（例如非 Tauri 环境）时返回空串，调用方渲染纯色占位块即可——
 * 这是最后一道兜底，保证界面永远不会因为图标而崩。
 */
export function loadDefaultIcon(): Promise<string> {
  if (!defaultIconPromise) {
    defaultIconPromise = api.defaultIcon().catch(() => "");
  }
  return defaultIconPromise;
}

/**
 * 取某个应用的图标 data URL（已带会话内缓存）。
 *
 * `force` 为真时同时绕过内存缓存与命令层的磁盘缓存，重新抓一次。
 *
 * 返回 \`null\` 表示「这个应用没有图标」，调用方应显示默认图。
 * 请求失败同样返回 \`null\`——错误对用户没有意义，静默降级。
 */
export function loadIcon(appId: string, force = false): Promise<IconState> {
  // force = 「这次别信内存缓存」：用于会话刚启动、窗口才出现的那一刻
  if (!force) {
    const cached = resolved.get(appId);
    if (cached !== undefined) {
      return Promise.resolve(cached);
    }
  }
  const inFlight = pending.get(appId);
  if (inFlight && !force) {
    return inFlight;
  }

  const request = api
    .getAppIcon(appId, force)
    .then((url) => {
      const value: IconState = url && url.length > 0 ? url : null;
      resolved.set(appId, value);
      return value;
    })
    .catch(() => {
      // 失败不写入 resolved：下次打开详情页可以重试
      return null as IconState;
    })
    .finally(() => {
      pending.delete(appId);
    });

  pending.set(appId, request);
  return request;
}

/**
 * 丢弃某个应用的图标缓存，让下次加载重新抓取。
 *
 * 用在「用户点了启动」之后：会话起来后窗口才存在，
 * 这时再抓一次才可能拿到基线说的「运行时真图标」。
 */
export function invalidateIcon(appId: string): void {
  resolved.delete(appId);
  pending.delete(appId);
}

/** 清空全部缓存（测试与「重启后刷新」用）。 */
export function resetIconCache(): void {
  resolved.clear();
  pending.clear();
  defaultIconPromise = null;
}

/** 直接写入一条已知结果（测试用，避免依赖真实 IPC）。 */
export function seedIcon(appId: string, value: IconState): void {
  resolved.set(appId, value);
}