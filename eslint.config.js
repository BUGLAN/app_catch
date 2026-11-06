// ESLint 扁平配置：只检查前端源码与 Node 脚本。
//
// 规则取舍：
// - typescript-eslint 的 recommended（不开类型感知规则：慢，且 tsc 已经在 build 里跑）；
// - react-hooks 只开最关键的两条：Hook 调用顺序（rules-of-hooks）与依赖完整性（exhaustive-deps）。
//   前者对应过「折叠侧栏时 Hook 数量变化导致白屏」那类问题；后者防止 effect 读到过期值。
//   插件 recommended 里基于 React Compiler 的规则面向编译器优化，本项目未启用编译器，暂不打开。
import js from "@eslint/js";
import { defineConfig } from "eslint/config";
import reactHooks from "eslint-plugin-react-hooks";
import globals from "globals";
import tseslint from "typescript-eslint";

export default defineConfig(
  {
    ignores: ["dist/", "target/", "src-tauri/", "node_modules/", "logo/", ".e2e/"],
  },
  js.configs.recommended,
  tseslint.configs.recommended,
  {
    rules: {
      // 下划线开头 = 有意不用（例如为了统一 API 形状保留的参数）
      "@typescript-eslint/no-unused-vars": [
        "error",
        { argsIgnorePattern: "^_", varsIgnorePattern: "^_", caughtErrorsIgnorePattern: "^_" },
      ],
    },
  },
  {
    files: ["src/**/*.{ts,tsx}"],
    languageOptions: {
      globals: globals.browser,
    },
    plugins: {
      "react-hooks": reactHooks,
    },
    rules: {
      "react-hooks/rules-of-hooks": "error",
      "react-hooks/exhaustive-deps": "error",
    },
  },
  {
    files: ["scripts/**/*.mjs", "*.config.{js,ts}"],
    languageOptions: {
      globals: globals.node,
    },
  }
);
