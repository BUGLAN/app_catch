import React from "react";
import ReactDOM from "react-dom/client";

import App from "./App";
import { applyPlatform } from "./lib/platform";
import "./styles/tokens.css";
import "./styles/base.css";
import "./styles/layout.css";
import "./styles/chrome.css";
import "./styles/primitives.css";
import "./styles/apps.css";
import "./styles/settings.css";

// 平台属性由 index.html 的内联脚本先写一次（防首帧错位）；
// 这里再写一次是幂等的 —— 测试环境或热更新直接挂载时也能保证属性存在。
applyPlatform();

ReactDOM.createRoot(document.getElementById("root") as HTMLElement).render(
  <React.StrictMode>
    <App />
  </React.StrictMode>
);
