# app_catch 项目启动提示词（Kickoff Prompt）

你将开发一个名为 app_catch 的开发者向桌面应用。本提示词由前期需求分析会话产出，是完整的需求与决策基线：标注「已锁定」的决策不要重新讨论；标注「默认方案」的条目按默认执行，除非我另有说明；有疑问在开工前列出问我。

## 1. 背景与痛点

我在 E:\repo 下有几十个自开发项目，形态混杂：Python 长跑服务与工具（如 GPT-SoVITS、VITS、Whisper 字幕工具）、后端 API 服务、Electron/Python GUI 应用。当前痛点：

1. 每次使用都要开终端手敲启动命令，一个应用一次，重复低效；
2. 应用的输出与运行状态散落在各个终端窗口，终端一关就丢；
3. 这种形态对 AI agent 极不友好：agent 无法可靠读到「什么在跑、输出是什么、退出了没有」。

## 2. 产品定义

一个开发者向的统一应用启动器（桌面 GUI）：把所有自开发应用收进一个可管理的列表，一键启动；自动捕获每个会话的进程信息、运行状态与输出，持久化为结构化 JSONL 文件，供 AI agent 直接读取分析；尽力抓取应用图标，让列表观感是「正经的应用列表」而不是一堆命令行条目。

## 3. 已锁定决策

| 项 | 决策 |
| --- | --- |
| 技术栈 | Rust + Tauri 2：核心逻辑（进程/会话/图标/存储）为纯 Rust crate，界面层用 web 技术 |
| 平台 | Windows 与 macOS 优先（我在两平台开发）；平台相关代码走抽象层，不锁死 Linux，但 Linux 不在本期范围 |
| 形态 | 桌面 GUI 应用 |
| 会话存储 | JSONL 文件，包含全部必要信息，agent 可直接读文件消费（不做查询服务） |
| 会话捕获范围 | 进程信息（PID/进程树/退出码）+ 运行时状态（运行中/崩溃/退出、窗口标题、监听端口等）+ stdout/stderr 全量 |
| 图标策略 | 应用启动后尽力抓取运行时图标（dev 模式下可执行文件常是默认图标，真图标在窗口上）；抓不到用内置默认图标兜底，不算失败 |
| 性能 | 核心不用 Python/TypeScript；日志流 MB/s 级不丢、不阻塞被启动应用 |
| 仓库 | E:\repo\app_catch（现为空目录，从零初始化） |

## 4. 功能需求

F1 配置管理：每个应用一条配置（id、名称、启动命令、参数、工作目录、环境变量、可选自定义图标、标签）；增删改查、持久化。默认放 ~/.app_catch/apps/ 下每应用一个 TOML，具体格式实现时定并写进文档。

F2 启动：列表一键拉起；同一配置允许多实例并发，每次启动生成唯一 session_id。

F3 会话捕获与持久化（核心）：
- 每次启动 = 一个会话目录 ~/.app_catch/sessions/<session_id>/，内含单个 JSONL 事件流，草案（字段可完善，但「一个会话一个 JSONL、事件流、自描述带版本」三点不变）：

```jsonl
{"v":1,"event":"start","ts":"RFC3339","session_id":"…","app_id":"…","cmd":"python main.py","cwd":"…","argv":[],"pid":123,"ppid":456}
{"v":1,"event":"output","ts":"…","stream":"stdout","line":"…"}
{"v":1,"event":"status","ts":"…","state":"running","window_title":"…","ports":[8000]}
{"v":1,"event":"exit","ts":"…","code":1,"reason":"normal|killed|crashed"}
```

- 状态监控：运行中/已退出/崩溃（区分正常退出、被杀、崩溃）与退出码；窗口标题、监听端口为尽力而为项，取不到不报错。
- 日志治理：单会话 JSONL 有大小上限（默认 50MB、可配），超限分片并保留最近 N 片，防长跑服务撑爆磁盘。
- 验收标准：只凭会话目录里的文件，agent 能完整还原「跑了什么、何时、输出什么、怎么结束的」。

F4 生命周期：停止（整棵进程树回收，Windows 用 Job Object 保证子进程不残留）、重启、查看状态。启动器自身退出不杀已启动的应用；重启启动器后能按 PID + 进程启动时间重新关联存量会话（防 PID 复用误判）。

F5 图标捕获：Windows 从 exe 资源与运行中窗口图标抓取；macOS 从 .app bundle / NSRunningApplication 抓取；统一规范化为 PNG 缓存到 ~/.app_catch/icons/；失败用内置默认图标。Python 服务类无窗口应用预期抓不到图标，走兜底。

## 5. 非功能需求

- 扩展性：状态项、捕获器、平台后端做成可扩展接口（trait），新增状态项或平台不改核心流程；
- 单安装包分发（Windows NSIS/MSI、macOS dmg）；
- 界面文案中文；
- launcher-core 不依赖 UI，可独立测试。

## 6. 架构建议（可调整，改动需说明理由）

- cargo workspace：crates/launcher-core（配置、进程管理、会话 JSONL、图标捕获，平台代码用条件编译隔离）+ src-tauri（Tauri 2 壳，command 层薄封装）+ src/ 前端（框架选主流之一即可）；
- 平台抽象 trait：spawn / kill_tree / attach / capture_icon 等；
- tokio 异步运行时；子进程输出管道 → 有界 channel → 独立写盘任务；写盘跟不上时显式记 overflow 事件，不静默丢行、不阻塞子进程。

## 7. 里程碑

- M0 仓库初始化：cargo workspace + Tauri 2 脚手架，cargo check/test 全绿；把本提示词存为 docs/REQUIREMENTS.md 作为基线；README 写清构建方式
- M1 launcher-core（Windows 先行）：F1 配置、F2 启动、F3 捕获 + JSONL + 轮转、F4 停止与进程树回收；用临时脚本进程做单元测试
- M2 GUI：应用列表（图标/名称/状态）、启动/停止/重启、会话详情（实时日志流、状态、退出信息）
- M3 图标：Windows 抓取 + 缓存 + 兜底
- M4 macOS 对齐：构建通过、进程组语义、图标抓取
- M5 打包与端到端验收：用 E:\repo 下真实项目（一个 Python 服务 + 一个 GUI 应用）走完整流程

每个里程碑：小步提交（中文 Conventional Commits）、cargo test 通过、更新 README 进度。

## 8. 默认方案（按此执行，除非我否决）

- 崩溃自动重启：一期不做，只记录 crashed 状态；
- stdin 转发：一期不做，只捕获输出；
- 配置自动发现/导入：一期不做，纯手动录入；
- 端口/窗口标题状态项：尽力而为，失败静默。

## 9. 开发环境

- 主开发机 Windows，仓库在 E:\repo\app_catch；Rust stable；Tauri CLI v2；Windows 依赖 WebView2（系统一般自带）；
- macOS 侧到 M4 再对齐，前期不阻塞。

## 10. 执行约定

- 先读完本提示词；已锁定项不重新讨论；偏离基线的设计决定先说明理由并征得我同意；
- 顺序 M0 → M1 → M2 …，先核心后 GUI；每个里程碑结束时给我一个可验证的结果（命令或操作步骤）。
