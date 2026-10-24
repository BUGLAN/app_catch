import { defineConfig } from "vite";
import react from "@vitejs/plugin-react";

// Tauri 2 开发期约定：devUrl 固定 http://localhost:5173，
// 因此这里锁死端口（strictPort），避免被占用后随机漂移导致 Tauri 白屏。
export default defineConfig({
  plugins: [react()],
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
