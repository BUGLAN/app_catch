import React from "react";
import ReactDOM from "react-dom/client";

import App from "./App";
import { suppressNativeContextMenu } from "./lib/contextMenu";
import { applyPlatform } from "./lib/platform";
import "./styles/tokens.css";
import "./styles/base.css";
import "./styles/layout.css";
import "./styles/chrome.css";
import "./styles/primitives.css";
import "./styles/apps.css";
import "./styles/settings.css";
import "./styles/motion.css";

// 平台属性由 index.html 的内联脚本先写一次（防首帧错位）；
// 这里再写一次是幂等的 —— 测试环境或热更新直接挂载时也能保证属性存在。
applyPlatform();

// 桌面应用不该弹出 WebView 的原生右键菜单（检查元素 / 重新加载等），输入框除外
suppressNativeContextMenu();

ReactDOM.createRoot(document.getElementById("root") as HTMLElement).render(
  <React.StrictMode>
    <App />
  </React.StrictMode>
);
