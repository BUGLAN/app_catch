# 应用配置格式（TOML）

app_catch 为每个应用保存**一个独立的 TOML 文件**，字段全部使用 snake_case。

本文档中的示例由 `crates/launcher-core` 的实际序列化结果生成，并由测试
`crates/launcher-core/tests/doc_examples.rs` 持续比对，**示例与代码不会漂移**。

## 存储位置

| 场景 | 路径 |
| --- | --- |
| 默认（Windows / macOS） | `~/.app_catch/apps/<app_id>.toml` |
| 覆盖（环境变量 `APP_CATCH_HOME`） | `$APP_CATCH_HOME/apps/<app_id>.toml` |

- 根目录解析优先级：**环境变量 `APP_CATCH_HOME`（非空）> 用户主目录下的 `.app_catch`**；
- 文件名由 `id` 决定：`<app_id>.toml`，因此 `id` 本身即主键；
- 目录不存在时 `AppStore::list()` 返回空列表（首次启动语义），不会报错；
- `list()` 只读取 `*.toml` 文件，忽略其它文件与原子写入可能残留的临时文件。

## 字段表

| 字段 | 类型 | 必填 | 默认值 | 说明 |
| --- | --- | --- | --- | --- |
| `id` | string | ✅ | — | 应用标识符，同时是文件名。只允许 `[A-Za-z0-9_-]`，非空，长度 ≤ 64 |
| `name` | string | ✅ | — | 展示名称，界面列表显示用 |
| `command` | string | ✅ | — | 启动命令（不含参数），如 `python` / `node` / 可执行文件绝对路径 |
| `args` | string[] | ❌ | `[]` | 命令行参数，顺序即传给子进程的顺序 |
| `cwd` | string | ❌ | 空（表示继承启动器当前目录） | 子进程工作目录 |
| `env` | table<string,string> | ❌ | `{}` | 环境变量**增量**，在父进程环境之上追加/覆盖 |
| `icon` | string 或省略 | ❌ | 省略 = 走运行时抓取 + 内置兜底图 | 自定义图标路径 |
| `tags` | string[] | ❌ | `[]` | 标签，供 GUI 分组与过滤 |
| `hooks.before` | string[] | ❌ | `[]` | 启动前 hook：每项一条 shell 命令，详见下文「启动 hook」 |
| `hooks.after` | string[] | ❌ | `[]` | 启动后 hook：每项一条 shell 命令 |
| `hooks.timeout_secs` | integer | ❌ | `600` | 单条 hook 命令的超时（秒），超时视为失败并杀掉其进程树 |

补充约定：

- **空值不落盘**：`args` / `cwd` / `env` / `icon` / `tags` / `hooks` 为空时不会写入文件，
  保持配置文件干净；读取时这些字段自动取默认值（向后兼容旧配置）。
- `env` 使用 `BTreeMap`，键按字典序输出，保证同样的配置产生同样的字节序列（便于 diff）。
- 路径使用正斜杠或反斜杠均可，Windows 下建议写成 `E:/repo/demo`（TOML 中反斜杠需转义）。

### 关于 id 的约束

| 规则 | 反例 |
| --- | --- |
| 非空 | `""` |
| 长度 ≤ 64 | 65 个字符的 id |
| 仅 `A-Z a-z 0-9 _ -` | `with space`、`中文`、`a/b`、`dot.dot` |
| 不能是路径穿越片段 | `..` |

因为 id 直接参与文件名拼接，以上约束确保配置**不可能写到 `apps/` 目录之外**。
非法 id 会得到 `AppCatchError::InvalidId { value, reason }`。

## 完整示例（全部字段）

```toml
id = "gpt-sovits"
name = "GPT-SoVITS"
command = "python"
args = [
    "main.py",
    "--port",
    "9880",
]
cwd = "E:/repo/GPT-SoVITS"
icon = "E:/repo/GPT-SoVITS/icon.png"
tags = [
    "python",
    "tts",
]

[env]
PYTHONUNBUFFERED = "1"
RUST_LOG = "info"

[hooks]
before = ["git pull --ff-only"]
```

## 启动 hook

`[hooks]` 用来在启动应用前后执行命令，典型场景是启动本地开发应用前先拉取 GitHub 更新：

```toml
[hooks]
before = ["git pull --ff-only", "pnpm install --frozen-lockfile"]
after = ["open http://localhost:5173"]
timeout_secs = 300
```

| 阶段 | 执行时机 | 失败时（非 0 退出 / 无法启动 / 超时） |
| --- | --- | --- |
| `before` | 点击启动（或重启）后、主进程 spawn **之前**，按数组顺序逐条同步执行；全部成功才启动主进程 | **中止启动**：后续 hook 不再执行，主进程不启动，不创建会话目录；界面弹出错误提示，内容含失败的命令、退出码与输出末尾几行 |
| `after` | 主进程 spawn 成功、`start` 事件落盘**之后**，在后台按顺序执行，不阻塞启动 | **只记日志**：后续 after hook 不再执行，主进程照常运行 |

执行细节：

- 每条命令交给系统 shell：macOS/Linux 为 `/bin/sh -c`，Windows 为 `cmd /D /S /C`，因此可以写 `&&`、管道、重定向；
- 工作目录与环境变量与应用的 `cwd` / `env` 一致；PATH 与主进程一样会补齐登录 shell 的 PATH；
- 额外注入环境变量：`APP_CATCH_APP_ID`、`APP_CATCH_SESSION_ID`、`APP_CATCH_HOOK_STAGE`（`before` / `after`），
  after 阶段还有主进程 pid `APP_CATCH_PID`；
- hook 没有 stdin（读 stdin 立即 EOF），需要交互输入的命令（如要求输入密码的 `git pull`）会失败或超时；
- 输出写进会话日志的 `output` 事件，行首带 `[before]` / `[after]` 前缀，并带一行 `$ <命令>` 与一行结果汇总。
  before 阶段会话日志尚未创建（`start` 必须是首行），其输出先缓冲在内存（保留末尾 2000 行），
  主进程启动后紧跟在 `start` 之后补写；
- 停止会话不会终止仍在运行的 after hook，它只受 `timeout_secs` 约束；
- Windows 下 Rust 会按 MSVC 规则给含空格的参数加引号，命令里若再写双引号，`cmd` 可能解析异常，
  复杂命令建议写成脚本文件再调用。

## 最小示例（只有必填字段）

```toml
id = "hello-service"
name = "Hello 服务"
command = "node"
```

## 写盘与并发

- 写入采用**原子替换**：先在目标同目录写临时文件（`<name>.tmp-<pid>-<nanos>`），
  `flush + sync_all` 之后再 `rename` 覆盖目标文件；
- 因此读到的配置**要么是完整的旧版本，要么是完整的新版本**，不会出现半截 TOML；
- 失败时会清理临时文件并返回 `AppCatchError::Io { path, source }`。

## 错误语义

| 场景 | 错误 |
| --- | --- |
| `id` 不合法 | `AppCatchError::InvalidId` |
| `create` 时 id 已存在 | `AppCatchError::AlreadyExists` |
| `get` / `update` / `delete` 时文件不存在 | `AppCatchError::NotFound` |
| TOML 语法或字段类型错误 | `AppCatchError::Parse { path, source }` |
| 序列化失败 | `AppCatchError::Serialize` |
| 目录/文件读写失败 | `AppCatchError::Io { path, source }` |

## 相关 API

```rust
use launcher_core::config::{AppConfig, AppId, AppStore};

let store = AppStore::from_home()?;      // 按 APP_CATCH_HOME / ~/.app_catch 推导
let id: AppId = "gpt-sovits".parse()?;   // 非法 id 返回 AppCatchError::InvalidId
let cfg = store.get(&id)?;               // 不存在返回 AppCatchError::NotFound
```
