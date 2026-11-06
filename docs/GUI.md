# 界面结构、命令层 API 与刷新策略

界面分左侧应用列表、右侧应用详情与会话详情，支持新建/编辑/删除应用、启动/停止/重启会话，会话输出以实时日志流呈现。

> 前置阅读：[RUNTIME.md](RUNTIME.md)（会话生命周期）、[SESSION_FORMAT.md](SESSION_FORMAT.md)（JSONL 事件格式）、[CONFIG.md](CONFIG.md)（应用配置 TOML）。

## 0. 视觉与交互约定（摘要）

- **设计 token**：颜色/间距/圆角/字号/阴影/动效全部定义在 `src/styles/tokens.css`，组件样式只引用语义层，
  不允许出现裸色值（由 `styleContract.test.ts` 守护）。
- **三态齐全**：所有可交互控件都有 hover / active / disabled，焦点靠全局 `:focus-visible` 统一提供 2px 高对比环。
- **状态不靠颜色单独编码**：徽标同时用形状（实心点 / 方块 / 菱形）+ 文本区分。
- **响应式**：1180 / 980 / 760 / 480 / 360 五档；760 以下侧栏转为图标条并保持单列主区。
  任意宽度下都不出现文档级横向滚动。
- **窗口外壳**：三端都是无边框 + 前端自绘标题栏与窗口按钮。macOS 额外开 `transparent: true`（需要 `macOSPrivateApi`），由 `src-tauri/src/window.rs` 把内容图层裁成 13px 圆角并重算阴影，外框圆角与内容区的 `--radius-window` 同值。
- **对比度**：正文 ≥ 4.5:1，控件边界 ≥ 3:1。
- **动效降级**：`prefers-reduced-motion` 下所有过渡降到 0.01ms；`forced-colors` 下补焦点框与边框。

## 1. 界面结构

```text
┌──────────────────────────────────────────────────────────────────────────┐
│ [ac] app_catch 开发者向应用启动器  [搜索…]     [+ 新建应用]  ─  □  ✕      │ 48px 自绘
├──────────────┬───────────────────────────────────────────────────────────┤
│ 应用   3  [▤]│ 应用详情                                                  │
│              │  名称 / id / 启动命令 / 工作目录 / 环境变量 / 标签          │
│ ┌──────────┐ │  [启动][停止][重启][编辑][删除]                            │
│ │ 名称     │ │ ───────────────────────────────────────────────────────── │
│ │ 标签     │ │ 会话表：会话 / PID / 状态 / 启动时间 / 退出                │
│ │ ▫状态    │ │ ───────────────────────────────────────────────────────── │
│ └──────────┘ │ 会话详情                                                  │
│  (56–320px)  │  状态条：会话id PID 状态 启动时间 窗口标题 端口 退出        │
│              │  [实时日志][最近事件]  [🔍搜索日志] □自动滚底 [清空视图]    │
│              │  ┌────────────────────────────────────────────────────┐   │
│              │  │ 18:30:01.123 stdout 服务已启动                     │   │
│              │  │ 18:30:02.456 stderr warn: ...                      │   │
│              │  └────────────────────────────────────────────────────┘   │
├──────────────┤                                                           │
│ 主题   跟随系统│                                                          │
│ 设置         │                                                           │
│ ● 已连接宿主 │                                                           │
└──────────────┴───────────────────────────────────────────────────────────┘
```

抽屉可折叠（56px 图标态，名称走 tooltip）、可拖拽右边缘调宽（56–320，默认 220），
宽度与折叠状态持久化在 `localStorage`。

### 1.1 应用列表项

| 元素 | 说明 |
| --- | --- |
| 图标 | 真实图标（30×30）：由 `AppIcon` 组件渲染，数据来自 `get_app_icon` 命令的 data URL；抓不到时用内置默认图；连默认图都取不到（非 Tauri 环境）才退回名称首字块（`initialOf`）。详见 [ICONS.md](ICONS.md) |
| 名称 | 超长省略，hover 显示完整名称 |
| 标签 | 最多显示 3 个；无标签显示虚线「无标签」 |
| 状态徽标 | `未运行` / `运行中`（多实例显示 ×N）/ `已退出` / `已崩溃`，由该应用的会话聚合得出 |
| 「启动」按钮 | 默认隐藏（`opacity:0` + `pointer-events:none`，避免误触）；hover 或键盘聚焦时**原地替换**状态徽标，不改变行宽；点击不切换选中项（`stopPropagation`）。触屏（`hover: none`）下常显 |

状态聚合规则（`aggregateState`，`src/lib/sessions.ts`）：有 running 会话 → 运行中；
否则取**最近一次**会话的结果（已退出 / 已崩溃）；没有会话 → 未运行。
只看最近一次：更早的会话崩溃过、但最近一次正常退出时，应用不会一直挂着「已崩溃」。

### 1.2 应用详情

展示 `command + args` 拼接后的完整命令行（`renderCommand`，含空格的参数自动加引号）、工作目录、
环境变量（`KEY=VALUE` 逐行）、标签、自定义图标路径。

操作按钮：

| 按钮 | 行为 |
| --- | --- |
| 启动 | 调 `start_app`，立刻把返回的快照插入会话表并选中 |
| 停止 | 停「最近启动的运行中会话」；无运行中会话时禁用 |
| 重启 | 对选中会话（无选中则取最近启动的）调 `restart_session` → 新 session_id |
| 编辑 / 删除 | 打开表单 / 二次确认弹窗 |

**多实例**：同一应用可同时有多个会话。会话表逐行列出，每行都有独立的「停止」按钮；
点击行切换下方会话详情。

### 1.3 新建 / 编辑表单

表单顶部是两个入口，选哪个决定自动填什么，下面的字段始终可以手动改：

| 入口 | 选的是 | 自动填 |
| --- | --- | --- |
| 选择项目文件夹 | 一个目录（`pick_directory`） | 工作目录 = 所选目录，再 `detect_project` 识别项目类型、给出启动命令候选 |
| 选择应用程序 | macOS `.app`，Windows `.exe` / `.bat` / `.cmd`（`pick_application`） | `inspect_application` 识别后填启动命令、名称、id（显示名推不出 id 时用文件名）；清空工作目录与参数；`.app` 默认「像 Dock 一样打开」 |

编辑已有应用时按配置推断高亮哪个入口：`.app`，或没有工作目录的 `.exe` / `.bat` / `.cmd` 视为应用程序，其余视为项目。

启动命令指向 `.app` 时出现「启动方式」单选（`launch_mode`）：「像 Dock 一样打开」（`open`）或「直接运行」（`direct`），
下方说明随选项切换；命令不是 `.app` 时一律按 `direct` 提交。

| 字段 | 约束 |
| --- | --- |
| id | 仅 `[A-Za-z0-9_-]`、长度 1–64，**实时校验**并就地提示；编辑时只读（id 是文件名，改名等于新建） |
| 名称 | 非空 |
| 启动命令 | 非空 |
| 参数 | **每行一个参数**。选这个方案而不是「单行按空白拆分」，是因为参数里可能自带空格（`--prompt hello world`），按空白拆会把参数切坏；空行自动忽略 |
| 工作目录 | 可空；空 = 继承启动器当前目录 |
| 环境变量 | **每行一条 `KEY=VALUE`**；键不可为空、不可含空白；值里可以出现 `=` 与空格 |
| 标签 | 逗号分隔（支持中文逗号），去重且保持输入顺序 |
| 自定义图标路径 | 可空 |

保存走 `create_app` / `update_app`，错误以中文 toast 展示；未提交时只做提示，
提交后未通过校验会额外显示「请先修正上面 N 处问题」。

### 1.4 删除

二次确认弹窗，明确写出**只删配置、不删会话目录**（历史日志属于审计材料）。

### 1.5 会话详情

状态条字段：会话 id（完整可复制）、PID、状态、启动时间、最新窗口标题、监听端口、退出码/原因。
数据来源是 `get_session_status` 返回的 `SessionSnapshot.latest_status`（实时最新值，不受日志限流影响）。

日志有两种视图：

- **实时日志**：按事件类型渲染成三种行

  | 形态 | 来源 | 样式 |
  | --- | --- | --- |
  | `output` | `output` 事件 | 等宽字体；统一常规色，不按 stream 或内容推断严重程度；中性的 `stdout`/`stderr` 小标签；带时间戳 |
  | `divider` | `start` / `status` / `exit` 事件 | 左侧竖线的分隔行，**不与进程输出混在一起** |
  | `notice` | `overflow` 事件 | 橙色提示条：`有 N 行输出因写入滞后被丢弃`，绝不静默 |

- **最近事件**：解析后的结构化事件表（时间 / 事件类型 / 摘要），保留最近 200 条，便于排查。

**lossy 标记**：`encoding === "lossy"` 的行整行淡橙底 + 橙色「编码损坏」徽标，
hover 提示「该行原始字节既不是合法 UTF-8 也无法按 GBK 解码，内容已被替换字符替代，不可还原」。
（`gbk` 是无损解码，不算损坏。）

**日志搜索**：工具栏搜索框，⌘F / Ctrl+F 聚焦（WebView 没有可依赖的原生页内查找）。
不区分大小写的纯子串匹配（不支持正则），命中高亮并显示「当前 / 总数」；Enter / Shift+Enter
在命中间跳转，Esc 清空；「只看匹配行」可把日志过滤成命中行。界面不替用户判断哪行是错误，
要找错误就搜 `error`。搜索范围是内存里最近 20000 行（`MAX_LOG_ROWS`）；更早的输出看会话的 JSONL 文件。

**虚拟滚动**：日志视图只渲染视口附近的行（`useVirtualRows` + `src/lib/virtualList.ts`），行高按真实测量缓存、未渲染的行用平均高度估计，窗口宽度变化时重新测量。行数多少不影响滚动与追加的流畅度。

**右键菜单**：全局屏蔽 WebView 原生右键菜单（检查元素、重新加载等），只有可编辑的文本控件保留，
方便剪切 / 复制 / 粘贴。逻辑在 `src/lib/contextMenu.ts`。

**自动滚底**：只有用户本来就在底部（容差 32px）时才自动跟随；手动上滚会暂停，
滚回底部或勾选「自动滚底」后恢复。判定逻辑是纯函数 `shouldAutoScroll`，有单测。

### 1.6 空状态与错误态

| 场景 | 表现 |
| --- | --- |
| 没有任何应用 | 主区显示引导：「app_catch 不做自动发现，应用需要手动录入」+ 新建按钮 |
| 应用无会话 | 「该应用还没有会话。点「启动」开始捕获输出。」 |
| 命令报错 | 右上角 toast，3.5 秒自动消失，可手动关闭；文案是 Rust 侧给出的中文 |
| 非 Tauri 环境 | 提示「请在 Tauri 窗口中使用（pnpm tauri dev）」而不是白屏 |
| 旧会话（重启启动器后） | 顶部黄色横幅说明「日志来自已落盘文件，无法再接管进程」 |

## 2. 命令层 API

全部命令在 `src-tauri/src/commands.rs`，注册于 `src-tauri/src/lib.rs` 的 `invoke_handler`。
所有命令失败时返回同一形状的结构化错误：

```jsonc
{ "kind": "not_found", "message": "未找到: 应用配置 ghost" }
```

`kind` 取值：`not_found` / `already_exists` / `invalid_id` / `invalid_input` / `io` /
`parse` / `serialize` / `home_unavailable` / `unsupported` / `conflict` / `internal` / `unknown`。

### 2.1 应用配置

| 命令 | 参数 | 返回 | 说明 |
| --- | --- | --- | --- |
| `list_apps` | — | `AppConfig[]` | 按 id 升序 |
| `get_app` | `id: string` | `AppConfig` | 不存在 → `not_found` |
| `create_app` | `config: AppConfigInput` | `AppConfig` | id 冲突 → `already_exists` |
| `update_app` | `config: AppConfigInput` | `AppConfig` | 不存在 → `not_found` |
| `delete_app` | `id: string` | `null` | 只删配置，不碰会话目录 |

`AppConfigInput` 字段：`id / name / command / args[] / cwd? / env{} / icon? / tags[] / hooks / launch_mode`。
`cwd` 与 `icon` 传空串等价于「未设置」，不会落盘成 `cwd = ""`。

新建表单两个入口用到的选择与识别命令：

| 命令 | 参数 | 返回 | 说明 |
| --- | --- | --- | --- |
| `pick_directory` | `defaultPath?: string` | `string \| null` | 原生「选择文件夹」对话框；取消返回 `null` |
| `detect_project` | `dir: string` | `ProjectDetection` | 识别项目类型并给出启动命令候选（只读文件） |
| `pick_application` | `defaultPath?: string` | `string \| null` | 原生「选择应用程序」对话框：macOS 只能选 `.app`（默认打开 /Applications），Windows 只能选 `.exe` / `.bat` / `.cmd`；取消返回 `null` |
| `inspect_application` | `path: string` | `ApplicationInfo` | 识别选中的应用：`command / name / is_bundle / executable`；路径不存在 → `not_found`，普通文件夹 → `unsupported` |

### 2.2 会话生命周期

| 命令 | 参数 | 返回 | 说明 |
| --- | --- | --- | --- |
| `start_app` | `id: string` | `SessionSnapshot` | 每次启动生成新 session_id，允许多实例 |
| `stop_session` | `sessionId: string` | `null` | 回收整棵进程树；已退出的会话直接成功（幂等）；未知会话 → `not_found` |
| `restart_session` | `sessionId: string, appId: string` | `SessionSnapshot` | 先停旧会话再起新会话 |
| `list_sessions` | — | `SessionSnapshot[]` | 运行中的全部 + 每个应用最近退出的 5 个，按启动先后排序。**界面用这个** |
| `list_active_sessions` | — | `SessionSnapshot[]` | 只含 `state === "running"` 的会话，按启动先后排序 |
| `get_session_status` | `sessionId: string` | `SessionSnapshot` | 未知会话 → `not_found` |

`SessionSnapshot`：

```ts
{
  session_id: string;   // uuid，也是会话目录名
  app_id: string;
  pid: number;
  state: "running" | "exited" | "crashed";
  started_at: string;   // RFC3339 UTC
  last_exit: { code: number | null; reason: "normal" | "killed" | "crashed" } | null;
  latest_status: { window_title: string | null; ports: number[] } | null;
}
```

### 2.3 会话事件

| 命令 | 参数 | 返回 |
| --- | --- | --- |
| `read_session_events` | `sessionId: string, fromLine: number, maxLines: number` | `SessionEventsPage` |

```ts
{
  events: SessionEvent[];  // 按全局行号升序
  next_line: number;       // 下一轮的游标
  has_more: boolean;       // 本轮被 maxLines 截断
  skipped_lines: number;   // 本轮无法解析的行数（不静默丢）
}
```

`maxLines` 传 0 用默认 500；硬上限 5000。

### 2.4 启动恢复

捕获参数（channel 容量、分片大小与数量、状态采集间隔等）不对界面开放，统一使用
`launcher-core` 的 `CaptureConfig::default()`；会话管理器在启动时构造一次，之后不再替换。

| 命令 | 参数 | 返回 | 说明 |
| --- | --- | --- | --- |
| `recover_sessions` | — | `BootReport` | `{ home, recovered[], error }`，启动期恢复残留会话 |
| `core_version` / `app_version` | — | `string` | 链路自检，保留兼容 |

### 2.5 图标

| 命令 | 参数 | 返回 | 说明 |
| --- | --- | --- | --- |
| `get_app_icon` | `appId: string` | `string \| null` | PNG 的 **data URL**；`null` 表示「没有图标」（不是错误，前端用默认图） |
| `default_icon` | — | `string` | 内置默认图的 data URL（编译进二进制，永远可用） |

图标以 data URL 返回，避免引入 `asset:` 协议所需的 scope 与 capabilities 配置（漏配只在运行时暴露成「图裂」）；本项目的 capabilities 保持只有 `core:default`。选择理由与前端三层缓存见 [ICONS.md](ICONS.md)。

### 2.6 启动时发生什么

`lib.rs` 的 `setup` 里执行 `AppState::bootstrap()`：`paths::ensure_layout()` 建好
`<home>/{apps,sessions,icons}`，再构造 `SessionManager`。

**会话恢复不在 setup 里**：`recover_sessions` 是异步的（要枚举进程表、可能重关联子进程），
改成由前端首屏调用，恢复慢也不会拖住窗口显示。恢复结果通过 `BootReport` 返回，
其中仍在运行、被重新接管的会话会自然出现在后续的 `list_sessions` 中；恢复时发现早已消失的旧会话不进列表（退出原因无从得知，列出来只会把应用误标成「已崩溃」）。

首次运行 **不会** 自动创建示例应用——基线要求「纯手动录入」，前端给空状态引导。

## 3. 刷新策略：推送为主，轮询兜底

后端在会话变化时推送事件（`src-tauri/src/notices.rs`），前端收到后才去取数据：

| 事件 | 载荷 | 触发时机 | 前端动作 |
| --- | --- | --- | --- |
| `sessions:changed` | `{ session_ids: string[] \| null }` | 会话启动 / 退出 / 重关联，窗口标题或端口变化 | `App.tsx` 重拉 `list_sessions`；`useSessionLog` 刷新该会话快照并读日志 |
| `session:log-appended` | 同上 | 写盘任务写完一批事件（此时数据已对读者可见） | `useSessionLog` 增量读日志 |

- 事件只说「哪些会话变了」，不带数据；`session_ids: null` 表示通知有积压丢失，所有会话都可能变了；
- 后端按 50 ms 窗口合并通知，输出很多的进程也只会让前端每秒收到约 20 次事件；
- 核心层的通知来自 `SessionManager::subscribe`（见 [RUNTIME.md](RUNTIME.md)），写盘任务只在**排空队列时**通知一次。

兜底轮询只防漏事件（以及浏览器预览里没有宿主的情况）：

| 项目 | 值 | 位置 |
| --- | --- | --- |
| 兜底间隔 | 5000 ms（有运行中会话）/ 15000 ms（全部退出） | `pollInterval`（`logModel.ts`） |
| 页面隐藏 | `document.visibilityState !== "visible"` 时跳过；重新可见时立刻补一轮 | `createPoller`（`poller.ts`） |
| 单轮最多连读 | 5 页（`MAX_PAGES_PER_TICK`），防止一次 tick 占住主线程 | `useSessionLog` |
| 单页条数 | 500（`PAGE_LINES`） | `logModel` |

调度统一交给 `createPoller`：**同一时刻最多一轮**，推送在读取途中到达时合并成「读完再补一轮」，
增量游标绝不会被两轮同时推进；每轮结束后才排下一轮（递归 setTimeout），慢请求不会堆积。

**命令的线程**：Tauri 2 里不带 `async` 的命令跑在主线程上。碰磁盘的命令都标了
`#[tauri::command(async)]` 或本身是 `async`，`read_session_events` 还放进了 `spawn_blocking`，
读日志不会让窗口拖动 / 缩放卡顿。

## 4. read_session_events 的分片合并与边写边读

### 4.1 文件顺序

```text
<home>/sessions/<session_id>/
    events.1.jsonl   轮转出的历史分片（seq 升序，越大越新）
    events.2.jsonl
    events.jsonl     当前活跃片（正在被写入）
```

读取顺序 = 分片 seq **按数字升序** → 活跃片最后。必须按数字排序：
字典序会把 `events.10.jsonl` 排到 `events.2.jsonl` 之前，事件顺序就乱了。

### 4.2 全局行号

所有文件串起来后行号从 0 连续递增，`fromLine` 就是这个全局行号，因此调用方不需要知道分片存在。
空行与不可解析行同样占一个行号（保证游标与物理行一一对应），只是不产生事件。

### 4.3 边跑边读的安全性

| 风险 | 处理 |
| --- | --- |
| 末行正在写，没有行尾换行 | **本轮不消费它**，游标停在它之前，下一轮再读 → 半行永远不会交给解析器 |
| 读到半行后还继续读后续文件 | 不继续：一旦发现不完整末行就终止本轮（后续文件内容顺序已不可信） |
| 列目录与打开之间文件被轮转/删除 | `NotFound` 按空文件处理，静默跳过，不报错 |
| 越界游标（历史被裁剪） | 返回真实行数作为 `next_line`，前端检测到「新游标 < 旧游标」时标记 `truncated` 并重新对齐 |
| 坏行 / 半行残留 | 计数进 `skipped_lines`，界面显示「有 N 行事件无法解析」，不静默丢 |

### 4.4 性能

| 优化 | 做法 |
| --- | --- |
| 已收尾分片 | 按「路径 + 文件长度」缓存行数；游标越过整片时 O(1) 跳过 |
| 活跃片 | 缓存「上次读到 EOF 的字节偏移 + 对应行号」，下一轮直接 seek 过去，不重扫全文件 |
| 缓存失效 | 活跃片缓存带**文件创建时间**校验：轮转后同名新文件创建时间不同，不会命中旧偏移 |

两级缓存只是优化，测试钉住「缓存不改变返回内容」。

## 5. 前端组件结构

```text
src/
  main.tsx                     入口（依次载入 5 个样式表）
  App.tsx                      布局骨架 + 全局状态（apps / sessions / 选中项 / toast）
  lib/
    types.ts                   DTO 类型镜像（与 src-tauri/src/dto.rs 对应）
    api.ts                     invoke 唯一出口：错误归一 + 非宿主降级
    logModel.ts                纯逻辑：事件->日志行、游标推进、自动滚底、格式化
    logModel.test.ts           纯逻辑单测
    sessions.ts                会话列表纯逻辑：默认选中、合并、状态聚合、推送事件匹配
    poller.ts                  可「踢一下」的兜底轮询器（推送触发 + 低频兜底，绝不并发）
    virtualList.ts             虚拟滚动的偏移 / 可见区间 / 定位计算
    iconModel.ts               图标会话内缓存 + 并发合并 + 失败静默降级 + 单应用强制重抓
    iconModel.test.ts          缓存单测
    styleContract.test.ts      「样式契约」测试（token / 交互态 / a11y 约束）
  hooks/
    useSessionLog.ts           会话日志 hook：推送驱动的增量读取 + 状态刷新 + 兜底轮询
    useVirtualRows.ts          变高行虚拟滚动：测量缓存 + 估计 + 宽度变化重测
  components/
    AppIcon.tsx                图标：有图用图 / 无图用默认图 / 再退回首字块
    AppList.tsx                左侧列表（状态聚合、运行中 / 崩溃小圆点）
    AppDetail.tsx              应用详情 + 会话表（运行中 + 最近退出的）
    AppForm.tsx                新建/编辑表单 + 纯函数导出（parseArgs/parseEnv/validateAppId）
    AppForm.test.ts            表单解析与校验单测
    SessionDetail.tsx          状态条 + 日志流 + 事件列表
    Toast.tsx                  顶部提示（去重 / 语义角色 / 悬停暂停）
    ui/
      Icons.tsx                内联 SVG 图标集（currentColor，无 emoji / 图标字体）
      Primitives.tsx           空态 / 提示条 / 骨架屏 / 分段控件 / TabPanel / 键值表
      Field.tsx                表单域：标签 + 控件 + 提示/错误（aria 关联）
      Dialog.tsx               模态框：焦点陷阱 + Esc + 滚动锁定 + 焦点还原
  styles/
    tokens.css                 设计 token（调色板 → 语义 → 尺度 / 动效 / 层级）
    base.css                   reset、排版基准、焦点环、滚动条、a11y 工具类
    layout.css                 应用骨架 + 响应式断点（1180/980/760/480/360）
    primitives.css             可复用基础组件样式
    apps.css                   列表 / 图标 / 详情 / 会话 / 日志
```

设计 token 分层与「组件只允许引用语义层」的约定由 `src/lib/styleContract.test.ts` 守护。

**不引入 UI 库**：界面只有列表、表单、表格、日志流四种结构，分层 CSS 即可覆盖；组件库会带来数百 KB 体积与长期维护的样式覆盖成本。
前端依赖仅 `vitest` 用于纯逻辑单测，未引入渲染测试环境。

## 6. 已知限制

1. **重启启动器后无法接管旧进程的 stdout/stderr**：管道随上一个父进程消失。
   `recover_sessions` 只能按 PID + 进程启动时间重关联进程身份、继续采集窗口标题与端口，
   但旧输出不会再产生新行。界面对这类会话会显示黄色横幅说明。
   若要读旧输出，只能看已落盘的 `events*.jsonl`。
2. **旧会话可能不在运行表里**：如果恢复判定进程已消失，`get_session_status` 会返回 `not_found`。
   界面此时保留已读到的日志并停止状态轮询，提示「无法再接管」。
3. **分片被裁剪后行号会前移**：`max_shards` 默认 5，长跑会话的旧分片会被删除，
   全局行号不再从 0 开始连续。读取器能检测到（返回更小的 `next_line`），
   界面标记「日志分片已被裁剪」，但已被删除的内容无法挽回。
4. **界面只保留最近的日志**：内存里最多 20000 行（`MAX_LOG_ROWS`），超出丢弃最旧的。
   更长的历史请直接读 JSONL 文件。
5. **捕获参数不可在界面调整**：分片大小 / 数量、状态采集间隔等统一用 `CaptureConfig::default()`。
6. **图标可能抓不到**：Python 服务类应用没有窗口，exe 又是解释器，因此会显示内置默认图——
   这是**产品决策**而不是缺陷（详见 [ICONS.md](ICONS.md)）。运行时图标要在会话启动后 1.5 秒才重抓一次。