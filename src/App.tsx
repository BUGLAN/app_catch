/**
 * app_catch 主界面：左侧应用列表 + 右侧详情/会话两栏。
 *
 * 状态管理刻意只用一个 `useState` 承载「数据 + 选中态」，
 * 轮询逻辑集中在 {@link usePolling} 里，组件本身只负责渲染。
 */
import { useCallback, useEffect, useMemo, useRef, useState } from "react";

import { api, isNotHosted, toCommandError } from "./lib/api";
import { invalidateIcon } from "./lib/iconModel";
import type {
  AppConfig,
  AppConfigInput,
  BootReport,
  CommandError,
  SessionSnapshot,
} from "./lib/types";
import { AppDetail } from "./components/AppDetail";
import { AppForm } from "./components/AppForm";
import { AppList } from "./components/AppList";
import { SessionDetail } from "./components/SessionDetail";
import { Toast, type ToastMessage } from "./components/Toast";

/**
 * 会话启动后等待多久再抓「运行时图标」（毫秒）。
 *
 * 窗口创建通常在 1 秒内完成（PySide/Electron 首次启动可能更久）。
 * 取 1.5 秒是折中：太早窗口还没出来，太晚用户已经看过列表了。
 * 抓不到也不影响任何功能——前端会一直显示默认图。
 */
const RUNTIME_ICON_DELAY_MS = 1500;

/** 表单的编辑目标：新建（null）或已有应用。 */
type FormTarget = AppConfig | null;

/** 一次删除确认的上下文。 */
interface DeleteTarget {
  app: AppConfig;
}

export default function App() {
  const [apps, setApps] = useState<AppConfig[]>([]);
  const [sessions, setSessions] = useState<SessionSnapshot[]>([]);
  const [selectedId, setSelectedId] = useState<string | null>(null);
  const [selectedSession, setSelectedSession] = useState<string | null>(null);

  const [formTarget, setFormTarget] = useState<FormTarget | undefined>(undefined);
  const [deleteTarget, setDeleteTarget] = useState<DeleteTarget | null>(null);
  const [busy, setBusy] = useState(false);
  const [loading, setLoading] = useState(true);
  const [boot, setBoot] = useState<BootReport | null>(null);
  const [toasts, setToasts] = useState<ToastMessage[]>([]);
  /** 图标刷新触发器：会话启动后 +1，让列表与详情重新抓「运行时图标」。 */
  const [iconRefresh, setIconRefresh] = useState(0);

  const toastSeq = useRef(0);

  const pushToast = useCallback((error: CommandError | string) => {
    const message = typeof error === "string" ? error : error.message;
    toastSeq.current += 1;
    const id = toastSeq.current;
    setToasts((prev) => prev.concat({ id, message }));
  }, []);

  const dismissToast = useCallback((id: number) => {
    setToasts((prev) => prev.filter((toast) => toast.id !== id));
  }, []);

  /** 统一包装一次可能失败的操作：捕获错误 -> toast -> 返回是否成功。 */
  const run = useCallback(
    async <T,>(action: () => Promise<T>): Promise<T | undefined> => {
      try {
        return await action();
      } catch (raw) {
        const error = toCommandError(raw);
        pushToast(isNotHosted(error) ? "请在 Tauri 窗口中使用（pnpm tauri dev）" : error.message);
        return undefined;
      }
    },
    [pushToast]
  );

  /** 拉取应用列表与活跃会话。 */
  const refresh = useCallback(async () => {
    const list = await run(() => api.listApps());
    if (list) {
      setApps(list);
      setSelectedId((current) => {
        if (current && list.some((app) => app.id === current)) {
          return current;
        }
        return list.length > 0 ? list[0]!.id : null;
      });
    }
    const active = await run(() => api.listActiveSessions());
    if (active) {
      setSessions(active);
    }
  }, [run]);

  // 首屏：恢复残留会话 + 拉列表
  useEffect(() => {
    let cancelled = false;
    (async () => {
      const report = await run(() => api.recoverSessions());
      if (!cancelled && report) {
        setBoot(report);
        if (report.error) {
          pushToast(`恢复会话失败：${report.error}`);
        }
      }
      await refresh();
      if (!cancelled) {
        setLoading(false);
      }
    })();
    return () => {
      cancelled = true;
    };
    // refresh/run 是 useCallback 稳定引用，这里只跑一次
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, []);

  // 轮询间隔读的是运行中会话数，但绝不能让 effect 依赖 sessions：
  // 那样每次轮询写回 sessions 都会重跑 effect、重置定时器，形成自我触发的循环。
  const runningCountRef = useRef(0);

  // 会话列表轮询：运行中 1s，全部退出后放慢到 3s
  useEffect(() => {
    let timer: number | undefined;
    let cancelled = false;

    const tick = async () => {
      if (document.visibilityState === "visible") {
        const active = await run(() => api.listActiveSessions());
        if (!cancelled && active) {
          runningCountRef.current = active.filter((s) => s.state === "running").length;
          setSessions(active);
        }
      }
      if (cancelled) {
        return;
      }
      const running = runningCountRef.current;
      timer = window.setTimeout(tick, running > 0 ? 1000 : 3000);
    };

    timer = window.setTimeout(tick, 1000);
    return () => {
      cancelled = true;
      if (timer !== undefined) {
        window.clearTimeout(timer);
      }
    };
  }, [run]);

  const selectedApp = useMemo(
    () => apps.find((app) => app.id === selectedId) ?? null,
    [apps, selectedId]
  );

  const appSessions = useMemo(() => {
    if (!selectedApp) {
      return [];
    }
    return sessions.filter((session) => session.app_id === selectedApp.id);
  }, [sessions, selectedApp]);

  const activeSession = useMemo(
    () => appSessions.find((session) => session.session_id === selectedSession) ?? null,
    [appSessions, selectedSession]
  );

  // 选中的应用变了：默认选中它的第一个活跃会话
  useEffect(() => {
    setSelectedSession((current) => {
      if (current && appSessions.some((session) => session.session_id === current)) {
        return current;
      }
      return appSessions.length > 0 ? appSessions[0]!.session_id : null;
    });
  }, [appSessions]);

  const handleStart = useCallback(
    async (app: AppConfig) => {
      setBusy(true);
      const snapshot = await run(() => api.startApp(app.id));
      setBusy(false);
      if (snapshot) {
        setSessions((prev) => prev.concat(snapshot));
        setSelectedId(app.id);
        setSelectedSession(snapshot.session_id);

        // 会话起来后窗口才存在：此刻才可能抓到基线说的「运行时真图标」。
        // 延迟一拍再抓，给窗口创建留出时间（抓不到也无所谓，静默兜底）。
        invalidateIcon(app.id);
        window.setTimeout(() => setIconRefresh((value) => value + 1), RUNTIME_ICON_DELAY_MS);
      }
    },
    [run]
  );

  const handleStop = useCallback(
    async (sessionId: string) => {
      setBusy(true);
      const done = await run(() => api.stopSession(sessionId));
      setBusy(false);
      if (done !== undefined) {
        // 停止后立刻刷新一次，让状态徽标马上回落到「已退出」
        const active = await run(() => api.listActiveSessions());
        if (active) {
          setSessions(active);
        }
      }
    },
    [run]
  );

  const handleRestart = useCallback(
    async (sessionId: string, appId: string) => {
      setBusy(true);
      const snapshot = await run(() => api.restartSession(sessionId, appId));
      setBusy(false);
      if (snapshot) {
        setSessions((prev) => prev.concat(snapshot));
        setSelectedSession(snapshot.session_id);
      }
    },
    [run]
  );

  const handleSave = useCallback(
    async (input: AppConfigInput, isNew: boolean) => {
      setBusy(true);
      const saved = await run(() =>
        isNew ? api.createApp(input) : api.updateApp(input)
      );
      setBusy(false);
      if (saved) {
        await refresh();
        setSelectedId(saved.id);
        setFormTarget(undefined);
        pushToast(isNew ? `已创建应用「${saved.name}」` : `已保存应用「${saved.name}」`);
      }
    },
    [pushToast, refresh, run]
  );

  const handleDelete = useCallback(async () => {
    if (!deleteTarget) {
      return;
    }
    setBusy(true);
    const done = await run(() => api.deleteApp(deleteTarget.app.id));
    setBusy(false);
    setDeleteTarget(null);
    if (done !== undefined) {
      await refresh();
      pushToast(`已删除应用「${deleteTarget.app.name}」`);
    }
  }, [deleteTarget, pushToast, refresh, run]);

  const editing = formTarget !== undefined;

  return (
    <div className="shell">
      <header className="topbar">
        <div className="brand">
          <span className="brand-mark">ac</span>
          <div className="brand-text">
            <strong>app_catch</strong>
            <span className="brand-sub">开发者向应用启动器</span>
          </div>
        </div>

        <div className="topbar-actions">
          <button
            type="button"
            className="btn btn-primary"
            onClick={() => setFormTarget(null)}
            disabled={busy}
          >
            + 新建应用
          </button>
        </div>
      </header>

      <div className="body">
        <aside className="sidebar">
          <div className="sidebar-head">
            <span>应用</span>
            <span className="count">{apps.length}</span>
          </div>
          {loading ? (
            <p className="sidebar-hint">加载中…</p>
          ) : (
            <AppList
              apps={apps}
              sessions={sessions}
              selectedId={selectedId}
              onSelect={(app) => {
                setSelectedId(app.id);
                setFormTarget(undefined);
              }}
              onStart={handleStart}
              busy={busy}
              iconRefresh={iconRefresh}
            />
          )}
        </aside>

        <main className="main">
          {editing && formTarget !== undefined ? (
            <AppForm
              initial={formTarget}
              busy={busy}
              onSubmit={(input, isNew) => handleSave(input, isNew)}
              onCancel={() => setFormTarget(undefined)}
            />
          ) : formTarget === null ? (
            <AppForm
              initial={null}
              busy={busy}
              onSubmit={(input, isNew) => handleSave(input, isNew)}
              onCancel={() => setFormTarget(undefined)}
            />
          ) : selectedApp ? (
            <div className="detail-stack">
              <AppDetail
                app={selectedApp}
                sessions={appSessions}
                selectedSession={activeSession?.session_id ?? null}
                busy={busy}
                onStart={() => handleStart(selectedApp)}
                onStop={handleStop}
                onRestart={handleRestart}
                onEdit={() => setFormTarget(selectedApp)}
                onDelete={() => setDeleteTarget({ app: selectedApp })}
                onSelectSession={setSelectedSession}
                iconRefresh={iconRefresh}
              />
              {activeSession ? (
                <SessionDetail
                  key={activeSession.session_id}
                  session={activeSession}
                  allSessions={sessions}
                />
              ) : (
                <p className="empty-panel">该应用当前没有运行中的会话。</p>
              )}
            </div>
          ) : (
            <div className="empty-state">
              <h2>还没有任何应用</h2>
              <p>app_catch 不做自动发现，应用需要手动录入。</p>
              <button type="button" className="btn btn-primary" onClick={() => setFormTarget(null)}>
                + 新建第一个应用
              </button>
            </div>
          )}
        </main>
      </div>

      {deleteTarget ? (
        <div className="modal-backdrop" role="dialog" aria-modal="true">
          <div className="modal">
            <h3>确认删除</h3>
            <p>
              将删除应用配置「<strong>{deleteTarget.app.name}</strong>」（id: {deleteTarget.app.id}）。
            </p>
            <p className="modal-note">已有会话目录与日志不会被删除，仍可在 sessions 目录中查看。</p>
            <div className="modal-actions">
              <button type="button" className="btn" onClick={() => setDeleteTarget(null)}>
                取消
              </button>
              <button type="button" className="btn btn-danger" onClick={handleDelete} disabled={busy}>
                确认删除
              </button>
            </div>
          </div>
        </div>
      ) : null}

      <Toast toasts={toasts} onDismiss={dismissToast} />

      {boot?.error ? <span className="sr-only">{boot.home}</span> : null}
    </div>
  );
}
