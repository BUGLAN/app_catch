# 应用配置格式（F1）

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
| `icon` | string 或省略 | ❌ | 省略 = 走运行时抓取 + 内置兜底图 | 自定义图标路径（M3 起使用） |
| `tags` | string[] | ❌ | `[]` | 标签，供 GUI 分组与过滤 |

补充约定：

- **空值不落盘**：`args` / `cwd` / `env` / `icon` / `tags` 为空时不会写入文件，
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
```

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
