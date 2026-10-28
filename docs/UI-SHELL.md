# 界面与外壳（M6）：自绘标题栏、可折叠抽屉与双主题

> 本文描述 M6 的界面结构、跨平台窗口差异与设计 token。
> 视觉走查的实测数据见 [UI-AUDIT.md](UI-AUDIT.md)，命令层 API 见 [GUI.md](GUI.md)。

## 0. 设计假设（原需求未给定、由本次实现决定的部分）

需求里"可自由发挥"的地方，这里明确列出取舍依据，便于后续调整：

| 决策点 | 选择 | 理由 |
| --- | --- | --- |
| 主色 | macOS 系统蓝：深色填充 `#0071e3`、浅色 `#0066cc`；中性色为纯石墨灰 | 白字 ≥ 4.7:1；原靛紫 + 蓝灰底在真机上观感偏"AI 产品"，改为与 macOS 原生一致的灰阶 + 系统蓝，颜色只用于可操作与状态 |
| 主题切换位置 | 侧栏底部、与「设置」相邻 | 标题栏右侧位置留给业务操作与窗口控制；偏好项归入设置区 |
| 主题交互 | 单按钮三态轮转（跟随系统 → 浅色 → 深色） | 二元开关无法表达"回到跟随系统" |
| 全局搜索 | 标题栏中部，过滤名称 / 标签 / id | 需求给的 Header 建议结构里"中间可选全局搜索" |
| 抽屉两段划分 | 应用列表 / 底部主题·设置·宿主状态；折叠开关在标题栏 | 顶部原本只有一个折叠按钮独占一行；底部状态点同时解决了"是否连着 Tauri"的疑问 |
| 抽屉折叠态交互 | 图标 + portal tooltip | 折叠后 56px 放不下文字，tooltip 是名称的唯一出口 |
| macOS 安全区宽度 | 92px；`trafficLightPosition` = (18, 26) | 真机实测：红黄绿右缘约 72px，80px 会让徽标贴着绿灯。y 与按钮中心的关系实测为「中心 ≈ y − 2.5」，26 使其与 48px 标题栏中线对齐 |

## 1. 文件结构

```text
src-tauri/
├── tauri.conf.json            主配置：decorations:false（Windows/Linux 无边框）
├── tauri.macos.conf.json      macOS 覆盖：titleBarStyle:Overlay + 红黄绿位置
├── capabilities/default.json  core:default + core:window:allow-start-dragging
├── src/window.rs              六个窗口命令 + 最大化状态事件
├── src/lib.rs                 注册命令、setup 挂监听
└── tests/window_config.rs     平台配置的合并结果与权限契约（4 个测试）

src/
├── index.html → index.html    首屏内联脚本：先定 theme/platform，避免闪一帧
├── main.tsx                   引入 chrome.css，挂载前 applyPlatform()
├── App.tsx                    标题栏 + 抽屉 + 主区的新骨架
├── components/
│   ├── Titlebar.tsx           自绘 Header（拖拽区、搜索、业务操作）
│   ├── WindowControls.tsx     最小化 / 最大化（还原）/ 关闭
│   ├── Sidebar.tsx            三段式抽屉 + 4px 拖拽热区
│   ├── AppList.tsx            列表项（折叠态走 tooltip）
│   └── ui/Tooltip.tsx         portal 气泡
├── hooks/
│   ├── useTheme.ts            三态主题 + 持久化
│   ├── useSidebar.ts          折叠 + 宽度 + 持久化
│   └── useMediaQuery.ts       窄窗口强制图标态的同步
├── lib/
│   ├── platform.ts            平台识别（data-platform）
│   ├── preferences.ts         localStorage 安全读写
│   └── search.ts              应用过滤（纯函数）
└── styles/
    ├── tokens.css             调色板 / 语义 token / 尺度 token / 双主题
    ├── chrome.css             标题栏 + 窗口控制 + 抽屉（跨平台差异都在这）
    ├── layout.css             骨架与主区
    ├── primitives.css         按钮 / 卡片 / 表格 / 弹窗 / toast / 骨架屏
    ├── apps.css               列表项 / 图标 / 详情 / 日志
    └── base.css               reset / 焦点 / 滚动条 / 减少动效
```

## 2. 自绘标题栏

### 2.1 结构

```text
┌──────────────────────────────────────────────────────────────────────────┐
│ ●●●  [▮] app_catch  [▯]  [搜索…]                              [+]  ─ □ ✕ │  48px
└──────────────────────────────────────────────────────────────────────────┘
       └ logo + 名称 ┘ └开关┘ └ search ┘                      └新建┘└ window ┘
       └─────────────── data-tauri-drag-region="deep" ─────────────────────┘
```

- logo 在两个平台上都是第一位（macOS 上位于红黄绿之后）；侧栏开关紧跟名称，不再在侧栏顶部独占一行。
- 新建是图标按钮（+），名称与快捷键（⌘N / Ctrl+N）在 `title` / `aria-label` / `aria-keyshortcuts` 里；
  不再用带字的主色按钮——新建是低频操作，不该抢详情页「启动」的视觉重心。
- 主区所有视图（详情 / 表单 / 设置）共用一条 `--content-max: 1040px` 的居中内容列，切换视图时左右边缘不动。

- 高度 `--titlebar-height: 48px`，背景实色 `--surface-raised`（下方无滚动内容，blur 只增加合成成本，且 WKWebView 需前缀），底部 1px 分割线。
- 关闭按钮 hover 变红 `--window-close-hover: #e81123`，图标同时转白（深色主题下浅灰在红底上对比不足）。

### 2.2 拖拽

Tauri 的 `window` 插件会自动注入一段脚本，读取 `data-tauri-drag-region`：

| 属性值 | 行为 |
| --- | --- |
| （无） | 只有直接点在该元素上才拖 |
| `deep` | 子树任意位置都能拖（本项目用这个） |
| `false` | 阻断拖拽（搜索框用） |

这段脚本**同时实现了双击最大化/还原**，因此前端不能再绑双击处理——两处都切会出现"双击后没变化"。

> **权限陷阱**：拖拽实际调用 `plugin:window|start_dragging`，它**不在** `core:window` 的默认权限集中。
> 必须在 `capabilities/default.json` 显式声明 `core:window:allow-start-dragging`，
> 否则按下标题栏没有任何反应，也不会有任何报错。`tests/window_config.rs` 会守住这条。

### 2.3 窗口控制 IPC

`src-tauri/src/window.rs`：

| 命令 | 说明 |
| --- | --- |
| `window_minimize` | 最小化 |
| `window_maximize` | 最大化（已最大化时幂等） |
| `window_unmaximize` | 还原（未最大化时幂等） |
| `window_toggle_maximize` | 二合一，**返回切换后的状态**，图标可立即更新 |
| `window_is_maximized` | 首屏同步（刷新页面时窗口可能已经是最大化） |
| `window_close` | 关闭 |

状态同步走事件而非轮询：`Resized` 事件密集触发，但只在**结果真的变了**时才 `emit("window:maximized-changed", bool)`。
前端同时用三处兜底：首屏查询、事件推送、点击命令的返回值。

## 3. 跨平台差异

### 3.1 主进程配置

| | Windows / Linux | macOS |
| --- | --- | --- |
| 配置文件 | `tauri.conf.json` | 叠加 `tauri.macos.conf.json` |
| `decorations` | `false` | `true` |
| `titleBarStyle` | — | `"Overlay"`（tao 映射为 titlebar_transparent + fullsize_content_view） |
| `hiddenTitle` | — | `true`（不显示标题文字） |
| `trafficLightPosition` | — | `{x: 18, y: 21}` |
| 窗口按钮 | 前端自绘 | 系统红黄绿 |

平台配置是 **JSON Merge Patch（RFC 7396）**：`app.windows` 是数组，整体替换而非逐项合并，
所以 macOS 配置里必须重复声明 label / 尺寸，测试会校验两者一致。

> `WindowConfig` 是 `deny_unknown_fields` + camelCase：字段名拼错会导致**应用直接启动失败**，
> 而且只在对应平台暴露。`tests/window_config.rs` 在任意平台上把合并结果反序列化一次，
> 把这类错误提前到 CI（已用"故意写成 titlebarStyle"验证过会红）。

Windows 无边框后仍可缩放：tao 对未最大化的无边框窗口保留了 8 个边缘的命中测试，无需自己实现 resize 热区。

### 3.2 前端 CSS

```css
/* macOS：给红黄绿让出 92px 安全区（padding 而非 margin，安全区本身仍可拖动） */
[data-platform="macos"] .titlebar { padding-left: var(--titlebar-mac-inset); }

/* macOS：不显示自绘窗口按钮（DOM 保留，读屏与快捷键结构两端一致） */
[data-platform="macos"] .window-controls { display: none; }
```

`data-platform` 由 `index.html` 的内联脚本在首帧写入（UA 判断），`src/lib/platform.ts` 再幂等地写一次。

## 4. 左侧抽屉

### 4.1 宽度模型

只有一个宽度状态：`width` 是**展开态**宽度。

| 状态 | 渲染宽度 |
| --- | --- |
| 折叠 | `--rail-collapsed-width: 56px` |
| 展开 | `width`（默认 220，范围 184–320；拖到 120 以下吸附为折叠态，不存在半折叠宽度） |
| 窗口 ≤ 900px | 强制 56px（CSS 断点 + `useMediaQuery` 同步给 React） |

折叠 → 展开会回到用户上次拖出来的宽度，不需要记两份数字。

### 4.2 拖拽

- 热区：`.rail-handle` 宽 4px，用 `::before` 向两侧各扩 4px 提高命中率，`cursor: col-resize`。
- 指针捕获：`setPointerCapture` 后，指针移出侧栏甚至移出窗口仍能收到 move/up（否则拖快了会"卡住"）。
- 拖拽期间 `.sidebar[data-resizing="true"] { transition: none }`：否则每帧都补间，视觉上像在追指针。
- 键盘：热区是 `<button>`，方向键调宽（Shift 加速）、Home/End 到两端、Enter 切换折叠。

### 4.3 持久化

`localStorage` 键（由 `preferences.test.ts` 守住）：

| 键 | 值 |
| --- | --- |
| `app_catch.sidebar` | `{"collapsed":bool,"width":number}` |
| `app_catch.theme` | `"system" \| "light" \| "dark"` |

写入节流 200ms：拖拽时每帧同步写 localStorage 会掉帧，而"松手 200ms 后落盘"对刷新恢复没有体感差异。
读到的值一律校验（非法值 / NaN / 越界都会退回默认），手改 localStorage 不会白屏。

## 5. 主题

`<html data-theme="dark|light" data-theme-mode="system|light|dark">`：

- `data-theme` 是**解析后的结果**，CSS 只认它；
- `data-theme-mode` 记录用户意图，设置项回显用；
- 首帧由 `index.html` 的内联脚本写入，避免先渲染一帧深色再跳浅色。

浅色主题只覆盖语义层（`--surface-*` / `--text-*` / `--border-*` / 状态色 / 阴影），
调色板与尺度 token 两套主题共用。

## 6. 响应式

| 断点 | 行为 |
| --- | --- |
| ≥1180 | 主区留白 20px |
| 1180–901 | 主区留白 16px |
| ≤900 | 抽屉强制图标态；热区隐藏；tooltip 接管名称 |
| ≤760 | 隐藏标题栏副标题与搜索；主区内边距收到 12px |
| ≤560 | 卡片内边距收到 12px；主区 8px |

任意宽度都不出现文档级横向滚动；需要横向滚动的表格自带 `.table-scroll`。
