/**
 * app_catch 主界面：自绘标题栏 + 左侧抽屉 + 右侧详情/会话。
 *
 * 状态管理刻意只用一个 useState 承载「数据 + 选中态」，
 * 轮询逻辑集中在 effect 里，组件本身只负责渲染。
 *
 * 本轮改动只碰表现层与窗口外壳：
 *   所有命令调用、轮询节奏、选中规则、表单载荷完全保持原样。
 *
 * 新增的三件事：
 *   1. 自绘标题栏（Titlebar）替代原来的 .topbar，移除系统窗口边框；
 *   2. 左侧抽屉（Sidebar）可折叠、可拖拽调宽，状态持久化；
 *   3. 标题栏全局搜索：按名称 / 标签 / id 过滤左侧列表。
 */
import { useCallback, useEffect, useLayoutEffect, useMemo, useRef, useState } from "react";

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
import { SettingsView } from "./components/SettingsView";
import { Sidebar } from "./components/Sidebar";
import { Titlebar } from "./components/Titlebar";
import { Dialog } from "./components/ui/Dialog";
import { IconPlus } from "./components/ui/Icons";
import { EmptyState, PanelSkeleton } from "./components/ui/Primitives";
import { Toast, type ToastMessage, type ToastTone } from "./components/Toast";
import { useRailCompact } from "./hooks/useMediaQuery";
import { usePresence } from "./hooks/usePresence";
import { useSidebar } from "./hooks/useSidebar";
import { useTheme } from "./hooks/useTheme";
import { isHosted } from "./lib/api";
import { readUpdatePrefs } from "./lib/preferences";
import { playViewEnter } from "./lib/motion";
import { filterApps } from "./lib/search";
import { backgroundCheckForUpdate } from "./lib/updater";

/**
 * 会话启动后等待多久再抓「运行时图标」（毫秒）。
 *
 * 窗口创建通常在 1 秒内完成（PySide/Electron 首次启动可能更久）。
 * 取 1.5 秒是折中：太早窗口还没出来，太晚用户已经看过列表了。
 * 抓不到也不影响任何功能——前端会一直显示默认图。
 */
const RUNTIME_ICON_DELAY_MS = 1500;

/** 同时显示的 toast 上限：再多会盖住界面。 */
const MAX_TOASTS = 3;

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
  const [query, setQuery] = useState("");

  const [formTarget, setFormTarget] = useState<FormTarget | undefined>(undefined);
  /** 设置页（关于 / 版本 / 更新）是否打开：打开时主区整体切换。 */
  const [settingsOpen, setSettingsOpen] = useState(false);
  /** 启动静默检查发现的新版本：进设置页直接展示，消费后清空。 */
  const [availableVersion, setAvailableVersion] = useState<string | null>(null);
  const [deleteTarget, setDeleteTarget] = useState<DeleteTarget | null>(null);
  const [busy, setBusy] = useState(false);
  const [loading, setLoading] = useState(true);
  const [boot, setBoot] = useState<BootReport | null>(null);
  const [toasts, setToasts] = useState<ToastMessage[]>([]);
  /** 图标刷新触发器：会话启动后 +1，让列表与详情重新抓「运行时图标」。 */
  const [iconRefresh, setIconRefresh] = useState(0);
  /**
   * 本次刷新是否要穿透缓存。
   *
   * 只有「会话刚启动」那一次是 true：那一刻窗口才出现，窗口档位第一次可能拿到
   * 应用自设的真图标，必须能覆盖磁盘上早先抓到的 `pnpm.cmd` 通用图标。
   * 其余刷新保持 false，避免每次点启动都白跑一遍 Shell API。
   */
  const [iconForceRefresh, setIconForceRefresh] = useState(false);

  const theme = useTheme();
  const rail = useSidebar();
  /*
   * 图标态 = 用户折叠 或 窗口太窄（CSS 强制）。
   * 两个来源必须在这里合并：列表的 tooltip、骨架屏形态都只看这一个布尔值。
   *
   * 注意：hook 必须无条件调用（不能写成 `rail.collapsed || useRailCompact()`）。
   * 短路求值会让折叠时整个 hook 不被调用，Hook 顺序随之改变 ——
   * 展开/折叠一次就会触发 "Rendered fewer hooks than expected" 白屏。
   */
  const windowNarrow = useRailCompact();
  const compactRail = rail.collapsed || windowNarrow;
  const hosted = useMemo(() => isHosted(), []);

  const toastSeq = useRef(0);

  /**
   * 追加一条 toast。
   *
   * 相同文案不重复堆叠：轮询类命令失败时原来每秒都会 push 一条一模一样的提示
   * （实测界面上同时出现 3 条相同红条）。这里复用旧条目并刷新它的 id，
   * 让 React 重新挂载该条目从而重置 3.5s 计时。
   */
  const pushToast = useCallback((input: CommandError | string, tone: ToastTone = "error") => {
    const message = typeof input === "string" ? input : input.message;
    toastSeq.current += 1;
    const id = toastSeq.current;
    setToasts((prev) => {
      const deduped = prev.filter((toast) => toast.message !== message);
      const next = deduped.concat({ id, message, tone });
      return next.length > MAX_TOASTS ? next.slice(next.length - MAX_TOASTS) : next;
    });
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

  // 启动静默检查更新：只在「宿主内 + 非开发模式 + 偏好开启」时进行。
  // backgroundCheckForUpdate 内部吞掉一切失败——启动路径上的网络抖动不该弹错误；
  // 发现新版本只提示一次，安装动作留给用户进设置页主动触发。
  useEffect(() => {
    if (!hosted || import.meta.env.DEV || !readUpdatePrefs().autoCheck) {
      return;
    }
    let cancelled = false;
    (async () => {
      const found = await backgroundCheckForUpdate();
      if (!cancelled && found) {
        setAvailableVersion(found);
        pushToast(`发现新版本 v${found}，可在「设置」中安装`, "success");
      }
    })();
    return () => {
      cancelled = true;
    };
  }, [hosted, pushToast]);

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

  /** 搜索过滤后的列表（只影响显示，不动原始数据与选中规则）。 */
  const visibleApps = useMemo(() => filterApps(apps, query), [apps, query]);

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
        window.setTimeout(() => {
          // 强制穿透磁盘缓存：此刻窗口已存在，窗口档位才有可能抓到真图标
          setIconForceRefresh(true);
          setIconRefresh((value) => value + 1);
        }, RUNTIME_ICON_DELAY_MS);
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
      const saved = await run(() => (isNew ? api.createApp(input) : api.updateApp(input)));
      setBusy(false);
      if (saved) {
        await refresh();
        setSelectedId(saved.id);
        setFormTarget(undefined);
        pushToast(
          isNew ? `已创建应用「${saved.name}」` : `已保存应用「${saved.name}」`,
          "success"
        );
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
      pushToast(`已删除应用「${deleteTarget.app.name}」`, "success");
    }
  }, [deleteTarget, pushToast, refresh, run]);

  /** 打开表单：null 表示新建，AppConfig 表示编辑。 */
  const editing = formTarget !== undefined;
  const openForm = useCallback((target: FormTarget) => {
    setSettingsOpen(false);
    setFormTarget(target);
  }, []);

  /** 打开设置页：同时关掉表单，主区一次只承载一个视图。 */
  const openSettings = useCallback(() => {
    setFormTarget(undefined);
    setSettingsOpen(true);
  }, []);

  const closeForm = useCallback(() => setFormTarget(undefined), []);

  const closeDialog = useCallback(() => setDeleteTarget(null), []);

  /*
   * ⌘N（macOS）/ Ctrl+N（Windows / Linux）新建应用：标题栏的 + 只剩图标，快捷键补上可发现性之外的效率。
   * 弹窗打开时不响应（焦点被困在弹窗里，新建会把它藏到背后）；带 Shift / Alt 的组合留给系统。
   */
  useEffect(() => {
    const isMac = document.documentElement.dataset.platform === "macos";
    const onKeyDown = (event: KeyboardEvent) => {
      const primary = isMac ? event.metaKey && !event.ctrlKey : event.ctrlKey && !event.metaKey;
      if (!primary || event.shiftKey || event.altKey || event.key.toLowerCase() !== "n") {
        return;
      }
      if (deleteTarget || busy) {
        return;
      }
      event.preventDefault();
      openForm(null);
    };
    window.addEventListener("keydown", onKeyDown);
    return () => window.removeEventListener("keydown", onKeyDown);
  }, [busy, deleteTarget, openForm]);

  /** 删除确认弹窗的存在态：关闭后再保留一拍，让退场动画播完。 */
  const dialogPresence = usePresence(deleteTarget);

  /**
   * 主区"当前是哪个视图"：变化时在 .main-inner 上补一段进场动画。
   *
   * 只在视图种类或选中应用变化时触发；轮询刷新、会话切换不会触发（那是同一个视图里的数据更新）。
   */
  const viewKey = settingsOpen
    ? "settings"
    : editing
      ? "form:" + (formTarget?.id ?? "__new__")
      : loading
        ? "loading"
        : selectedApp
          ? "detail:" + selectedApp.id
          : "empty";
  const mainInnerRef = useRef<HTMLDivElement | null>(null);
  const viewAnimationRef = useRef<Animation | null>(null);
  // layout effect：在浏览器绘制新视图之前挂上动画起始帧，避免先闪一帧终态
  useLayoutEffect(() => {
    viewAnimationRef.current?.cancel();
    viewAnimationRef.current = playViewEnter(mainInnerRef.current);
  }, [viewKey]);

  /**
   * 主区内容：表单 / 加载中 / 详情 / 空态。
   *
   * 表单优先于加载态：首屏还在拉列表时用户就点了「新建应用」，
   * 不能让他看着骨架屏干等而没有任何反馈。
   */
  let mainContent: React.ReactNode;
  if (settingsOpen) {
    mainContent = (
      <SettingsView
        availableVersion={availableVersion}
        onConsumeAvailableVersion={() => setAvailableVersion(null)}
      />
    );
  } else if (editing) {
    mainContent = (
      <AppForm
        key={formTarget?.id ?? "__new__"}
        initial={formTarget ?? null}
        busy={busy}
        onSubmit={handleSave}
        onCancel={closeForm}
      />
    );
  } else if (loading) {
    mainContent = <PanelSkeleton />;
  } else if (selectedApp) {
    mainContent = (
      <div className="detail-stack">
        <AppDetail
          app={selectedApp}
          sessions={appSessions}
          selectedSession={activeSession?.session_id ?? null}
          busy={busy}
          onStart={() => handleStart(selectedApp)}
          onStop={handleStop}
          onRestart={handleRestart}
          onEdit={() => openForm(selectedApp)}
          onDelete={() => setDeleteTarget({ app: selectedApp })}
          onSelectSession={setSelectedSession}
          iconRefresh={iconRefresh}
          iconForceRefresh={iconForceRefresh}
        />
        {activeSession ? (
          <SessionDetail
            key={activeSession.session_id}
            session={activeSession}
            allSessions={sessions}
          />
        ) : (
          <div className="empty-panel">该应用当前没有运行中的会话。</div>
        )}
      </div>
    );
  } else {
    mainContent = (
      <EmptyState
        title="还没有任何应用"
        description="app_catch 不做自动发现，应用需要手动录入。"
        action={
          <button type="button" className="btn btn-primary" onClick={() => openForm(null)}>
            <IconPlus size={14} />
            新建第一个应用
          </button>
        }
      />
    );
  }

  /** 侧栏内容：加载中 / 搜索无命中 / 列表。 */
  let railContent: React.ReactNode;
  if (loading) {
    railContent = (
      <AppList
        apps={[]}
        sessions={[]}
        selectedId={null}
        onSelect={() => {}}
        onStart={() => {}}
        busy
        loading
        collapsed={compactRail}
      />
    );
  } else if (apps.length > 0 && visibleApps.length === 0) {
    railContent = (
      <p className="search-empty">
        <strong>没有匹配「{query.trim()}」的应用</strong>
        <span>试试应用名、标签或 id。</span>
      </p>
    );
  } else {
    railContent = (
      <AppList
        apps={visibleApps}
        sessions={sessions}
        selectedId={selectedId}
        onSelect={(app) => {
          setSelectedId(app.id);
          setFormTarget(undefined);
          setSettingsOpen(false);
        }}
        onStart={handleStart}
        busy={busy}
        iconRefresh={iconRefresh}
        iconForceRefresh={iconForceRefresh}
        collapsed={compactRail}
      />
    );
  }

  return (
    <div
      className="shell"
      data-resizing={rail.resizing ? "true" : "false"}
    >
      <a className="skip-link" href="#main-content">
        跳到主内容
      </a>

      <Titlebar
        query={query}
        onQueryChange={setQuery}
        matchCount={query.trim().length > 0 ? visibleApps.length : null}
        onNewApp={() => openForm(null)}
        busy={busy}
        sidebarCollapsed={rail.collapsed}
        onToggleSidebar={rail.toggle}
        sidebarForced={windowNarrow}
        onWindowError={(error) => pushToast(error.message)}
      />

      <div className="body">
        <Sidebar
          rail={rail}
          themeMode={theme.mode}
          onCycleTheme={theme.cycle}
          hosted={hosted}
          onOpenSettings={openSettings}
          settingsActive={settingsOpen}
        >
          {railContent}
        </Sidebar>

        <main
          className="main"
          id="main-content"
          tabIndex={-1}
          aria-busy={loading}
          aria-label="主内容"
        >
          <div className="main-inner" ref={mainInnerRef}>
            {mainContent}
          </div>
        </main>
      </div>

      {dialogPresence.value ? (
        <Dialog
          title="确认删除"
          closing={dialogPresence.leaving}
          onClose={closeDialog}
          actions={
            <>
              <button
                type="button"
                className="btn"
                onClick={closeDialog}
                data-variant="cancel"
              >
                取消
              </button>
              <button
                type="button"
                className="btn btn-danger"
                onClick={handleDelete}
                disabled={busy}
              >
                确认删除
              </button>
            </>
          }
        >
          <p>
            将删除应用配置「<strong>{dialogPresence.value.app.name}</strong>」（id: {dialogPresence.value.app.id}）。
          </p>
          <p className="modal-note">
            已有会话目录与日志不会被删除，仍可在 sessions 目录中查看。
          </p>
        </Dialog>
      ) : null}

      <Toast toasts={toasts} onDismiss={dismissToast} />

      {boot?.error ? <span className="sr-only">数据目录：{boot.home}</span> : null}
    </div>
  );
}