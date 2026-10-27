/**
 * 更新流程的界面状态：updaterModel（纯状态机）+ updater（插件客户端）的接线层。
 *
 * 职责边界：
 * - 状态迁移规则全部在 updaterModel，这里只做「调插件 → 把结果喂给状态机」；
 * - 「自动检查」偏好走 preferences（localStorage），与主题/侧栏同一套读写约定；
 * - 挂载时**不做**任何事：启动时的静默检查由 App 统一做（只有一份定时逻辑），
 *   设置页只消费 App 播种进来的版本号（seedAvailable）。
 */
import { useCallback, useState } from "react";

import {
  readUpdatePrefs,
  writeUpdatePrefs,
} from "../lib/preferences";
import {
  checkForUpdate,
  describeUpdaterError,
  relaunchApp,
  unsupportedReason,
  updaterSupported,
  type PendingUpdate,
} from "../lib/updater";
import {
  INITIAL_UPDATE_STATE,
  beginCheck,
  beginDownload,
  downloadProgress,
  failUpdate,
  finishCheck,
  finishDownload,
  seedAvailable,
  type UpdateState,
} from "../lib/updaterModel";

/** useUpdater 的返回值（组件只用这些，不直接碰状态机函数）。 */
export interface UseUpdaterResult {
  /** 更新流程状态（渲染唯一依据）。 */
  state: UpdateState;
  /** 当前环境是否允许更新（宿主内且非开发模式）。 */
  supported: boolean;
  /** 不允许时的原因说明。 */
  unsupportedMessage: string | null;
  /** 启动时是否自动检查（持久化偏好）。 */
  autoCheck: boolean;
  /** 修改自动检查偏好（立即落盘）。 */
  setAutoCheck: (next: boolean) => void;
  /** 手动「检查更新」。 */
  check: () => Promise<void>;
  /** 「下载并安装」；安装完成后进入 ready（Windows 上安装即退出，到不了 ready）。 */
  downloadAndInstall: () => Promise<void>;
  /** ready 之后「重启进入新版本」。 */
  relaunch: () => Promise<void>;
  /** 把 App 启动静默检查发现的新版本直接展示出来。 */
  seedAvailable: (version: string) => void;
}

export function useUpdater(): UseUpdaterResult {
  const [state, setState] = useState<UpdateState>(INITIAL_UPDATE_STATE);
  const [autoCheck, setAutoCheckState] = useState(() => readUpdatePrefs().autoCheck);

  // check 返回的 Update 对象必须活到安装完成，这里持有；组件不需要知道它的类型
  const [pending, setPending] = useState<PendingUpdate | null>(null);

  const supported = updaterSupported();

  const check = useCallback(async () => {
    if (!supported) {
      setState((current) => failUpdate(current, unsupportedReason()));
      return;
    }
    setState((current) => beginCheck(current));
    try {
      const found = await checkForUpdate();
      setPending(found);
      setState((current) => finishCheck(current, found?.info.version ?? null));
    } catch (raw) {
      setState((current) => failUpdate(current, describeUpdaterError(raw)));
    }
  }, [supported]);

  const downloadAndInstall = useCallback(async () => {
    // 设置页展示的「发现新版本」可能来自 App 启动时的静默检查（句柄不在手），
    // 此时先补一次检查拿到句柄——最新清单就在眼前，代价可忽略。
    let handle = pending;
    if (!handle) {
      try {
        handle = await checkForUpdate();
        setPending(handle);
      } catch (raw) {
        setState((current) => failUpdate(current, describeUpdaterError(raw)));
        return;
      }
      if (!handle) {
        setState((current) => failUpdate(current, "没有可安装的更新，请先检查更新。"));
        return;
      }
    }
    setState((current) => beginDownload(current));
    try {
      await handle.downloadAndInstall((event) => {
        setState((current) => {
          switch (event.kind) {
            case "started":
              return downloadProgress(current, 0, event.totalBytes);
            case "chunk":
              return downloadProgress(current, event.bytes, null);
            case "finished":
              return current;
          }
        });
      });
      setState((current) => finishDownload(current));
    } catch (raw) {
      setState((current) => failUpdate(current, describeUpdaterError(raw)));
    }
  }, [pending]);

  const relaunch = useCallback(async () => {
    try {
      await relaunchApp();
    } catch (raw) {
      setState((current) => failUpdate(current, describeUpdaterError(raw)));
    }
  }, []);

  const setAutoCheck = useCallback((next: boolean) => {
    setAutoCheckState(next);
    writeUpdatePrefs({ autoCheck: next });
  }, []);

  const seed = useCallback((version: string) => {
    setState((current) => seedAvailable(current, version));
  }, []);

  return {
    state,
    supported,
    unsupportedMessage: supported ? null : unsupportedReason(),
    autoCheck,
    setAutoCheck,
    check,
    downloadAndInstall,
    relaunch,
    seedAvailable: seed,
  };
}
