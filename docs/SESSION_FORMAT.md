# 会话事件格式（JSONL，v=1）

每次启动一个应用 = **一个会话目录 + 一个 JSONL 事件流文件**。
只凭这个文件，AI agent 就能还原「跑了什么、何时、输出什么、怎么结束的」。

本文档中的示例由 `crates/launcher-core` 的实际序列化结果生成，并由测试
`crates/launcher-core/tests/doc_examples.rs` 持续比对，**示例与代码不会漂移**。

## 存储位置

| 内容 | 路径 |
| --- | --- |
| 会话目录 | `<home>/sessions/<session_id>/` |
| 事件流文件 | `<home>/sessions/<session_id>/events.jsonl` |
| 图标缓存 | `<home>/icons/` |
| 应用配置 | `<home>/apps/<app_id>.toml`（见 [CONFIG.md](./CONFIG.md)） |

`<home>` 的解析规则：环境变量 `APP_CATCH_HOME`（非空）优先，否则取用户主目录下的 `.app_catch`。

- `session_id` 是 **uuid v4**，标准连字符小写形式，例如 `e6b562fe-91e5-45f3-8a34-8c42e0be9cef`；
- 同一个应用配置允许**多实例并发**，每次启动各自生成独立的 `session_id`。

## 行格式约定

1. **一行一个 JSON 对象**，行间以 `\n` 分隔；
2. 序列化结果里**不允许出现裸换行**：输出内容中的 `\n`、`\r`、制表符等都由 JSON 转义为 `\n` 等序列，
   `SessionEvent::to_jsonl_line()` 会显式校验并在发现换行时返回错误；
3. **每条事件自带 `v` 版本字段**，使事件流自描述；
4. 字段顺序：`event`（标签）在最前，随后是 `v`、`ts` 与各事件自有字段；
5. 未知事件类型应被读取方**跳过而不是崩溃**（前向兼容，见下文版本策略）。

## 事件总览

| `event` | 用途 | 何时写 |
| --- | --- | --- |
| `start` | 会话起点：启动命令、工作目录、参数、进程信息 | 每次启动写且只写一条，位于文件首行 |
| `output` | 一行 stdout / stderr | 进程每输出一行（不做多行合并） |
| `status` | 运行状态快照 | 状态变化或周期性采样时（窗口标题/端口为尽力而为项） |
| `exit` | 进程结束 | 会话终止时写且只写一条 |
| `overflow` | 显式溢出 | 写盘跟不上、有界 channel 丢弃行时记录丢弃行数 |

## 版本策略（v=1 的演进规则）

- `v` 表示**事件格式版本**，当前恒为 `1`（`launcher_core::session::SESSION_EVENT_VERSION`）；
- **只增不改**：v=1 内可以新增可选字段（读取方必须能忽略未知字段）；
- **必填字段不删除、不改名、不改变语义**；需要破坏性变更时把 `v` 提升到 2，
  同一文件内允许不同版本的行共存，读取方按行内 `v` 分派解析；
- 已知事件类型不变，新增事件类型用新的 `event` 取值；旧读取方遇到未知 `event` 应跳过该行；
- 会话目录与文件名（`sessions/<session_id>/events.jsonl`）**不随版本变化**；
- 日志轮转会以**额外分片文件**出现（见下节），不影响单文件行格式。

## 日志轮转与分片

单会话日志体积上限默认 **50MB**，可通过 `LogWriterConfig::max_file_bytes` 配置。超过上限时按以下规则切换分片：

| 项 | 规则 |
| --- | --- |
| 活跃片 | 恒为 `events.jsonl`，新事件始终追加到这里 |
| 分片命名 | `events.<seq>.jsonl`，`seq` 从 **1** 开始递增（`events.1.jsonl`、`events.2.jsonl`…） |
| 切换时机 | 写入某行**之前**若会超限，则先把当前活跃片改名为分片，再新建空的活跃片 |
| 保留策略 | 只保留最近 **N=5** 个分片（可配），超出时删除最旧的；删除失败只记录不中断 |
| 行完整性 | 轮转只发生在两次写入之间，**任何分片的最后一行都是完整 JSON** |

读取一个会话的完整事件流时，应按 `seq` 升序读取全部分片，最后读活跃片 `events.jsonl`。
工具函数 `launcher_core::runtime::shard_file_name(seq)` 给出分片文件名。

## 编码处理策略

子进程的输出是**字节流**，不保证是 UTF-8。Windows 中文环境的控制台/日志默认是
cp936(GBK)，大量 Python 项目（如 GPT-SoVITS、VITS、各类字幕工具）直接以 GBK 写出中文。
因此捕获层必须显式处理编码，而不是把所有非 UTF-8 字节一律替换掉。

### 解码顺序（每一行独立判定）

| 步骤 | 做法 | 标记 |
| --- | --- | --- |
| 1 | `std::str::from_utf8` **严格**解码成功 | `utf-8` |
| 2 | 判定为「超长行按字节切开的 UTF-8 残片」时按 UTF-8 还原 | `utf-8`（无损失）或 `lossy`（断口处有损） |
| 3 | 失败则按**平台回退编码**解码：Windows 用 `encoding_rs::GBK` | `gbk` |
| 4 | 仍然失败才用 `from_utf8_lossy` 兜底 | `lossy` |

### 为什么不用 `from_utf8_lossy` 兜底

`from_utf8_lossy` 会把每个无法解码的字节静默替换成 U+FFFD。对 GBK 中文来说，
这等于**当场销毁原始信息**：`d6 d0 ce c4 b2 e2 ca d4`（「中文测试」）会变成
一串替换字符，事后无法还原，日志里也看不出发生过什么。这直接违反
「只凭会话目录里的文件，agent 能完整还原输出」的基线验收标准。

所以兜底只在**真正无法解码**时才发生，并且必定带上 `encoding: "lossy"` 标记，
让读取方明确知道「这一行不可信」。

### 为什么是 GBK 而不是 cp1252 / latin-1

- **GBK/GB18030** 是简体中文环境的实际默认代码页（cp936），中文内容解码后是正确汉字；
- **cp1252 / latin-1** 虽然「几乎总能解码」任何字节，但产出的西欧字母对中文日志毫无意义，
  等于换一种方式损坏信息，而且**永远不会触发 lossy**，反而掩盖了问题；
- 因此本项目只做「UTF-8 → GBK」这条与目标平台匹配的回退链，不做「总是能解」的假回退。

### 对 agent 的含义

- `encoding` 缺失 → 按 `utf-8` 解读（旧日志兼容）；
- `encoding: "utf-8"` / `"gbk"` → `line` 就是原文，可直接使用；
- `encoding: "lossy"` → 该行**不可还原**，分析时必须显式说明「此处内容已损坏」，不得当作正常输出；
- 同一次会话的不同行可以有不同编码（进程先输出 UTF-8、后输出 GBK 是常见情况），
  因此编码判定必须**逐行**进行，不能整会话只判定一次。

### 相关 API

```rust
use launcher_core::session::OutputEncoding;

OutputEncoding::Utf8.as_str();   // "utf-8"
OutputEncoding::Gbk.as_str();    // "gbk"
OutputEncoding::Lossy.as_str();  // "lossy"
OutputEncoding::default();       // Utf8 —— 旧日志缺字段时的默认值
```

## 溢出（overflow）语义

输出捕获走「读管道 → 有界 channel → 串行写盘」，channel 默认容量 8192 行。
当写盘速度跟不上、channel 满时，读取方**绝不阻塞子进程**，而是丢弃当前行并累计计数，
随后补写 `overflow` 事件：

- `dropped` 是该条 `overflow` 事件覆盖的丢弃行数（可能累计多条）；
- 出现 `overflow` 即表示**该时间点附近的输出不完整**，分析时必须说明；
- 正常负载下 `dropped` 恒为 0，`overflow` 事件不会出现。

## 会话重关联（recover）语义

启动器重启后，`SessionManager::recover_sessions()` 会扫描 `sessions/` 下每个会话目录，
解析事件流，找出**有 `start` 无 `exit`** 的会话，用 `start` 里的 `pid` + `start_time` 调用 `attach`：

| 判定 | 处理 |
| --- | --- |
| pid 存活且 `start_time` 一致 | 标记为 running，追加一条 `status(state=running)` 事件 |
| 进程不存在 | 追加 `exit(code=null, reason=crashed)`，会话收尾 |
| `start_time` 不一致（**PID 已被复用**） | 同上：追加 `exit(code=null, reason=crashed)`，绝不误认 |
| `start_time` 为 0（旧事件） | 视为无法判定身份，按已消失处理，追加 `exit(crashed)` |

> **已知限制**：重启启动器后**无法重新接管**已有进程的 stdout/stderr ——
> 管道句柄随父进程消失，新进程无法再读取旧进程的输出。
> 这些会话的日志会停在重启前的位置，重关联后也不会再追加新的 `output` 事件。
> 详见 [RUNTIME.md](./RUNTIME.md)。

## 各事件字段

`ts` 统一为 **RFC3339 字符串（UTC，以 `Z` 结尾）**，例如 `2026-10-24T18:30:00.123456789Z`。

### start

| 字段 | 类型 | 必有 | 说明 |
| --- | --- | --- | --- |
| `event` | string | ✅ | 固定 `"start"` |
| `v` | number | ✅ | 事件格式版本，当前 `1` |
| `ts` | string | ✅ | RFC3339 时间戳 |
| `session_id` | string | ✅ | uuid v4，与会话目录名一致 |
| `app_id` | string | ✅ | 关联的应用配置 id |
| `cmd` | string | ✅ | 可读命令行（给人和 agent 看） |
| `cwd` | string | ✅ | 子进程工作目录 |
| `argv` | string[] | ✅ | 完整参数向量（含 argv[0]），可精确还原启动方式 |
| `pid` | number | ✅ | 子进程 pid |
| `ppid` | number | ✅ | 启动器自身 pid |
| `start_time` | number | ✅ | 子进程启动时间的平台原始值（Windows 为 FILETIME，100ns 粒度）。与 `pid` 一起构成进程身份，用于重启后判定 PID 是否被复用；旧事件缺省为 `0`，表示无法重关联 |

### output

| 字段 | 类型 | 必有 | 说明 |
| --- | --- | --- | --- |
| `event` | string | ✅ | 固定 `"output"` |
| `v` | number | ✅ | `1` |
| `ts` | string | ✅ | RFC3339 时间戳 |
| `stream` | string | ✅ | `"stdout"` 或 `"stderr"` |
| `line` | string | ✅ | 单行内容（不含行尾换行） |
| `encoding` | string | ❌ | 该行原始字节的编码标记，取值见下表；**缺失时按 `"utf-8"` 解读**（向后兼容旧日志） |

`encoding` 的取值与含义：

| 取值 | 含义 | 原始字节可否还原 |
| --- | --- | --- |
| `"utf-8"` | 原始字节本身就是合法 UTF-8，严格解码成功 | ✅ 完全可还原 |
| `"gbk"` | 原始字节不是合法 UTF-8，但按 GBK/GB18030（cp936）无损解码成功 | ✅ 完全可还原（按 GBK 解码后即原文） |
| `"lossy"` | 两种解码都失败，已用 U+FFFD 替换字符兜底 | ❌ **不可还原**，该行内容有损 |

> Windows 中文环境下 Python 项目默认以 cp936(GBK) 输出，因此 `"gbk"` 在真实日志中很常见，
> 它不是异常，而是「这行是 GBK 编码的中文」的正常标记。

### status

| 字段 | 类型 | 必有 | 说明 |
| --- | --- | --- | --- |
| `event` | string | ✅ | 固定 `"status"` |
| `v` | number | ✅ | `1` |
| `ts` | string | ✅ | RFC3339 时间戳 |
| `state` | string | ✅ | `"running"` / `"exited"` / `"crashed"` |
| `window_title` | string 或 null | ❌ | 窗口标题；取不到为 `null` |
| `ports` | number[] 或 null | ❌ | 监听端口；取不到为 `null` |

### exit

| 字段 | 类型 | 必有 | 说明 |
| --- | --- | --- | --- |
| `event` | string | ✅ | 固定 `"exit"` |
| `v` | number | ✅ | `1` |
| `ts` | string | ✅ | RFC3339 时间戳 |
| `code` | number 或 null | ❌ | 退出码；被信号杀死或取不到时为 `null` |
| `reason` | string | ✅ | `"normal"` / `"killed"` / `"crashed"` |

### overflow

| 字段 | 类型 | 必有 | 说明 |
| --- | --- | --- | --- |
| `event` | string | ✅ | 固定 `"overflow"` |
| `v` | number | ✅ | `1` |
| `ts` | string | ✅ | RFC3339 时间戳 |
| `dropped` | number | ✅ | 本次丢弃的输出行数（不静默丢行） |

## 完整示例

下面 9 行覆盖了全部事件形态（真实文件里 `output` 会有成百上千行）：

```jsonl
{"event":"start","v":1,"ts":"2026-10-24T18:30:00.123456789Z","session_id":"e6b562fe-91e5-45f3-8a34-8c42e0be9cef","app_id":"gpt-sovits","cmd":"python main.py --port 9880","cwd":"E:/repo/GPT-SoVITS","argv":["python","main.py","--port","9880"],"pid":4321,"ppid":1234,"start_time":134000000000000000}
{"event":"output","v":1,"ts":"2026-10-24T18:30:01.000000000Z","stream":"stdout","line":"Running on local URL: http://127.0.0.1:9880","encoding":"utf-8"}
{"event":"output","v":1,"ts":"2026-10-24T18:30:02.000000000Z","stream":"stderr","line":"warning: torch not found, fallback to CPU","encoding":"utf-8"}
{"event":"status","v":1,"ts":"2026-10-24T18:30:03.000000000Z","state":"running","window_title":"GPT-SoVITS","ports":[9880]}
{"event":"status","v":1,"ts":"2026-10-24T18:30:04.000000000Z","state":"running","window_title":null,"ports":null}
{"event":"exit","v":1,"ts":"2026-10-24T18:31:00.000000000Z","code":0,"reason":"normal"}
{"event":"exit","v":1,"ts":"2026-10-24T18:32:00.000000000Z","code":null,"reason":"killed"}
{"event":"exit","v":1,"ts":"2026-10-24T18:33:00.000000000Z","code":1,"reason":"crashed"}
{"event":"overflow","v":1,"ts":"2026-10-24T18:34:00.000000000Z","dropped":128}
```

> 这是**形态示例**：真实文件里一个会话只有一条 `start`、一条 `exit`，
> 这里把三种退出原因都列出以便查阅。

## 读取建议（给 agent / 分析脚本）

- 按行读取并跳过空行；对每行做 JSON 解析；
- 用 `event` 字段分派；用 `v` 判断版本，未知 `event` 或未知字段直接忽略；
- `output` 的 `stream` 区分 stdout/stderr，`line` 是按行切分的原始内容（不含换行符）；
- `output` 的 `encoding` 说明该行原始编码：`utf-8` / `gbk` 可直接使用，`lossy` 表示内容已损坏、
  分析时必须说明；字段缺失时按 `utf-8` 解读（兼容旧日志）；
- 出现 `overflow` 事件表示该时间点有 `dropped` 行输出未被写入，分析时需说明数据不完整；
- 文件末尾没有 `exit` 事件 = 会话未正常结束（启动器被强杀等），应视为不完整会话。

## 相关 API

```rust
use launcher_core::session::{SessionEvent, SessionId, StreamKind};

let id = SessionId::new_v4();                       // uuid v4
let line = SessionEvent::output(StreamKind::Stdout, "hello")
    .to_jsonl_line()?;                              // 单行 JSON，含 "v":1
let back = SessionEvent::from_jsonl_line(&line)?;   // 反序列化回等值结构
```
