# app_catch

开发者向的桌面应用启动器：把 E:\repo 下的自开发应用收进可管理列表，一键启动，并把每个会话的进程信息、运行状态与 stdout/stderr 全量捕获为结构化 JSONL，供 AI agent 直接读取。

> 当前进度：**M1 数据层（M1a）进行中**。已完成路径布局与应用配置 TOML 存储；会话事件模型与平台抽象接口、进程运行时逻辑随后补齐。界面仍是 M0 的状态页。

## 目录架构

| 路径 | 说明 |
| --- | --- |
| `crates/launcher-core/` | 纯 Rust 核心库：配置管理、进程生命周期、会话 JSONL、图标捕获（M1 起实现）。**不依赖 tauri 或任何 UI 库**，可在无窗口环境下独立测试。 |
| `src-tauri/` | Tauri 2 壳层：`src/lib.rs` 为 command 薄封装层，业务一律委托给 launcher-core；`tauri.conf.json` 为 v2 schema 配置；`capabilities/` 为权限声明。 |
| `src/` | 前端（React + TypeScript + Vite）：M0 只有一个状态页，通过 `invoke` 调用 Tauri command 展示 launcher-core 版本号。 |
| `docs/REQUIREMENTS.md` | 需求与决策基线（与仓库根 `PROMPT.md` 内容完全一致）。 |
| `docs/CONFIG.md` | 应用配置 TOML 格式：字段表、默认值、存储位置、错误语义。 |
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

# 开发模式：启动 Vite(5173) 并拉起 Tauri 窗口
pnpm tauri dev

# 仅构建前端产物（tsc 类型检查 + vite build，输出到 dist/）
pnpm build

# Rust 侧检查与测试（含 launcher-core 单元测试）
cargo check --workspace
cargo test --workspace

# 打包安装包
pnpm tauri build
```

> `pnpm tauri dev` / `pnpm tauri build` 依赖 `dist/` 与 Vite 端口 5173（`vite.config.ts` 中 `strictPort` 已锁定，避免端口漂移导致白屏）。

## 里程碑进度

| 里程碑 | 内容 | 状态 |
| --- | --- | --- |
| M0 | 仓库初始化：cargo workspace + Tauri 2 脚手架，cargo check/test 全绿，需求基线入库，README 写清构建方式 | ✅ 已完成 |
| M1 | launcher-core（Windows 先行）：F1 配置、F2 启动、F3 捕获 + JSONL + 轮转、F4 停止与进程树回收 | 🚧 进行中（数据层完成） |
| M2 | GUI：应用列表（图标/名称/状态）、启动/停止/重启、会话详情（实时日志流、状态、退出信息） | ⏳ 待做 |
| M3 | 图标：Windows 抓取 + 缓存 + 兜底 | ⏳ 待做 |
| M4 | macOS 对齐：构建通过、进程组语义、图标抓取 | ⏳ 待做 |
| M5 | 打包与端到端验收：用真实项目（Python 服务 + GUI 应用）走完整流程 | ⏳ 待做 |

## launcher-core 模块一览（M1a 数据层）

| 模块 | 内容 | 文档 |
| --- | --- | --- |
| `config` | `AppId` / `AppConfig` / `AppStore`：每应用一个 TOML，原子写盘 | [docs/CONFIG.md](docs/CONFIG.md) |
| `paths` | `APP_CATCH_HOME` 优先的目录布局（apps / sessions / icons） | [docs/CONFIG.md](docs/CONFIG.md) |
| `error` / `fsutil` | 统一错误类型 `AppCatchError` 与原子写盘工具 | 见源码 rustdoc |

> 数据隔离：所有测试通过环境变量 `APP_CATCH_HOME` 指向 `tempfile` 临时目录，**绝不触碰真实的 `~/.app_catch`**。

## 约定

- 代码标识符用英文，注释与界面文案用中文；
- 小步提交，提交信息遵循中文 Conventional Commits（英文小写 type + 冒号 + 中文描述）；
- 平台相关代码走条件编译 / trait 抽象，不锁死单一平台。