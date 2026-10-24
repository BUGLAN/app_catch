import { defineConfig } from "vitest/config";

/**
 * vitest 配置：只跑纯逻辑测试（无 DOM 依赖）。
 *
 * 为了不给项目引入 jsdom（约 4MB 依赖），这里保持默认的 node 环境，
 * 测试目标限定在不碰 DOM 的模块：`src/lib/logModel.ts` 与
 * `src/components/AppForm.tsx` 里导出的纯函数（表单组件本身不渲染）。
 * 组件渲染级测试留给后续里程碑，需要时再引入 @testing-library/react。
 */
export default defineConfig({
  test: {
    environment: "node",
    include: ["src/**/*.test.ts"],
    reporters: ["default"],
  },
});
