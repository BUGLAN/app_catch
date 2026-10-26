# GUI（M2）：界面结构、命令层 API 与轮询策略

M2 把 app_catch 从「M0 状态页」变成可用的桌面启动器：左侧应用列表、右侧应用详情与会话详情，
支持新建/编辑/删除应用、启动/停止/重启会话，并把会话输出以实时日志流的形式呈现。

> 前置阅读：[docs/RUNTIME.md](RUNTIME.md)（会话生命周期）、[docs/SESSION_FORMAT.md](SESSION_FORMAT.md)（JSONL 事件格式）、[docs/CONFIG.md](CONFIG.md)（应用配置 TOML）。
>
> 视觉规范与设计 token 见 [docs/UI-AUDIT.md](UI-AUDIT.md)（含完整走查清单、修复记录与实测验收数据）。

## 0. 视觉与交互约定（摘要）

- **设计 token**：颜色/间距/圆角/字号/阴影/动效全部定义在 `src/styles/tokens.css`，组件样式只引用语义层，
  不允许出现裸色值（由 `styleContract.test.ts` 守护）。
- **三态齐全**：所有可交互控件都有 hover / active / disabled，焦点靠全局 `:focus-visible` 统一提供 2px 高对比环。
- **状态不靠颜色单独编码**：徽标同时用形状（实心点 / 方块 / 菱形）+ 文本区分。
- **响应式**：1180 / 980 / 760 / 480 / 360 五档；760 以下侧栏转为图标条并保持单列主区。
  任意宽度下都不出现文档级横向滚动。
- **窗口外壳**：Windows/Linux 无边框 + 前端自绘标题栏；macOS 保留红黄绿（`titleBarStyle: Overlay`）。
  详见 [UI-SHELL.md](UI-SHELL.md)。
- **对比度**：正文 ≥ 4.5:1，控件边界 ≥ 3:1（实测值见 UI-AUDIT.md）。
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
│ │ 标签     │ │ 活跃会话表：会话 / PID / 状态 / 启动时间 / 退出            │
│ │ ▫状态    │ │ ───────────────────────────────────────────────────────── │
│ └──────────┘ │ 会话详情                                                  │
│  (56–320px)  │  状态条：会话id PID 状态 启动时间 窗口标题 端口 退出        │
│              │  [实时日志][最近事件]        □自动滚底 [清空视图]           │
│              │  ┌────────────────────────────────────────────────────┐   │
│              │  │ 18:30:01.123 OUT 服务已启动                        │   │
│              │  │ 18:30:02.456 ERR warn: ...                         │   │
│              │  └────────────────────────────────────────────────────┘   │
├──────────────┤                                                           │
│ 主题   跟随系统│                                                          │
│ 设置         │                                                           │
│ ● 已连接宿主 │                                                           │
└──────────────┴───────────────────────────────────────────────────────────┘
```

抽屉可折叠（56px 图标态，名称走 tooltip）、可拖拽右边缘调宽（56–320，默认 220），
宽度与折叠状态持久化在 `localStorage`。详见 [UI-SHELL.md](UI-SHELL.md)。

### 1.1 应用列表项

| 元素 | 说明 |
| --- | --- |
| 图标 | **M3 起是真实图标**（30×30）：由 `AppIcon` 组件渲染，数据来自 `get_app_icon` 命令的 data URL；抓不到时用内置默认图；连默认图都取不到（非 Tauri 环境）才退回名称首字块（`initialOf`）。详见 [docs/ICONS.md](ICONS.md) |
| 名称 | 超长省略，hover 显示完整名称 |
| 标签 | 最多显示 3 个；无标签显示虚线「无标签」 |
| 状态徽标 | `未运行` / `运行中`（多实例显示 ×N）/ `已退出` / `已崩溃`，由该应用的活跃会话聚合得出 |
| 「启动」按钮 | 默认隐藏（`opacity:0` + `pointer-events:none`，避免误触）；hover 或键盘聚焦时**原地替换**状态徽标，不改变行宽；点击不切换选中项（`stopPropagation`）。触屏（`hover: none`）下常显 |

状态聚合规则（`aggregateState`）：有 running 会话 → 运行中；否则有 crashed → 已崩溃；
否则有任何会话 → 已退出；否则 → 未运行。

### 1.2 应用详情

展示 `command + args` 拼接后的完整命令行（`renderCommand`，含空格的参数自动加引号）、工作目录、
环境变量（`KEY=VALUE` 逐行）、标签、自定义图标路径。

操作按钮：

| 按钮 | 行为 |
| --- | --- |
| 启动 | 调 `start_app`，立刻把返回的快照插入会话表并选中 |
| 停止 | 停「最近启动的运行中会话」；无运行中会话时禁用 |
| 重启 | 对选中会话（无选中则取第一个）调 `restart_session` → 新 session_id |
| 编辑 / 删除 | 打开表单 / 二次确认弹窗 |

**多实例**：同一应用可同时有多个会话。会话表逐行列出，每行都有独立的「停止」按钮；
点击行切换下方会话详情。

### 1.3 新建 / 编辑表单

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
  | `output` | `output` 事件 | 等宽字体；stdout 常规色、stderr 红色；`ERR`/`OUT` 小标签；带时间戳 |
  | `divider` | `start` / `status` / `exit` 事件 | 左侧竖线的分隔行，**不与进程输出混在一起** |
  | `notice` | `overflow` 事件 | 橙色提示条：`有 N 行输出因写入滞后被丢弃`，绝不静默 |

- **最近事件**：解析后的结构化事件表（时间 / 事件类型 / 摘要），保留最近 200 条，便于排查。

**lossy 标记**：`encoding === "lossy"` 的行整行淡橙底 + 橙色「编码损坏」徽标，
hover 提示「该行原始字节既不是合法 UTF-8 也无法按 GBK 解码，内容已被替换字符替代，不可还原」。
（`gbk` 是无损解码，不算损坏。）

**自动滚底**：只有用户本来就在底部（容差 32px）时才自动跟随；手动上滚会暂停，
滚回底部或勾选「自动滚底」后恢复。判定逻辑是纯函数 `shouldAutoScroll`，有单测。

### 1.6 空状态与错误态

| 场景 | 表现 |
| --- | --- |
| 没有任何应用 | 主区显示引导：「app_catch 不做自动发现，应用需要手动录入」+ 新建按钮 |
| 应用无会话 | 「该应用当前没有运行中的会话。」 |
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

`AppConfigInput` 字段：`id / name / command / args[] / cwd? / env{} / icon? / tags[]`。
`cwd` 与 `icon` 传空串等价于「未设置」，不会落盘成 `cwd = ""`。

### 2.2 会话生命周期

| 命令 | 参数 | 返回 | 说明 |
| --- | --- | --- | --- |
| `start_app` | `id: string` | `SessionSnapshot` | 每次启动生成新 session_id，允许多实例 |
| `stop_session` | `sessionId: string` | `null` | 回收整棵进程树；未知会话 → `not_found` |
| `restart_session` | `sessionId: string, appId: string` | `SessionSnapshot` | 先停旧会话再起新会话 |
| `list_active_sessions` | — | `SessionSnapshot[]` | 只含 `state === "running"` 的会话 |
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

### 2.4 捕获配置与启动恢复

| 命令 | 参数 | 返回 | 说明 |
| --- | --- | --- | --- |
| `get_capture_config` | — | `CaptureConfig` | 时长字段以毫秒为单位 |
| `set_capture_config` | `config: CaptureConfig` | `CaptureConfig` | **有会话在跑时返回 `conflict`**：重建管理器会丢掉会话表 |
| `recover_sessions` | — | `BootReport` | `{ home, recovered[], error }`，启动期恢复残留会话 |
| `core_version` / `app_version` | — | `string` | M0 链路自检，保留兼容 |

### 2.5 图标（M3）

| 命令 | 参数 | 返回 | 说明 |
| --- | --- | --- | --- |
| `get_app_icon` | `appId: string` | `string \| null` | PNG 的 **data URL**；`null` 表示「没有图标」（不是错误，前端用默认图） |
| `default_icon` | — | `string` | 内置默认图的 data URL（编译进二进制，永远可用） |

**为什么返回 data URL 而不是路径**：图标只有几十 KB，base64 之后仍远小于一次 IPC 的合理开销；
而走 Tauri 的 `asset:` 协议需要在 `tauri.conf.json` 配 `assetProtocol.scope`、
在 `capabilities/default.json` 加 `core:asset:default`，任一处漏配都只在运行时暴露成「图裂」。
因此 **M3 没有改动 capabilities**，仍是 `core:default`。选择理由与前端三层缓存见 [docs/ICONS.md](ICONS.md)。

### 2.6 启动时发生什么

`lib.rs` 的 `setup` 里执行 `AppState::bootstrap()`：`paths::ensure_layout()` 建好
`<home>/{apps,sessions,icons}`，再构造 `SessionManager`。

**会话恢复不在 setup 里**：`recover_sessions` 是异步的（要枚举进程表、可能重关联子进程），
改成由前端首屏调用，恢复慢也不会拖住窗口显示。恢复结果通过 `BootReport` 返回，
其中 `recovered` 里的会话会自然出现在后续的 `list_active_sessions` 中。

首次运行 **不会** 自动创建示例应用——基线要求「纯手动录入」，前端给空状态引导。

## 3. 轮询策略

| 项目 | 值 | 位置 |
| --- | --- | --- |
| 日志增量 | 1000 ms（有运行中会话）/ 3000 ms（全部退出） | `useSessionLog` + `pollInterval` |
| 会话列表 | 1000 ms / 3000 ms | `App.tsx` |
| 页面隐藏 | `document.visibilityState !== "visible"` 时完全暂停（不读日志也不查状态） | 两处轮询都有判断 |
| 单轮最多连读 | 5 页（`MAX_PAGES_PER_TICK`），防止一次 tick 占住主线程 | `useSessionLog` |
| 单页条数 | 500（`PAGE_LINES`） | `logModel` |

调度用 **递归 setTimeout** 而不是 setInterval：一次请求可能比间隔更慢，
setInterval 会堆积请求；递归调度保证「上一轮结束才开始下一轮」。

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

两级缓存都只是优化：`shard_cache_does_not_change_results` 与 `rotation_resets_active_cursor`
两条测试专门钉住「缓存不改变返回内容」。

### 4.5 测试覆盖

- 单元（`src-tauri/src/events_reader.rs`）：数字序分片、跨片翻页、半行不消费、
  活跃片偏移缓存、轮转重置、坏行计数、空行占号、CRLF、越界游标、缺失目录。
- 集成（`src-tauri/tests/session_events.rs`）：**真实进程** 跑出真实分片
  （把 `max_file_bytes` 压到 200 字节触发多次轮转），用 3 条/页的游标读完全量，
  再与「按文件顺序直读」的第二套独立实现逐条比对，并断言 output 序号严格递增、
  首尾事件是 `start` / `exit`。

## 5. 前端组件结构

```text
src/
  main.tsx                     入口（依次载入 5 个样式表）
  App.tsx                      布局骨架 + 全局状态（apps / sessions / 选中项 / toast）
  lib/
    types.ts                   DTO 类型镜像（与 src-tauri/src/dto.rs 对应）
    api.ts                     invoke 唯一出口：错误归一 + 非宿主降级
    logModel.ts                纯逻辑：事件->日志行、游标推进、自动滚底、格式化
    logModel.test.ts           45 条单测
    iconModel.ts               图标会话内缓存 + 并发合并 + 失败静默降级
    iconModel.test.ts          11 条单测
    styleContract.test.ts      19 条「样式契约」测试（token / 交互态 / a11y 约束）
  hooks/
    useSessionLog.ts           轮询 hook：增量游标 + 状态刷新 + 隐藏暂停
  components/
    AppIcon.tsx                图标：有图用图 / 无图用默认图 / 再退回首字块
    AppList.tsx                左侧列表（状态聚合、hover 启动；选择按钮与启动按钮同级）
    AppDetail.tsx              应用详情 + 活跃会话表
    AppForm.tsx                新建/编辑表单 + 纯函数导出（parseArgs/parseEnv/validateAppId）
    AppForm.test.ts            15 条单测
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

设计 token 分层、token 命名与「组件只允许引用语义层」的约定见 [UI-AUDIT.md](UI-AUDIT.md)。

**不引入 UI 库的理由**：界面只有列表、表单、表格、日志流四种结构，分层 CSS 约 33KB（gzip 6.5KB）就能覆盖；
引入组件库（如 MUI/AntD）会带来数百 KB 与一套需要长期跟随的样式覆盖，对「信息密度高的开发者工具」不划算。

**样式约束由测试守护**：`src/lib/styleContract.test.ts` 会检查「除 tokens.css 外不得出现裸色值」「字号/圆角必须走 token」
「所有 `var(--x)` 必须有定义」「`!important` 只能出现在 reduced-motion 块」以及若干 a11y 结构约束，
避免后续迭代把这些约定改回去。

**新增前端依赖**：仅 `vitest`（devDependency，150 包中实际新增 16 个包），用于纯逻辑单测；
没有引入 jsdom / @testing-library（那会再加约 4MB），因此当前不测组件渲染。

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
4. **修改捕获配置需要先停掉所有会话**：管理器重建会丢弃内存会话表。
   这是刻意选择——否则那些进程会既停不掉也查不到。
5. **无虚拟滚动**：日志最多保留 4000 行（`MAX_LOG_ROWS`），超出丢弃最旧的。
   更长的历史请用「最近事件」或直接读 JSONL 文件。
6. **图标可能抓不到**：Python 服务类应用没有窗口，exe 又是解释器，因此会显示内置默认图——
   这是**产品决策**而不是缺陷（详见 [docs/ICONS.md](ICONS.md)）。运行时图标要在会话启动后 1.5 秒才重抓一次。
7. **组件渲染级测试暂缺**：当前 vitest 只跑纯逻辑（node 环境），渲染正确性靠手动验收清单覆盖。

## 7. 手动验收步骤

见 [README「如何运行 GUI」](../README.md#如何运行-gui) 与本文 §1 的界面结构。