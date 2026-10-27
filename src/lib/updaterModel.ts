/**
 * 更新流程的纯状态机：所有状态迁移都在这里，组件只负责渲染。
 *
 * 抽成纯函数的原因与 logModel 相同：
 * - 状态迁移规则可单测（不依赖 Tauri IPC，浏览器里也能跑）；
 * - 「检查中 / 已是最新 / 发现新版本 / 下载中 / 待重启 / 失败」六态
 *   的合法迁移只有几条，散在组件里一定会写出非法组合。
 */

/** 更新流程的阶段。 */
export type UpdatePhase =
  | "idle" // 还没做过任何事
  | "checking" // 正在查询 latest.json
  | "up-to-date" // 查询完成：没有新版本
  | "available" // 发现新版本，等待用户点「下载并安装」
  | "downloading" // 下载中（可能有进度，也可能拿不到总长）
  | "ready" // 已安装，等待重启进新版本
  | "error"; // 任一步失败（原因在 error 里）

/** 更新流程的完整状态。 */
export interface UpdateState {
  phase: UpdatePhase;
  /** 新版本号；available / downloading / ready 阶段有值。 */
  nextVersion: string | null;
  /** 失败原因（中文，直接可展示）；error 阶段有值。 */
  error: string | null;
  /** 已下载字节数；downloading 阶段递增。 */
  downloadedBytes: number;
  /** 总字节数；服务器未返回 content-length 时为 null（进度条退化为不确定态）。 */
  totalBytes: number | null;
}

/** 初始状态：从未检查过。 */
export const INITIAL_UPDATE_STATE: UpdateState = {
  phase: "idle",
  nextVersion: null,
  error: null,
  downloadedBytes: 0,
  totalBytes: null,
};

/** 进入「检查中」；清掉上一次的结果，避免新旧状态叠着显示。 */
export function beginCheck(_state: UpdateState): UpdateState {
  // 参数保留：状态机 API 形状统一（每个迁移都吃旧状态），这里确实不需要旧值
  return { ...INITIAL_UPDATE_STATE, phase: "checking" };
}

/** 检查结束：有新版本带着版本号进 available，没有则 up-to-date。 */
export function finishCheck(state: UpdateState, nextVersion: string | null): UpdateState {
  if (state.phase !== "checking" && state.phase !== "idle") {
    // 只允许从 checking（或后台静默检查的 idle）落结果，防止乱序回调覆盖后续阶段
    return state;
  }
  if (nextVersion === null || nextVersion.length === 0) {
    return { ...state, phase: "up-to-date", nextVersion: null };
  }
  return { ...state, phase: "available", nextVersion, error: null };
}

/** 任一步失败：带上可直接展示的中文原因。 */
export function failUpdate(state: UpdateState, message: string): UpdateState {
  return {
    ...state,
    phase: "error",
    error: message.length > 0 ? message : "更新失败（未知原因）",
  };
}

/** 开始下载：回到零进度，但保留版本号。 */
export function beginDownload(state: UpdateState): UpdateState {
  return {
    ...state,
    phase: "downloading",
    error: null,
    downloadedBytes: 0,
    totalBytes: null,
  };
}

/** 下载进度事件：累加已下载字节，并记录总长（只信第一次拿到的总长）。 */
export function downloadProgress(
  state: UpdateState,
  chunkBytes: number,
  totalBytes: number | null
): UpdateState {
  if (state.phase !== "downloading") {
    return state;
  }
  const total = state.totalBytes ?? totalBytes;
  const downloaded = state.downloadedBytes + Math.max(0, chunkBytes);
  return {
    ...state,
    downloadedBytes: downloaded,
    // 总长只缩不涨：服务器偶尔会在中途改口，以先拿到为准
    totalBytes: total !== null && downloaded > total ? downloaded : total,
  };
}

/** 下载并安装完成：等待用户确认重启（Windows 上安装即退出，走不到这里）。 */
export function finishDownload(state: UpdateState): UpdateState {
  if (state.phase !== "downloading") {
    return state;
  }
  return { ...state, phase: "ready", error: null };
}

/** 从外部播种一个「已发现的新版本」（App 启动静默检查 → 设置页直接展示）。 */
export function seedAvailable(state: UpdateState, version: string): UpdateState {
  if (state.phase !== "idle") {
    return state;
  }
  return { ...INITIAL_UPDATE_STATE, phase: "available", nextVersion: version };
}

/**
 * 进度百分比（0-100）；拿不到总长时返回 null，调用方应渲染不确定态进度条。
 */
export function progressPercent(state: UpdateState): number | null {
  if (state.totalBytes === null || state.totalBytes <= 0) {
    return null;
  }
  return Math.min(100, Math.round((state.downloadedBytes / state.totalBytes) * 100));
}

/** 字节数的中文可读形式（进度条旁的数字）。 */
export function formatBytes(bytes: number): string {
  if (!Number.isFinite(bytes) || bytes < 0) {
    return "0 B";
  }
  if (bytes < 1024) {
    return `${bytes} B`;
  }
  const units = ["KB", "MB", "GB"] as const;
  let value = bytes;
  let unit = "B";
  for (const next of units) {
    if (value < 1024) {
      break;
    }
    value /= 1024;
    unit = next;
  }
  return `${value >= 10 ? Math.round(value) : Math.round(value * 10) / 10} ${unit}`;
}

/** 每个阶段的一句话描述：status 区直接渲染，aria-live 播报也用它。 */
export function describeUpdateState(state: UpdateState): string {
  switch (state.phase) {
    case "idle":
      return "尚未检查更新。";
    case "checking":
      return "正在检查更新……";
    case "up-to-date":
      return "已是最新版本。";
    case "available":
      return state.nextVersion ? `发现新版本 v${state.nextVersion}。` : "发现新版本。";
    case "downloading": {
      const done = formatBytes(state.downloadedBytes);
      const total = state.totalBytes === null ? null : formatBytes(state.totalBytes);
      const percent = progressPercent(state);
      const percentText = percent === null ? "" : `（${percent}%）`;
      return total === null
        ? `正在下载更新……已下载 ${done}`
        : `正在下载更新……${done} / ${total}${percentText}`;
    }
    case "ready":
      return "更新已安装，重启后生效。";
    case "error":
      return state.error ?? "更新失败（未知原因）。";
  }
}
