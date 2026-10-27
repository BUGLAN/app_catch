/**
 * localStorage 读写：所有"界面偏好"共用这一层。
 *
 * 三个约束：
 * - **读写都可能抛异常**：隐私模式、配额满、被策略禁用时 localStorage 会直接抛错，
 *   界面不能因为存不下一个宽度就白屏；
 * - **值必须校验**：用户手改过 localStorage 是常事，读到 NaN / 负数要退回默认值；
 * - **键名集中在这里**：散落的字符串键最容易在两处写法不一致（一个 `theme`、一个 `app_catch.theme`）。
 */

/** localStorage 键名。改这里等于清空所有用户的偏好，慎改。 */
export const STORAGE_KEYS = {
  theme: "app_catch.theme",
  rail: "app_catch.sidebar",
  updater: "app_catch.updater",
} as const;

/* -------------------------------------------------------------------------- */
/* 更新偏好                                                                     */
/* -------------------------------------------------------------------------- */

/** 自动更新偏好（目前只有一项：启动时是否静默检查）。 */
export interface UpdatePrefs {
  autoCheck: boolean;
}

/** 默认开启：这是桌面应用的常规预期，关闭是用户的主动选择。 */
export const DEFAULT_UPDATE_PREFS: UpdatePrefs = { autoCheck: true };

/**
 * 读更新偏好。
 *
 * 与其它偏好同样的防御：值损坏 / 手改过 / 旧版本没写过的键，
 * 一律退回默认值，绝不让「读偏好」这件事抛错。
 */
export function readUpdatePrefs(): UpdatePrefs {
  const raw = readJson<Partial<UpdatePrefs>>(STORAGE_KEYS.updater);
  if (raw === null || typeof raw.autoCheck !== "boolean") {
    return { ...DEFAULT_UPDATE_PREFS };
  }
  return { autoCheck: raw.autoCheck };
}

/** 写更新偏好；失败静默（只影响下次启动是否自动检查）。 */
export function writeUpdatePrefs(prefs: UpdatePrefs): void {
  writeJson(STORAGE_KEYS.updater, prefs);
}

/** 读一个 JSON 值；任何异常/缺失都返回 null。 */
export function readJson<T>(key: string): T | null {
  try {
    const raw = window.localStorage.getItem(key);
    if (!raw) {
      return null;
    }
    return JSON.parse(raw) as T;
  } catch {
    return null;
  }
}

/** 写一个 JSON 值；失败时静默（不影响当前会话中的状态）。 */
export function writeJson(key: string, value: unknown): void {
  try {
    window.localStorage.setItem(key, JSON.stringify(value));
  } catch {
    // 存不下就只影响"下次启动能否恢复"，当前这次交互必须照常可用
  }
}

/** 读一个字符串值；失败返回 null。 */
export function readString(key: string): string | null {
  try {
    return window.localStorage.getItem(key);
  } catch {
    return null;
  }
}

/** 写一个字符串值；失败静默。 */
export function writeString(key: string, value: string): void {
  try {
    window.localStorage.setItem(key, value);
  } catch {
    // 同上
  }
}
