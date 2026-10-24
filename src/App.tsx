import { useEffect, useState } from "react";
import { invoke } from "@tauri-apps/api/core";

/** 从 Tauri 宿主获取核心库版本；非 Tauri 环境（纯浏览器预览）返回 null。 */
async function fetchCoreVersion(): Promise<string | null> {
  try {
    return await invoke<string>("core_version");
  } catch {
    return null;
  }
}

export default function App() {
  const [coreVersion, setCoreVersion] = useState<string | null>(null);
  const [hosted, setHosted] = useState<boolean>(false);

  useEffect(() => {
    fetchCoreVersion().then((v) => {
      setCoreVersion(v);
      setHosted(v !== null);
    });
  }, []);

  return (
    <main className="app">
      <header className="app-header">
        <h1 className="app-title">app_catch</h1>
        <p className="app-subtitle">开发者向桌面应用启动器</p>
      </header>

      <section className="card">
        <h2>骨架状态</h2>
        <dl className="kv">
          <dt>应用名称</dt>
          <dd>app_catch</dd>
          <dt>launcher-core 版本</dt>
          <dd>{coreVersion ?? "（未连接到 Tauri 宿主）"}</dd>
          <dt>运行环境</dt>
          <dd>{hosted ? "Tauri 桌面窗口" : "浏览器预览模式"}</dd>
          <dt>当前里程碑</dt>
          <dd>M0 仓库初始化与脚手架</dd>
        </dl>
      </section>

      <section className="card muted">
        <h2>下一步</h2>
        <p>
          M1 起将在 launcher-core 中实现应用配置管理、进程启动、会话 JSONL
          捕获与进程树回收；本页面目前仅用于验证前后端链路是否打通。
        </p>
      </section>
    </main>
  );
}
