# Windows 打包与端到端验收（M5）

本文记录 M5 的打包方式与端到端验收方法，并给出可复跑的取证步骤。
验收的判定标准是需求基线里那条硬要求：

> **只凭会话目录里的文件，agent 能完整还原「跑了什么、何时、输出什么、怎么结束的」。**

## 1. 打包

```bash
pnpm tauri build
```

产物（x64 release，0.1.0）：

| 文件 | 字节数 | 说明 |
| --- | --- | --- |
| `target/release/bundle/nsis/app_catch_0.1.0_x64-setup.exe` | 2549341 | NSIS，当前用户级安装，首选分发产物 |
| `target/release/bundle/msi/app_catch_0.1.0_x64_en-US.msi` | 3784704 | WiX MSI，企业分发用 |
| `target/release/app-catch.exe` | 11036672 | 免安装裸可执行文件 |

构建耗时：全量首次约 1m37s（含依赖编译与 WiX/NSIS 工具链下载缓存），增量重新打包约 27s。

### 为什么 `targets` 是显式清单

配置从 `"all"` 改为 `["nsis", "msi"]`：

- `"all"` 表示「当前平台所有已知打包目标」，属于隐式集合——引入新打包器或新平台时构建行为会静默变化；
- 显式列出两个目标，让「Windows 上产出什么」变成可审阅的配置事实；
- NSIS 显式固定 `installMode: currentUser`：安装不需 UAC 提权，与免安装版行为一致。

## 2. 端到端验收

### 2.0 通用前提：隔离 `APP_CATCH_HOME`

一切验收都跑在仓库内的隔离目录（已在 `.gitignore` 中忽略）：

```bash
export APP_CATCH_HOME="E:/repo/app_catch/.e2e/home-python"
```

这样既不会污染真实 `~/.app_catch`，也不会往被验收的项目目录里写任何东西。

### 2.1 Python 服务类：TrendRadar MCP Server

配置（`<home>/apps/trendradar-mcp.toml`）：

```toml
id = "trendradar-mcp"
name = "TrendRadar MCP 服务"
command = "E:/repo/TrendRadar/.venv/Scripts/python.exe"
args = ["-m", "mcp_server.server", "--transport", "http", "--host", "127.0.0.1", "--port", "3333"]
cwd = "E:/repo/TrendRadar"
tags = ["python", "mcp", "service"]

[env]
PYTHONUNBUFFERED = "1"
PYTHONDONTWRITEBYTECODE = "1"
```

两个 env 是刻意加的：`PYTHONUNBUFFERED=1` 保证日志即时可见（否则会被 Python 缓冲到退出才吐），
`PYTHONDONTWRITEBYTECODE=1` 保证不在被验收的项目目录里写 `__pycache__`。

运行：

```bash
cargo run -p launcher-core --example e2e_session -- \
    --app-id trendradar-mcp --wait-seconds 20 --expect-ports 3333
```

实测结果：71 条事件（1 start + 67 output + 2 status + 1 exit），端口 3333 被采到，
窗口标题为 `null`（服务类应用无窗口，符合预期），停止后进程树全部回收。

### 2.2 GUI 应用类：字符映射表（charmap.exe）

```toml
id = "charmap"
name = "字符映射表"
command = "C:/Windows/System32/charmap.exe"
tags = ["gui", "system"]
```

```bash
cargo run -p launcher-core --example e2e_session -- \
    --app-id charmap --wait-seconds 12 --expect-window-substr "字符"
```

实测：`status.window_title = "字符映射表"`；图标缓存 `<home>/icons/charmap.png`，256×256、48706 字节。

> 为什么不用 notepad.exe：Windows 11 的 notepad 是 Store 应用，`System32\notepad.exe` 只是个转交给
> `Microsoft.WindowsNotepad` 包的 stub，启动后 stub 立即退出（`exit code 0 / normal`），真正的窗口属于另一个进程，
> 会话里抓不到窗口标题。这是**被启动程序自身的启动器语义**导致的，不是 app_catch 的缺陷。
> 需要「带窗口、且是单进程」的对照组时，用 `charmap.exe`（老式 Win32 应用）更稳。

### 2.3 安装版闭环（关键）

安装版没有 dev 工具链，但仍然可以驱动真实 UI：WebView2 支持 Chrome DevTools Protocol，
启动安装版时注入环境变量即可开调试端口：

```bash
# 1) 静默安装到隔离目录（/D 必须放最后且不加引号）
target/release/bundle/nsis/app_catch_0.1.0_x64-setup.exe /S /D=E:\repo\app_catch\.e2e\install

# 2) 用隔离 home 启动安装版，并开 CDP 端口
export APP_CATCH_HOME="E:/repo/app_catch/.e2e/home-final"
export WEBVIEW2_ADDITIONAL_BROWSER_ARGUMENTS="--remote-debugging-port=9224"
./.e2e/install/app-catch.exe &

# 3) 在真实 DOM 上点「启动」→ 等「运行中」→ 点「停止」→ 等回落
node scripts/e2e-installed-gui.mjs --port 9224 --app "字符映射表" --hold 10
```

实测输出：

```text
CDP 已连接: http://tauri.localhost/ （app_catch）
应用列表: [{"name":"字符映射表","badge":"未运行","selected":true}]
已点击「启动」: 字符映射表
运行中状态: [{"name":"字符映射表","badge":"运行中","selected":true}]
保持 10s 后页面状态: [{"name":"字符映射表","badge":"运行中","selected":true}]
已点击「停止」
停止后页面状态: [{"name":"字符映射表","badge":"未运行","selected":true}]
GUI 闭环驱动完成
```

对应会话目录的 `events.jsonl`（安装版写入，与 dev 模式格式完全一致）：

```jsonl
{"event":"start","v":1,"ts":"2026-10-24T14:57:23.1123431Z","session_id":"460268cd-d976-4ec9-b05e-aa7aebaa86b2","app_id":"charmap","cmd":"C:/Windows/System32/charmap.exe","cwd":"E:\\repo\\app_catch","argv":["C:/Windows/System32/charmap.exe"],"pid":476852,"ppid":477064,"start_time":134373274431103044}
{"event":"status","v":1,"ts":"2026-10-24T14:57:23.1775727Z","state":"running","window_title":null,"ports":null}
{"event":"status","v":1,"ts":"2026-10-24T14:57:26.3923233Z","state":"running","window_title":"字符映射表","ports":null}
{"event":"exit","v":1,"ts":"2026-10-24T14:57:33.5323444Z","code":1,"reason":"killed"}
```

这条路径同时证明了：**安装版能读到 `APP_CATCH_HOME/apps/` 下已有的配置**、能正常启动/捕获/停止，
产物与 dev 模式一致。

安装版在**不设** `APP_CATCH_HOME` 时，也会自动创建 `~/.app_catch` 下的 `apps/`、`icons/`、`sessions/`。

## 3. 基线验收标准的自证

`scripts/session-replay.mjs` 只接受一个会话目录路径，**除该目录下的 `events*.jsonl` 外不读任何东西**：
不读 `apps/<id>.toml`、不查进程表、不连任何服务。它打印出来的，就是「只凭会话文件能知道的一切」。

```bash
node scripts/session-replay.mjs <home>/sessions/<session_id>
```

输出分五段，正好对应基线要求的四个问题加一段运行期状态：

| 段 | 内容 | 数据来源 |
| --- | --- | --- |
| 【1】跑了什么 | app_id、完整命令行、argv、工作目录、pid/ppid、start_time | `start` 事件 |
| 【2】何时 | 启动/结束时间（RFC3339 + 可读 UTC）、运行时长 | `start.ts` / `exit.ts` |
| 【3】输出什么 | 行数、首行、末行、前 5 行采样 | `output` 事件 |
| 【4】运行期状态 | 监听端口、窗口标题、状态序列 | `status` 事件 |
| 【5】怎么结束的 | 退出码、退出原因、结束时间 | `exit` 事件 |

### 一个必须注意的精度陷阱

`start_time` 是 Windows FILETIME（`u64`，100ns 粒度），典型值 `134373274431103044` 已经超过
JavaScript 的 `Number.MAX_SAFE_INTEGER`（2^53-1 = 9007199254740991）。
直接 `JSON.parse` 再 `JSON.stringify` 会把末位改掉（实测 `…441` 变成 `…440`），
而它是判断「同一 pid 是否已被复用」的唯一依据——精度丢了，重关联就会误判。

`session-replay.mjs` 因此从原始行里用正则把 `start_time` 抠出来保留为字符串。
任何按 `start_time` 做身份判定的工具都必须遵守同一原则。

## 4. 复跑清单

```bash
# 回归
cargo test --workspace
cargo check --workspace
pnpm build
pnpm test

# 打包
pnpm tauri build

# 验收（三个场景，见上文）
cargo run -p launcher-core --example e2e_session -- --app-id trendradar-mcp --wait-seconds 20 --expect-ports 3333
cargo run -p launcher-core --example e2e_session -- --app-id charmap --wait-seconds 12 --expect-window-substr 字符
node scripts/e2e-installed-gui.mjs --port 9224 --app "字符映射表" --hold 10

# 复原
node scripts/session-replay.mjs <session_dir>
```

## 5. 已知限制

1. **安装版 CDP 需要环境变量**：`WEBVIEW2_ADDITIONAL_BROWSER_ARGUMENTS` 是 WebView2 运行时的开关，
   不是产品功能。它只用于自动化验收，正常使用时不需要设置，也不会默认开启（默认不开调试端口）。
2. **MSI 未做静默安装验证**：本机未安装 WiX 之外的企业分发依赖，MSI 只验证了产物生成与体积；
   NSIS 走了完整的静默安装 → 启动 → 驱动 → 停止链路。
3. **Store 应用型 exe 的窗口抓取**：如 notepad.exe 这类「Stub 转发到包」的程序，
   启动器进程会先退出，窗口属于另一个进程，会话里采不到窗口标题（详见 2.2 的说明）。
4. **图标抓取依赖桌面会话**：无交互桌面（如纯服务/CI）时窗口图标抓不到，会回落到 exe 资源图标或内置默认图。
