import { spawn } from "node:child_process";
import { once } from "node:events";
import { readFileSync } from "node:fs";
import net from "node:net";
import process from "node:process";
import { fileURLToPath } from "node:url";

import { expect, test } from "vitest";

const rootDir = fileURLToPath(new URL("..", import.meta.url));
const pkg = JSON.parse(readFileSync(new URL("../package.json", import.meta.url), "utf8"));
const tauriConfig = JSON.parse(
  readFileSync(new URL("../src-tauri/tauri.conf.json", import.meta.url), "utf8"),
);
const devPort = Number(new URL(tauriConfig.build.devUrl).port || 15173);

function isPortOpen(port) {
  return new Promise((resolve) => {
    const socket = net.connect({ host: "127.0.0.1", port });
    socket.once("connect", () => {
      socket.destroy();
      resolve(true);
    });
    socket.once("error", () => {
      socket.destroy();
      resolve(false);
    });
  });
}

async function waitForPort(port, expected, timeoutMs) {
  const deadline = Date.now() + timeoutMs;
  while (Date.now() < deadline) {
    if ((await isPortOpen(port)) === expected) {
      return true;
    }
    await new Promise((resolve) => setTimeout(resolve, 200));
  }
  return false;
}

test("dev 脚本与 Tauri beforeDevCommand 保持同一条受管启动链", () => {
  expect(pkg.scripts.dev).toBe("node scripts/dev-server.mjs");
  expect(tauriConfig.build.beforeDevCommand).toBe("pnpm dev");
});

const portAlreadyInUse = await isPortOpen(devPort);
const integrationTest = portAlreadyInUse ? test.skip : test;

integrationTest(
  "父进程 stdin 关闭后 dev 启动器释放 Vite 端口",
  async () => {
    const child = spawn(process.execPath, ["scripts/dev-server.mjs"], {
      cwd: rootDir,
      env: { ...process.env, NO_COLOR: "1" },
      stdio: ["pipe", "pipe", "pipe"],
      windowsHide: true,
    });

    let stdout = "";
    let stderr = "";
    child.stdout.on("data", (chunk) => {
      stdout += chunk;
    });
    child.stderr.on("data", (chunk) => {
      stderr += chunk;
    });

    try {
      const ready = await waitForPort(devPort, true, 30_000);
      expect(ready, `等待 Vite 监听 ${devPort} 超时。stderr: ${stderr}`).toBe(true);

      child.stdin.end();
      const exit = await Promise.race([
        once(child, "exit").then(([code, signal]) => ({ code, signal })),
        new Promise((resolve) => setTimeout(() => resolve("timeout"), 10_000)),
      ]);
      expect(exit, `dev 启动器未在 stdin 关闭后退出。stdout: ${stdout}`).not.toBe("timeout");

      const released = await waitForPort(devPort, false, 10_000);
      expect(released).toBe(true);
    } finally {
      if (child.exitCode === null && child.signalCode === null) {
        child.kill("SIGKILL");
        await once(child, "exit").catch(() => {});
      }
    }
  },
  40_000,
);
