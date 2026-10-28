import { fstatSync } from "node:fs";
import process from "node:process";
import { fileURLToPath } from "node:url";

import { createServer, version as viteVersion } from "vite";

/**
 * Windows 下 Tauri CLI 的 beforeDevCommand 只启动一个 cmd 子进程，CLI 被
 * pnpm/Node 的 job 提前终止时不会执行自己的进程树清理逻辑。若这里再
 * spawn 一个独立的 vite 进程，它会变成孤儿并继续占用 5173。
 *
 * 因此本启动器不再把 Vite 当子进程，而是直接在当前进程里创建 dev server：
 * 5173 的监听者就是 Tauri beforeDevCommand 的进程本身，不存在需要额外
 * 追踪的 Vite 子进程。收到 Ctrl+C 或 Tauri 的 stdin 管道关闭时，调用
 * server.close() 停止 watcher / WebSocket / esbuild，然后退出。
 */
const rootDir = fileURLToPath(new URL("..", import.meta.url));

let server;
let shuttingDown = false;

async function shutdown(reason, exitCode = 0) {
  if (shuttingDown) {
    return;
  }
  shuttingDown = true;
  console.error(`[dev] ${reason}，正在关闭 Vite...`);
  try {
    await server?.close();
  } catch (error) {
    console.error("[dev] 关闭 Vite 时出错:", error?.message ?? error);
  }
  process.exit(exitCode);
}

/**
 * 普通终端或 CI 的 stdin 是字符设备/普通文件；只有 Tauri CLI 用匿名管道
 * 连接 beforeDevCommand。仅在管道场景监听 EOF，避免把后台运行误判为父进程
 * 已退出。Tauri CLI 消失（包括被 taskkill 或父 Node 的 job 杀掉）时会关闭
 * 写端，这里就能收到 end/close，而 Vite 自己从不读取 stdin。
 */
function isWatchablePipe(fd) {
  try {
    const stat = fstatSync(fd);
    if (stat.isFIFO()) {
      return true;
    }
    // Windows 匿名管道在 Node 的 fstat 中既不是字符设备也不是普通文件。
    return !stat.isCharacterDevice() && !stat.isFile();
  } catch {
    return false;
  }
}

try {
  const startedAt = Date.now();
  server = await createServer({ root: rootDir });
  await server.listen();
  console.log(
    `\n  VITE v${viteVersion}  ready in ${Date.now() - startedAt} ms\n`,
  );
  server.printUrls();
} catch (error) {
  console.error("[dev] Vite 启动失败:", error?.message ?? error);
  process.exit(1);
}

if (isWatchablePipe(0)) {
  process.stdin.on("end", () => void shutdown("Tauri 开发进程已退出"));
  process.stdin.on("close", () => void shutdown("Tauri 开发进程已退出"));
  process.stdin.on("error", () => void shutdown("Tauri 开发进程 stdin 异常"));
  process.stdin.resume();
}

process.on("SIGINT", () => void shutdown("收到 Ctrl+C", 130));
process.on("SIGTERM", () => void shutdown("收到 SIGTERM", 143));
if (process.platform !== "win32") {
  process.on("SIGHUP", () => void shutdown("收到 SIGHUP", 129));
}
