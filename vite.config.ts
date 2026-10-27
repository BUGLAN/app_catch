import { readFileSync } from "node:fs";

import { defineConfig } from "vite";
import react from "@vitejs/plugin-react";

// 版本号的单一来源是 package.json：
// - 安装包用 src-tauri/tauri.conf.json 的 version，两者必须一致
//   （src/lib/versionContract.test.ts 在 CI 里守护这条契约）；
// - 浏览器预览（非 Tauri 宿主）拿不到运行时 getVersion()，
//   这里在编译期把 package.json 的 version 注入成 __APP_VERSION__ 兜底。
const pkg = JSON.parse(
  readFileSync(new URL("./package.json", import.meta.url), "utf8"),
) as { version: string };

// Tauri 2 开发期约定：devUrl 固定 http://localhost:5173，
// 因此这里锁死端口（strictPort），避免被占用后随机漂移导致 Tauri 白屏。
export default defineConfig({
  plugins: [react()],
  define: {
    __APP_VERSION__: JSON.stringify(pkg.version),
  },
  clearScreen: false,
  server: {
    port: 5173,
    strictPort: true,
    host: "127.0.0.1",
  },
  build: {
    outDir: "dist",
    target: "chrome105",
    sourcemap: false,
  },
});
