# app_catch

开发者向的桌面应用启动器：把 E:\repo 下的自开发应用收进可管理列表，一键启动，并把每个会话的进程信息、运行状态与 stdout/stderr 全量捕获为结构化 JSONL，供 AI agent 直接读取。

> 当前进度：**M3 完成，图标可用**。配置管理、路径布局、会话事件模型、Windows 进程后端（Job Object 杀树）、输出捕获与 JSONL 轮转、会话管理器与重启重关联、窗口标题与监听端口采集、完整 GUI（应用列表、启动/停止/重启、会话详情实时日志流）均已落地；M3 在此之上把图标从占位块换成真实图标：Windows 从**运行中窗口**与 **exe 资源**两路抓取，统一规范化成 PNG 缓存到 `~/.app_catch/icons/`，抓不到用内置默认图兜底（**抓不到不算失败**）。
>
> 输出捕获按「严格 UTF-8 → Windows GBK(cp936) 回退 → lossy 标记」逐行解码，GBK 中文日志无损保真；每行带 `encoding` 标记，详见 [docs/SESSION_FORMAT.md](docs/SESSION_FORMAT.md)。
>
> GUI 的界面结构、命令层 API 与轮询策略见 [docs/GUI.md](docs/GUI.md)；图标的优先级链、缓存失效规则与兜底语义见 [docs/ICONS.md](docs/ICONS.md)。

## 目录架构

| 路径 | 说明 |
| --- | --- |
| `crates/launcher-core/` | 纯 Rust 核心库：配置管理、进程生命周期、会话 JSONL、图标捕获。**不依赖 tauri 或任何 UI 库**，可在无窗口环境下独立测试。 |
| `src-tauri/` | Tauri 2 壳层：`src/lib.rs` 注册命令并初始化状态；`src/commands.rs` 是命令薄封装（业务一律委托 launcher-core）；`src/events_reader.rs` 做会话事件的分片合并与增量读取。 |
| `src/` | 前端（React + TypeScript + Vite）：两栏布局（应用列表 + 详情/会话），深色高密度主题，纯 CSS 无 UI 库。 |
| `docs/REQUIREMENTS.md` | 需求与决策基线（与仓库根 `PROMPT.md` 内容完全一致）。 |
| `docs/CONFIG.md` | 应用配置 TOML 格式：字段表、默认值、存储位置、错误语义。 |
| `docs/SESSION_FORMAT.md` | 会话 JSONL 事件格式：五类事件字段表、示例行、版本演进策略、轮转分片与溢出/重关联语义。 |
| `docs/RUNTIME.md` | 会话运行时：生命周期状态机、`SessionManager` API、退出分类、捕获/轮转策略、Windows 后端实现与已知限制。 |
| `docs/GUI.md` | **M2**：界面结构、命令层 API 一览、轮询策略、分片读取与边写边读的安全性、已知限制。 |
| `docs/ICONS.md` | **M3**：图标优先级链、PNG 规范、缓存位置与失效规则、兜底语义、为什么用 data URL 与 `png` crate、已知限制。 |
| `docs/E2E.md` | **M5**：Windows 打包说明、真实项目端到端验收流程、CDP 驱动安装版 GUI 的方法、会话目录复原工具。 |
| `scripts/` | `e2e-installed-gui.mjs`（CDP 驱动安装版 GUI）、`session-replay.mjs`（只读会话目录复原全过程）与 `merge-updater-manifests.mjs`（CI 里把各平台签名合并成 `latest.json`）。 |
| `.github/workflows/` | `ci.yml`（push/PR 构建校验）与 `release.yml`（tag 触发：矩阵打包 + GitHub Release + 更新清单）。 |
| `Cargo.toml` | cargo workspace 根，成员为 `crates/launcher-core` 与 `src-tauri`，公共依赖集中在 `[workspace.dependencies]`。 |

## 前置依赖

- **Rust** stable（`cargo` / `rustc`，已验证 1.99.1）
- **Node.js** 20+ 与 **pnpm**（已验证 node 22.19.0 / pnpm 11.21.0）
- **Windows**：MSVC 生成工具（VS 2022 Build Tools 的 VC.Tools.x86.x64）与 WebView2 运行时
- Tauri CLI v2 已作为前端 devDependency 安装，无需全局安装

## 构建与开发命令

```bash
# 安装前端依赖（首次必做）
pnpm install

# 开发模式：启动 Vite(15173) 并拉起 Tauri 窗口
pnpm tauri dev

# 仅构建前端产物（tsc 类型检查 + vite build，输出到 dist/）
pnpm build

# 前端纯逻辑单测（vitest）
pnpm test

# Rust 侧检查与测试（含 launcher-core 单元测试与命令层集成测试）
cargo check --workspace
cargo test --workspace

# 打包安装包
pnpm tauri build
```

> `pnpm tauri dev` / `pnpm tauri build` 依赖 `dist/` 与 Vite 端口 15173（`vite.config.ts` 中 `strictPort` 已锁定，避免端口漂移导致白屏）。

## 打包（Windows）

### 命令

```bash
# 一条命令完成：pnpm build（tsc + vite）-> cargo build --release -> 生成安装包
pnpm tauri build
```

### 产物路径

| 目标 | 路径 | 说明 |
| --- | --- | --- |
| NSIS 安装包 | `target/release/bundle/nsis/app_catch_0.1.0_x64-setup.exe` | **首选产物**，当前用户级安装，无需管理员权限 |
| MSI 安装包 | `target/release/bundle/msi/app_catch_0.1.0_x64_en-US.msi` | WiX 产物，适合企业分发/GPO |
| 裸可执行文件 | `target/release/app-catch.exe` | 免安装运行，同样读取 `~/.app_catch` |

实测体积（0.1.0，x64 release）：NSIS ≈ 2.43 MiB、MSI ≈ 3.61 MiB、裸 exe ≈ 10.5 MiB。

### NSIS 与 MSI 的差别

| 维度 | NSIS（.exe） | MSI（.msi） |
| --- | --- | --- |
| 依赖 | NSIS 3.13（由 Tauri CLI 自动下载缓存，无需手动安装） | WiX 3.14（同样自动下载缓存） |
| 安装范围 | 默认 `currentUser`（本配置显式固定），装到 `%LOCALAPPDATA%\app_catch` | 默认 perMachine，需管理员 |
| 静默安装 | `app_catch_0.1.0_x64-setup.exe /S /D=<目录>`（`/D` 必须最后且不加引号） | `msiexec /i app_catch_0.1.0_x64_en-US.msi /qn INSTALLDIR=<目录>` |
| 典型用途 | 面向个人用户的分发 | 企业软件分发与卸载登记 |

打包配置（`src-tauri/tauri.conf.json`）：

```json
"bundle": {
  "active": true,
  "targets": ["nsis", "msi"],
  "windows": { "nsis": { "installMode": "currentUser" } }
}
```

为什么把 `targets` 从 `"all"` 改成显式清单：`"all"` 会尝试当前平台上**所有**已知目标，一旦将来引入新平台或新打包器，构建会静默多出（或失败于）预期之外的产物；显式列出 `nsis`+`msi` 让「Windows 上产出什么」成为可审阅的配置事实。NSIS 固定 `currentUser` 则是为了避免安装时触发 UAC 提权——安装版与免安装版的行为应一致。

### 打包后的数据位置

安装版与 dev 模式**共用同一套路径规则**：默认 `~/.app_catch`（即 `%USERPROFILE%\.app_catch`），首次启动自动创建 `apps/`、`sessions/`、`icons/`。要换位置，启动前设置 `APP_CATCH_HOME` 即可；dev 模式写的配置，安装版能直接读到，反之亦然。

### 安装包验证（无需 GUI 的快速自查）

```bash
# 静默安装到隔离目录（避免污染真实安装位置）
target/release/bundle/nsis/app_catch_0.1.0_x64-setup.exe /S /D=E:\tmp\app_catch_install

# 用隔离的 APP_CATCH_HOME 启动，确认窗口正常且目录布局被创建
set APP_CATCH_HOME=E:\tmp\app_catch_home
E:\tmp\app_catch_install\app-catch.exe
```

预期：出现 1024×720 的 `app_catch` 窗口；`%APP_CATCH_HOME%` 下出现 `apps\`、`icons\`、`sessions\` 三个目录。

完整的端到端验收流程（含用真实项目跑通、以及用 CDP 驱动安装版 GUI 点「启动/停止」）见 [docs/E2E.md](docs/E2E.md)。

## 如何运行 GUI

### 步骤

1. **首次准备**：安装 Rust stable、Node 20+、pnpm，Windows 上还需 MSVC 生成工具与 WebView2 运行时。
2. **安装前端依赖**：

   ```bash
   cd E:\repo\app_catch
   pnpm install
   ```

3. **启动开发窗口**：

   ```bash
   pnpm tauri dev
   ```

   该命令会先跑 Vite（http://localhost:15173），再编译 Rust 并拉起 Tauri 窗口（1024×720，深色主题）。
   首次编译 Rust 依赖需要几分钟，之后是增量编译。

4. **数据位置**：应用配置与会话日志默认写在 `~/.app_catch`；要换位置，启动前设置环境变量：

   ```powershell
   $env:APP_CATCH_HOME = 'E:\repo\app_catch-data'
   pnpm tauri dev
   ```

   ```bash
   # Git Bash / WSL
   APP_CATCH_HOME=/e/repo/app_catch-data pnpm tauri dev
   ```

5. **首次打开是空的**：app_catch 不做自动发现，需要手动录入。点右上角「+ 新建应用」。

### 手动验收步骤（1024×720 窗口）

| # | 操作 | 预期 |
| --- | --- | --- |
| 1 | 启动后看主区 | 显示「还没有任何应用 / app_catch 不做自动发现，应用需要手动录入」+ 新建按钮 |
| 2 | 点「+ 新建应用」，id 输入 `bad id` | id 下方实时红字：`id 只允许 A-Z a-z 0-9 _ -`，保存按钮不可点 |
| 3 | id 改 `demo`，名称 `演示`，命令 `cmd`，参数框填两行：`/c` 与 `echo hello & ping 127.0.0.1 -n 6 > nul`，保存 | toast「已创建应用「演示」」，左侧列表出现带首字头像的条目，右侧显示拼接后的命令行 |
| 4 | 点「启动」 | 会话表出现一行：状态徽标变「运行中」；下方会话详情状态条显示会话 id、PID、启动时间；日志流出现分隔行「会话开始」与 `hello` 输出行 |
| 5 | 观察 3~5 秒 | 日志窗口出现分隔行「状态 运行中」；约 6 秒后进程退出，出现分隔行「进程已退出 · 退出码 0 · 正常退出」 |
| 6 | 点「最近事件」 | 表格列出 start / output / status / exit 的结构化字段，时间列为 `HH:MM:SS.mmm` |
| 7 | 切回「实时日志」，在日志区上滚 | 「自动滚底」复选框自动取消勾选；滚回底部后重新勾上 |
| 8 | 再点一次「启动」（多实例） | 会话表出现第二行；两行各自有「停止」按钮 |
| 9 | 对一个运行中的会话点「停止」 | 该会话状态变「已退出」，日志末尾出现「退出码 … · 被停止」 |
| 10 | 点「编辑」，改名称为「演示2」，保存 | 列表与详情标题同步更新 |
| 11 | 点「删除」 | 弹出二次确认，说明只删配置不删会话目录；确认后列表移除，toast 提示已删除 |
| 12 | 重新点「启动」后关掉窗口，再跑一次 `pnpm tauri dev` | 首屏调用 `recover_sessions`；残留会话出现在会话表中，日志从已落盘文件继续显示 |
| 13 | 新建一个命令为 `notepad` 的应用 | 列表与详情页显示 **notepad 的真图标**（不是名称首字） |
| 14 | 新建一个命令为 `python` 且没有窗口的应用 | 显示**内置默认图标**（深色圆角 + 四宫格），不弹任何错误 |
| 15 | 检查 `~/.app_catch/icons/` | 出现 `<app_id>.png`；删掉它再刷新列表会自动重建 |

### 验证 lossy 与 overflow 的显式提示（可选）

这两类事件平时不常见，可直接构造会话目录验证渲染：在 `~/.app_catch/sessions/<uuid>/events.jsonl` 里写入
带 `"encoding":"lossy"` 的 output 事件与一条 `overflow` 事件（字段见 [docs/SESSION_FORMAT.md](docs/SESSION_FORMAT.md)），
预期：`lossy` 行淡橙底 + 「编码损坏」徽标；overflow 渲染成橙色提示条「有 N 行输出因写入滞后被丢弃」；stderr 行红色。

## 里程碑进度

| 里程碑 | 内容 | 状态 |
| --- | --- | --- |
| M0 | 仓库初始化：cargo workspace + Tauri 2 脚手架，cargo check/test 全绿，需求基线入库，README 写清构建方式 | ✅ 已完成 |
| M1 | launcher-core（Windows 先行）：F1 配置、F2 启动、F3 捕获 + JSONL + 轮转、F4 停止与进程树回收、窗口标题与监听端口采集 | ✅ 已完成 |
| M2 | GUI：应用列表（图标/名称/状态）、启动/停止/重启、会话详情（实时日志流、状态、退出信息） | ✅ 已完成 |
| M3 | 图标：Windows 抓取 + 缓存 + 兜底 | ✅ 已完成 |
| M4 | macOS 对齐：构建通过、进程组语义、图标抓取 | ⏳ 待做 |
| M5 | 打包与端到端验收：NSIS/MSI 安装包产出，用真实项目（Python 服务 + GUI 应用）走完整流程，并用安装版验证闭环 | ✅ 已完成 |
| M6 | 界面外壳：自绘标题栏、可折叠抽屉、双主题、全局搜索与视觉重构 | ✅ 已完成 |
| M7 | 发布与自动更新：GitHub Actions CI/Release、设置页版本展示、tauri-plugin-updater 全流程 | ✅ 已完成 |

## launcher-core 模块一览

| 模块 | 内容 | 文档 |
| --- | --- | --- |
| `config` | `AppId` / `AppConfig` / `AppStore`：每应用一个 TOML，原子写盘 | [docs/CONFIG.md](docs/CONFIG.md) |
| `paths` | `APP_CATCH_HOME` 优先的目录布局（apps / sessions / icons） | [docs/CONFIG.md](docs/CONFIG.md) |
| `session` | `SessionId` / `SessionEvent`：自描述 v=1 的 JSONL 事件模型（`output` 事件带 `encoding` 标记） | [docs/SESSION_FORMAT.md](docs/SESSION_FORMAT.md) |
| `platform` | `ProcessBackend` / `IconCapture` trait；Windows 后端用 Job Object 实现杀树与 PID 复用判定 | [docs/RUNTIME.md](docs/RUNTIME.md) |
| `platform::icon` | 图标核心（跨平台）：RGBA 规范化、PNG 编解码、缓存三态判定、`IconSource` 抽象与 `resolve_and_cache` | [docs/ICONS.md](docs/ICONS.md) |
| `platform::icon_windows` | Windows 图标抓取：窗口 WM_GETICON / 类图标、exe 资源（PrivateExtractIconsW 等），RAII 句柄 | [docs/ICONS.md](docs/ICONS.md) |
| `runtime` | `SessionManager` 门面、输出捕获管线、JSONL 写入与轮转 | [docs/RUNTIME.md](docs/RUNTIME.md) |
| `error` / `fsutil` | 统一错误类型 `AppCatchError` 与原子写盘工具 | 见源码 rustdoc |

## src-tauri 模块一览

| 模块 | 内容 |
| --- | --- |
| `lib.rs` | `setup` 里初始化共享状态并注册全部命令；不含业务逻辑 |
| `commands.rs` | 命令薄封装：解析入参 → 调 launcher-core → 转 DTO，错误统一为中文 |
| `app_state.rs` | `AppStore` / `Arc<SessionManager>` / `CaptureConfig` / 事件读取器的托管 |
| `dto.rs` | 命令层 DTO 与 `AppConfigInput` 校验（宽进严出，中文提示） |
| `error.rs` | `AppCatchError` → `{ kind, message }`，8 个变体映射到稳定分类 |
| `events_reader.rs` | 会话事件分片合并、增量游标、边写边读安全、两级缓存 |

命令层 API 一览见 [docs/GUI.md](docs/GUI.md) 的「命令层 API」一节。

### 手动验收示例

```bash
# 默认执行 cmd /c echo hello，打印会话目录、事件数与 JSONL 尾部
cargo run -p launcher-core --example session_demo

# 指定命令
cargo run -p launcher-core --example session_demo -- cmd /c "echo A&echo B&exit /b 3"
```

示例使用独立的临时 `APP_CATCH_HOME`，不会污染真实的 `~/.app_catch`。

### 真实项目端到端验收

```bash
# 1) 起一个真实应用（配置放在隔离的 APP_CATCH_HOME 下）
export APP_CATCH_HOME="E:/repo/app_catch/.e2e/home-python"
cargo run -p launcher-core --example e2e_session -- \
    --app-id trendradar-mcp --wait-seconds 20 --expect-ports 3333

# 2) 只凭会话目录复原全过程（不读配置、不读进程表）
node scripts/session-replay.mjs "$APP_CATCH_HOME/sessions/<session_id>"
```

完整流程与验收结论见 [docs/E2E.md](docs/E2E.md)。

> 数据隔离：所有测试通过环境变量 `APP_CATCH_HOME` 指向 `tempfile` 临时目录，**绝不触碰真实的 `~/.app_catch`**。

## 发布与自动更新

### 方案

- 桌面端更新采用 Tauri 官方 [`tauri-plugin-updater`](https://v2.tauri.app/plugin/updater/)（v2）配 `tauri-plugin-process`（安装后 relaunch）：
  - `src-tauri/tauri.conf.json` 里 `bundle.createUpdaterArtifacts: true`，打包时额外产出更新包与 `.sig` minisign 签名（Windows 复用 NSIS 的 `-setup.exe`，macOS 用 `.app.tar.gz`）；
  - `plugins.updater` 配置公钥与更新清单地址 `https://github.com/BUGLAN/app_catch/releases/latest/download/latest.json`，与 CI 发布的资产同源；
  - 前端封装在 `src/lib/updater.ts`（支持判定、静默检查、错误归一），界面在设置页「关于与更新」：版本号、检查更新、下载进度、待重启、失败原因、自动检查开关。
- 版本号单一来源：运行时「关于」页经 `getVersion()` 读 `tauri.conf.json` 的版本（与安装包同名）；`package.json` / `tauri.conf.json` / workspace `Cargo.toml` 三方一致由 `src/lib/versionContract.test.ts` 在 CI 里守护。

### CI 与发布流程

| 工作流 | 触发 | 做什么 |
| --- | --- | --- |
| `.github/workflows/ci.yml` | push（任意分支）/ PR | Windows + macOS 各自跑 `pnpm test`、`cargo test --workspace --locked`、完整 `tauri build`（用 `--config` 关闭更新签名件，无需私钥）；产物留 workflow artifact |
| `.github/workflows/release.yml` | 推送 `v*.*.*` tag | macOS aarch64/x86_64 + Windows x64 矩阵打包并签名更新产物 → 汇总创建 GitHub Release（名称与正文取自 tag 说明）→ 上传安装包与合并后的 `latest.json` |

发版步骤：把三处版本号一起改（测试会拦下不一致）→ 提交 → `git tag -a v0.x.y -m "发布说明"` → `git push origin v0.x.y`。

### 需要配置的 secrets（GitHub 仓库 Settings → Secrets and variables → Actions）

| Secret | 内容 | 来源 |
| --- | --- | --- |
| `TAURI_SIGNING_PRIVATE_KEY` | 更新签名私钥的**内容**（文本，不是路径） | 本机 `~/.app_catch-updater/app_catch.key`（2026-10-26 生成，**未入库**） |
| `TAURI_SIGNING_PRIVATE_KEY_PASSWORD` | 上述私钥的密码 | 本机 `~/.app_catch-updater/password.txt` |

> 未配置这两个 secret 时：`ci.yml` 照常通过；`release.yml` 会在打包签名一步失败（构建产物仍留作 artifact 供排查），`latest.json` 也不会生成。公钥已写入 `tauri.conf.json`，与私钥成对出现；**私钥或密码一旦丢失，已安装用户将无法再收到新更新**，请离线备份这两份文件。
>
> macOS 产物目前未做 Apple 开发者签名（不影响 Windows 更新；macOS 首次打开需右键「打开」绕过 Gatekeeper）。若之后要做公证，再补 `APPLE_CERTIFICATE` 等 secrets 即可，与现有流程不冲突。

### 本地开发与更新检查

- `pnpm tauri dev` 与浏览器预览里更新器整体禁用（`updaterSupported()`：非宿主或 `import.meta.env.DEV` 判为不可用），设置页显示禁用原因而不是报错，也不会发起任何请求；
- 启动静默检查只在「宿主内 + 非开发 + 偏好开启」时进行（设置页可关），失败完全静默，只有发现新版本才提示一次；
- 本地想手工走一遍更新流：设好 `TAURI_SIGNING_PRIVATE_KEY` / `TAURI_SIGNING_PRIVATE_KEY_PASSWORD` 环境变量后 `pnpm tauri build`，把 `src-tauri/target/release/bundle/` 下的安装包、`.sig` 与手工合并的 `latest.json` 传到一个测试 Release，再装一个旧版本点「检查更新」。

## 测试

| 层 | 命令 | 覆盖 |
| --- | --- | --- |
| launcher-core | `cargo test -p launcher-core` | 配置、路径、会话事件、日志轮转、捕获管线、会话管理器、状态采集、图标编解码与缓存 |
| launcher-core 图标 | `cargo test -p launcher-core --test icon_capture` | **真实 notepad.exe 图标**、真实 WinForms 窗口图标、缓存命中（mtime 不变）、兜底语义、自定义图标优先级 |
| src-tauri 单元 | `cargo test -p app-catch --lib` | 错误转换、DTO 校验、事件分片读取（含半行/轮转/裁剪）、默认图 data URL |
| src-tauri 集成 | `cargo test -p app-catch --test session_events` | 真实进程跑出真实分片，分页游标与直读结果逐条比对 |
| 前端 | `pnpm test` | 日志行转换、游标推进、lossy 判定、自动滚底、表单解析、图标缓存与降级、更新状态机、版本与更新端点契约 |
| 发布脚本 | `pnpm test`（`scripts/merge-updater-manifests.test.mjs`） | latest.json 合并：平台映射、安装包优先级、URL 构造、异常输入 |

## 约定

- 代码标识符用英文，注释与界面文案用中文；
- 小步提交，提交信息遵循中文 Conventional Commits（英文小写 type + 冒号 + 中文描述）；
- 平台相关代码走条件编译 / trait 抽象，不锁死单一平台。
