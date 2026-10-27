/**
 * tauri-plugin-updater 的前端薄封装。
 *
 * 为什么再包一层而不是组件里直接 import 插件：
 * - 「什么时候允许更新」的判定只有一处：必须在 Tauri 宿主内 **且** 不是开发模式。
 *   `pnpm tauri dev` 跑的是未打包的二进制，没有更新可言，检查只会报错；
 * - 插件的 `Update` 对象（check 的返回值）要跨「检查」和「安装」两个动作存活，
 *   收敛在一个类型里，调用方不用碰插件的原生类型；
 * - 单测只需要 mock 这一个模块，不用 mock 插件本身。
 *
 * 配套配置（都在 src-tauri 侧）：
 * - `tauri.conf.json` 的 `plugins.updater`：GitHub Releases endpoint + minisign 公钥；
 * - `capabilities/default.json` 的 `updater:default` / `process:default` 权限。
 */
import { check, type Update } from "@tauri-apps/plugin-updater";
import { relaunch } from "@tauri-apps/plugin-process";

import { isHosted } from "./api";

/**
 * 更新清单地址。必须与 `src-tauri/tauri.conf.json` 里 `plugins.updater.endpoints[0]`
 * 保持一致（`versionContract.test.ts` 会跨文件核对这条契约）。
 */
export const UPDATER_ENDPOINT =
  "https://github.com/BUGLAN/app_catch/releases/latest/download/latest.json";

/** 当前环境是否允许更新：宿主内 + 非开发构建。 */
export function updaterSupported(): boolean {
  // import.meta.env.DEV 在 tauri dev（vite dev server）下为 true；
  // 打包后的前端它被静态替换成 false，判断零开销。
  return isHosted() && !import.meta.env.DEV;
}

/** 不支持更新时的原因说明（设置页直接展示）。 */
export function unsupportedReason(): string {
  if (!isHosted()) {
    return "浏览器预览模式下不可用：请通过安装版应用检查更新。";
  }
  return "开发模式下已禁用更新检查：请用安装版应用验证更新流程。";
}

/** 检查结果里组件关心的元信息。 */
export interface UpdateInfo {
  version: string;
  /** 更新说明（Release 正文），可能为 null。 */
  body: string | null;
}

/** 一次「检查 + 安装」的完整句柄：check 返回的 Update 对象在这里存活到安装完成。 */
export interface PendingUpdate {
  info: UpdateInfo;
  /** 下载并安装；进度通过回调流出（chunk 为增量字节，total 可能为 null）。 */
  downloadAndInstall(onProgress: (event: DownloadEvent) => void): Promise<void>;
}

/** 下载进度事件（对应插件 Started / Progress / Finished 三种）。 */
export type DownloadEvent =
  | { kind: "started"; totalBytes: number | null }
  | { kind: "chunk"; bytes: number }
  | { kind: "finished" };

/** 检查更新；返回 null 表示没有新版本（或当前已是最新）。 */
export async function checkForUpdate(): Promise<PendingUpdate | null> {
  const update: Update | null = await check();
  if (!update) {
    return null;
  }
  return {
    info: { version: update.version, body: update.body ?? null },
    downloadAndInstall(onProgress) {
      return update.downloadAndInstall((event) => {
        switch (event.event) {
          case "Started":
            onProgress({ kind: "started", totalBytes: event.data.contentLength ?? null });
            break;
          case "Progress":
            onProgress({ kind: "chunk", bytes: event.data.chunkLength });
            break;
          case "Finished":
            onProgress({ kind: "finished" });
            break;
        }
      });
    },
  };
}

/**
 * App 启动时的静默检查。
 *
 * 与手动检查的差别：**任何失败都吞掉**——启动路径上的网络抖动不该弹错误。
 * 返回发现的版本号（没有新版本 / 不可用 / 出错都返回 null），提示交给调用方。
 */
export async function backgroundCheckForUpdate(): Promise<string | null> {
  if (!updaterSupported()) {
    return null;
  }
  try {
    const pending = await checkForUpdate();
    return pending?.info.version ?? null;
  } catch {
    return null;
  }
}

/** 更新装好后重启进新版本。 */
export async function relaunchApp(): Promise<void> {
  await relaunch();
}

/** 把插件抛出的任意错误归一成中文文案（设置页直接展示）。 */
export function describeUpdaterError(raw: unknown): string {
  const message = raw instanceof Error ? raw.message : typeof raw === "string" ? raw : "";
  if (/network|fetch|timeout|dns|ENOTFOUND|ECONNREFUSED/i.test(message)) {
    return "网络请求失败：请检查网络后重试。";
  }
  if (/signature|public key/i.test(message)) {
    return "更新包签名校验失败：请重新下载或联系维护者。";
  }
  return message.length > 0 ? `更新失败：${message}` : "更新失败（未知原因）。";
}
