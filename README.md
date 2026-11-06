# app_catch

开发者向的桌面应用启动器：把本地自开发应用收进一个列表，一键启动，并把每个会话的进程信息、运行状态与 stdout/stderr 全量捕获为结构化 JSONL，供 AI agent 直接读取。

## 功能

- **应用管理**：每个应用一份 TOML（id / 名称 / 命令 / 参数 / 工作目录 / 环境变量 / 图标 / 标签），原子写盘。
- **会话生命周期**：启动、停止、重启，支持同一应用多实例；停止时按进程树回收（Windows Job Object，macOS/Linux 进程组 + SIGTERM→SIGKILL）。
- **输出捕获**：逐行解码「严格 UTF-8 → GBK 回退 → lossy 标记」，写 JSONL 并按大小轮转分片。
- **状态采集**：仅状态变化时追加 `status` 事件，含窗口标题与监听端口；启动器重启后可按 PID + 启动时间重关联残留会话。
- **图标**：自定义 → 运行中窗口 → 项目图标 → exe 资源逐档回退，结果缓存为 PNG；抓不到用内置默认图。
- **界面**：自绘标题栏、可折叠抽屉、双主题、全局搜索、实时日志与结构化事件表。
- **更新**：`tauri-plugin-updater`，GitHub Release 分发安装包与 `latest.json`。

## 目录结构

| 路径 | 说明 |
| --- | --- |
| `crates/launcher-core/` | 纯 Rust 核心库：配置、进程生命周期、会话 JSONL、图标捕获。不依赖 tauri 或 UI。 |
| `src-tauri/` | Tauri 2 壳层：命令注册与薄封装、DTO、事件分片读取。 |
| `src/` | 前端（React + TypeScript + Vite），纯 CSS，无 UI 库。 |
| `docs/` | 配置、会话格式、运行时、界面、图标五份规范文档。 |
| `scripts/` | 会话目录复原、CDP 驱动安装版 GUI、应用图标生成、发布清单合并、dev 启动器。 |
| `.github/workflows/` | `ci.yml`（lint / 测试 + clippy / main 与 PR 上打包）与 `release.yml`（tag 触发打包发布）。 |
| `Cargo.toml` | cargo workspace 根，公共依赖在 `[workspace.dependencies]`。 |

## 环境要求

- Rust stable
- Node.js 20+ 与 pnpm
- Windows：MSVC 生成工具 + WebView2 运行时；macOS：Xcode Command Line Tools

## 常用命令

```bash
pnpm install          # 安装前端依赖
pnpm tauri dev        # Vite(15173) + Tauri 开发窗口
pnpm dev              # 仅启动 Vite，浏览器里预览界面
pnpm build            # tsc 类型检查 + vite build 到 dist/
pnpm test             # 前端纯逻辑单测
pnpm lint             # ESLint（含 React Hooks 规则）
cargo test --workspace  # Rust 单元测试与集成测试
cargo clippy --workspace --all-targets -- -D warnings  # Rust 静态检查（CI 同款）
cargo fmt --all       # Rust 格式化（CI 用 --check 校验）
pnpm tauri build      # 打包安装包
```

Vite 端口在 `vite.config.ts` 中锁定为 15173（`strictPort`）。

## 打包与运行数据

```bash
pnpm tauri build
```

| 产物 | 路径 |
| --- | --- |
| NSIS 安装包（当前用户级安装） | `target/release/bundle/nsis/app_catch_<version>_x64-setup.exe` |
| MSI 安装包 | `target/release/bundle/msi/app_catch_<version>_x64_en-US.msi` |
| 免安装可执行文件 | `target/release/app-catch.exe` |

安装版与 dev 模式共用路径规则：默认 `~/.app_catch`，首次启动创建 `apps/`、`sessions/`、`icons/`；可用 `APP_CATCH_HOME` 覆盖。配置与应用不做自动发现，需要手动录入。

## 发布

推送 `v*.*.*` tag 触发 `release.yml`：矩阵打包 → 签名更新产物 → 创建 GitHub Release → 上传安装包与合并后的 `latest.json`。需在仓库配置 `TAURI_SIGNING_PRIVATE_KEY` / `TAURI_SIGNING_PRIVATE_KEY_PASSWORD` 两个 secret；`package.json`、`tauri.conf.json`、workspace `Cargo.toml` 三处版本号需一致（由测试守护）。

## 文档

| 文档 | 内容 |
| --- | --- |
| [docs/CONFIG.md](docs/CONFIG.md) | 应用配置 TOML：字段、默认值、存储位置、错误语义 |
| [docs/SESSION_FORMAT.md](docs/SESSION_FORMAT.md) | 会话 JSONL：五类事件字段、编码策略、轮转分片、overflow 与 recover |
| [docs/RUNTIME.md](docs/RUNTIME.md) | 运行时：生命周期状态机、`SessionManager` API、退出分类、各平台进程后端 |
| [docs/GUI.md](docs/GUI.md) | 界面结构、命令层 API、推送与兜底轮询、分片读取 |
| [docs/ICONS.md](docs/ICONS.md) | 图标优先级链、PNG 规范、缓存失效、兜底语义 |

## 测试

| 层 | 命令 |
| --- | --- |
| launcher-core（含真实图标与进程测试） | `cargo test -p launcher-core` |
| src-tauri（错误映射、DTO、事件分片读取） | `cargo test -p app-catch` |
| 前端纯逻辑与样式契约 | `pnpm test` |

测试统一通过 `APP_CATCH_HOME` 指向临时目录，不触碰真实的 `~/.app_catch`。

## 约定

- 代码标识符用英文，注释与界面文案用中文；
- 提交信息遵循中文 Conventional Commits（英文小写 type + 冒号 + 中文描述）；
- 平台相关代码走条件编译 / trait 抽象，不锁死单一平台。
