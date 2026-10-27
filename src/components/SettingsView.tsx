/**
 * 设置页：当前只有「关于 / 版本 / 更新」一个区块。
 *
 * 后续往这里加设置项时保持两条约定：
 * - 每个设置项要么直接读写 preferences（localStorage），要么走命令层，
 *   不在组件里发明第三种持久化；
 * - 状态展示优先复用 panel / kv / checkbox 基础件，新样式集中到 settings.css。
 */
import { useEffect, useState } from "react";

import { resolveAppVersion } from "../lib/appInfo";
import { UPDATER_ENDPOINT } from "../lib/updater";
import { describeUpdateState, progressPercent } from "../lib/updaterModel";
import { useUpdater } from "../hooks/useUpdater";
import { IconAlert, IconCheck, IconInfo, IconRestart } from "./ui/Icons";
import { KeyValueList } from "./ui/Primitives";

interface Props {
  /** App 启动静默检查发现的新版本：进设置页直接展示，不必再点一次检查。 */
  availableVersion?: string | null;
  /** 设置页消费掉播种版本后回调（App 清掉自己的副本，避免重复播种）。 */
  onConsumeAvailableVersion?: () => void;
}

export function SettingsView({ availableVersion, onConsumeAvailableVersion }: Props) {
  const [version, setVersion] = useState<string | null>(null);
  const updater = useUpdater();
  const { state, supported, unsupportedMessage, autoCheck, setAutoCheck } = updater;

  // 版本号异步取：宿主内走 getVersion()（tauri.conf.json），预览走编译期注入值
  useEffect(() => {
    let cancelled = false;
    (async () => {
      const resolved = await resolveAppVersion();
      if (!cancelled) {
        setVersion(resolved);
      }
    })();
    return () => {
      cancelled = true;
    };
  }, []);

  // 消费 App 播种的新版本（只在 idle 时生效，见 seedAvailable）
  useEffect(() => {
    if (availableVersion) {
      updater.seedAvailable(availableVersion);
      onConsumeAvailableVersion?.();
    }
    // seedAvailable / onConsume 均为稳定引用；播种只跟 availableVersion 走
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [availableVersion]);

  const busy = state.phase === "checking" || state.phase === "downloading";
  const percent = progressPercent(state);
  const versionText = version === null ? "读取中…" : `v${version}`;

  return (
    <div className="detail-stack">
      <section className="panel settings-panel" aria-labelledby="settings-about-title">
        <div className="panel-head">
          <div className="panel-head-main">
            <div className="panel-heading">
              <h2 className="panel-title" id="settings-about-title">
                关于与更新
              </h2>
              <p className="panel-sub">
                版本号取自打包配置（tauri.conf.json），更新包由 GitHub Releases 分发
              </p>
            </div>
          </div>
          <div className="panel-actions">
            <button
              type="button"
              className="btn"
              onClick={() => void updater.check()}
              disabled={busy || !supported}
            >
              检查更新
            </button>
            {state.phase === "available" ? (
              <button
                type="button"
                className="btn btn-primary"
                onClick={() => void updater.downloadAndInstall()}
                disabled={busy}
              >
                下载并安装 v{state.nextVersion}
              </button>
            ) : null}
            {state.phase === "ready" ? (
              <button
                type="button"
                className="btn btn-primary"
                onClick={() => void updater.relaunch()}
              >
                <IconRestart size={14} />
                重启进入新版本
              </button>
            ) : null}
          </div>
        </div>

        <KeyValueList
          items={[
            {
              label: "当前版本",
              value: <span className="mono">{versionText}</span>,
            },
            { label: "更新通道", value: "stable（GitHub Releases）" },
            {
              label: "更新清单",
              value: (
                <span className="mono small update-endpoint" title={UPDATER_ENDPOINT}>
                  {UPDATER_ENDPOINT}
                </span>
              ),
            },
          ]}
        />

        <div className="update-block" data-phase={state.phase}>
          <p className="update-status" role="status" aria-live="polite">
            <span className="update-status-icon">
              {state.phase === "error" ? (
                <IconAlert size={14} />
              ) : state.phase === "up-to-date" || state.phase === "ready" ? (
                <IconCheck size={14} />
              ) : (
                <IconInfo size={14} />
              )}
            </span>
            <span>{describeUpdateState(state)}</span>
          </p>

          {state.phase === "downloading" ? (
            <div
              className="update-progress"
              role="progressbar"
              aria-label="更新下载进度"
              aria-valuemin={0}
              aria-valuemax={100}
              aria-valuenow={percent ?? undefined}
            >
              <div
                className="update-progress-fill"
                data-indeterminate={percent === null ? "true" : "false"}
                style={percent === null ? undefined : { width: `${percent}%` }}
              />
            </div>
          ) : null}

          {supported ? null : (
            <p className="update-note muted">
              {unsupportedMessage ?? "当前环境不支持自动更新。"}
            </p>
          )}

          {state.phase === "ready" ? (
            <p className="update-note muted">
              Windows 安装器会在安装时自动退出应用；重新打开即进入新版本。
            </p>
          ) : null}

          <label className="checkbox update-auto">
            <input
              type="checkbox"
              checked={autoCheck}
              onChange={(event) => setAutoCheck(event.target.checked)}
              disabled={!supported}
            />
            <span>启动时自动检查更新（发现新版本时提示，不自动安装）</span>
          </label>
        </div>
      </section>
    </div>
  );
}
