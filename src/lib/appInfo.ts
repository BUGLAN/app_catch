/**
 * 应用版本号的唯一出口。
 *
 * 版本号的「事实来源」是打包配置：
 * - 安装包版本 = `src-tauri/tauri.conf.json` 的 `version`（Tauri 打包器用它命名产物）；
 * - `package.json` 的 `version` 必须与它一致（`versionContract.test.ts` 守护），
 *   并在编译期由 vite 注入为 `__APP_VERSION__`，供浏览器预览兜底。
 *
 * 界面任何地方要显示版本，都从这里拿，不允许再写死字符串。
 */
import { getVersion } from "@tauri-apps/api/app";

import { isHosted } from "./api";

/**
 * 解析当前应用版本。
 *
 * 宿主内走 `getVersion()`：它读的是 `tauri.conf.json` 的 version，
 * 与安装包上的版本号同源，绝不会与「关于」页显示的漂移。
 * 非宿主（浏览器预览 / 单测）退回编译期注入的 `__APP_VERSION__`。
 */
export async function resolveAppVersion(): Promise<string> {
  if (isHosted()) {
    try {
      return await getVersion();
    } catch {
      // 理论上不可达：宿主标记已存在但命令失败（例如极端早期调用）。
      // 落到编译期版本，宁可显示旧值也不让「关于」页白屏。
    }
  }
  return __APP_VERSION__;
}
