# 会话运行时（RUNTIME）

本文档面向 **M2 GUI 对接**：说明会话的生命周期状态机、`SessionManager` 公开 API、退出分类规则，以及日志轮转与溢出策略。

事件字段的逐字段定义见 [SESSION_FORMAT.md](./SESSION_FORMAT.md)，本文只讲「怎么用、为什么这样设计」。

## 分层

```text
SessionManager（会话门面，async）
   ├── CapturePipeline（stdout/stderr 读取 → 有界 channel → 串行写盘）
   │      ├── 输出 reader 任务 ×2（stdout / stderr）
   │      ├── CaptureSink（唯一写入口：状态循环与退出收尾都只能经它写）
   │      └── 写盘任务 ×1 ── LogWriter（JSONL 追加、批量 flush、轮转、保留 N 片）
   ├── 状态采集循环（StatusProbe → 变化才写 status 事件，经同一个 CaptureSink）
   └── ProcessBackend（平台抽象：spawn / kill_tree / attach / is_running / wait）
          └── WindowsProcessBackend（Job Object + 启动时间校验）
```

`SessionManager` 只依赖 `Arc<dyn ProcessBackend>`，因此可替换实现（macOS 在 M4 接入）。

### 单 writer 所有权（轮转与保留策略成立的前提）

**硬约束：一个会话目录下，同一时刻只能有一个 `LogWriter`。**

原因是 `LogWriter` 把轮转状态保存在自己内存里——`shard_seq`（下一个分片序号）
和 `shards`（已轮转出的分片列表，用于保留策略）。多开一个 writer 就等于多一份
「它自己以为的分片现状」：

| 现象 | 机制 |
| --- | --- |
| 分片序号乱跳 | 后开的 writer 看不到已有分片，`shard_seq` 从 0 重新计数，落到 `events.28.jsonl` 这类错位序号 |
| 旧分片永远删不掉 | prune 只按自己那份不完整的 `shards` 列表删，真正最旧的分片不在列表里 |
| 保留上限形同虚设 | 上述两条叠加 → 长跑服务分片无限增长，最终撑爆磁盘 |

**做法**（实现见 `runtime::capture::CaptureSink`）：

```text
任意事件来源（输出 reader / 状态循环 / 退出收尾）
        │  CaptureSink::{try_write, write, finish}
        ▼
   mpsc channel（有界）
        │
        ▼
   唯一的写盘任务 ── 独占 LogWriter ── events.jsonl / events.&lt;seq&gt;.jsonl
```

| 方法 | 语义 | 使用方 |
| --- | --- | --- |
| `try_write` | **非阻塞**：通道满只累计丢弃计数（由 `overflow` 事件如实反映），绝不阻塞子进程 | 输出 reader |
| `write` | **背压 + 落盘确认**：带 oneshot 回执，写完并 flush 才返回，返回 true 即「此刻读日志就能看到」 | 状态循环、重关联补写 |
| `finish` | **收尾**：写入后写盘任务关闭文件并退出，之后任何写都会失败，`exit` 因此必然是最后一条 | 退出收尾 |

> **为什么用 channel 而不是 `Mutex<LogWriter>`**：`LogWriter::write_event` / `close` 都是 async，
> 包进互斥锁会让每个状态事件与整条输出管道的 flush 抢同一把锁，退出收尾还要再抢一次；
> channel 让写盘任务天然独占 writer，事件顺序（output → overflow → exit）由队列本身保证，
> 同时保住「生产者绝不阻塞子进程」这条不变量。

## 生命周期状态机

```text
                    start()
                       │
                       ▼
                 ┌──────────┐
                 │ Running  │
                 └────┬─────┘
                      │
      ┌───────────────┼────────────────┬──────────────────┐
      │ code == 0     │ code != 0      │ 调用 stop()       │
      │ 且未主动停止   │ 且未主动停止    │ （主动停止）       │
      ▼               ▼                ▼
 ┌─────────┐    ┌──────────┐     ┌──────────┐
 │ Exited  │    │ Crashed  │     │ Exited   │
 │ normal  │    │ crashed  │     │ killed   │
 └─────────┘    └──────────┘     └──────────┘
```

| 状态 | `RunState` | `ExitReason` | 含义 |
| --- | --- | --- | --- |
| 运行中 | `running` | — | 进程存活，输出持续落盘 |
| 正常退出 | `exited` | `normal` | 进程自行退出且退出码为 0 |
| 崩溃/异常 | `crashed` | `crashed` | 进程自行退出但退出码非 0，或被外部杀死 |
| 被停止 | `exited` | `killed` | 由本启动器调用 `stop()` 主动回收 |

> 被外部杀死（如任务管理器结束进程）时退出码通常非 0，归为 `crashed`——因为启动器没有发起停止请求，无法区分「崩溃」与「被杀」。

## 退出分类规则

实现见 `runtime::session_manager::classify_exit`：

```rust
pub fn classify_exit(stop_requested: bool, code: Option<i32>) -> ExitReason {
    if stop_requested {
        return ExitReason::Killed;   // 主动停止优先级最高
    }
    match code {
        Some(0) => ExitReason::Normal,
        _ => ExitReason::Crashed,    // 非 0，或被杀拿不到码
    }
}
```

三条要点：

1. **主动停止优先**：即使进程恰好以 0 退出，只要调用过 `stop()` 就是 `killed`；
2. **`code == None` 归为 `crashed`**：拿不到退出码意味着非正常终止；
3. **标记先于杀进程**：`stop()` 先置 `stop_requested = true` 再调用 `kill_tree`，保证收尾任务判定时一定能看到该标记。

## SessionManager API 一览

全部为 `async`（除构造与纯查询）：

```rust
// 构造
SessionManager::from_home(backend: Arc<dyn ProcessBackend>, capture: CaptureConfig) -> Result<Self>
SessionManager::new(backend: Arc<dyn ProcessBackend>, capture: CaptureConfig, sessions_root: impl Into<PathBuf>) -> Self

// 路径
fn sessions_root(&self) -> &Path
fn session_dir(&self, session_id: &SessionId) -> PathBuf

// 生命周期
async fn start(&self, app: &AppConfig) -> Result<SessionHandle>
async fn stop(&self, session_id: SessionId) -> Result<()>
async fn restart(&self, session_id: SessionId, app: &AppConfig) -> Result<SessionHandle>

// 查询
async fn list_active(&self) -> Vec<SessionSnapshot>
async fn status(&self, session_id: SessionId) -> Result<SessionSnapshot>
async fn wait_for_exit(&self, session_id: SessionId, timeout: Duration) -> Result<ExitInfo>

// 重启后重关联
async fn recover_sessions(&self) -> Result<Vec<SessionSnapshot>>
```

### 数据结构

```rust
pub struct SessionHandle {
    pub session_id: SessionId,
    pub handle: ProcessHandle,   // { pid, start_time }
    pub snapshot: SessionSnapshot,
}

pub struct SessionSnapshot {
    pub session_id: SessionId,
    pub app_id: AppId,
    pub pid: u32,
    pub state: RunState,          // Running | Exited | Crashed
    pub started_at: String,       // RFC3339
    pub last_exit: Option<ExitInfo>,
    pub latest_status: Option<StatusSnapshot>,  // 最近一次采集到的窗口标题 / 监听端口
}

pub struct ExitInfo {
    pub code: Option<i32>,
    pub reason: ExitReason,       // Normal | Killed | Crashed
}
```

### 典型用法（M2 对接）

```rust
use std::sync::Arc;
use launcher_core::platform::default_process_backend;
use launcher_core::runtime::{CaptureConfig, SessionManager};

let manager = SessionManager::from_home(
    Arc::new(default_process_backend()),
    CaptureConfig::default(),
)?;

// 启动：返回后进程已在跑，输出由后台管线持续落盘
let session = manager.start(&app_config).await?;

// 停止：返回时整棵进程树已回收
manager.stop(session.session_id).await?;

// 查询
for snapshot in manager.list_active().await {
    println!("{} {:?} pid={}", snapshot.session_id, snapshot.state, snapshot.pid);
}
```

## 输出捕获策略

| 项 | 默认值 | 说明 |
| --- | --- | --- |
| channel 容量 | 8192 条事件 | 有界队列，防止内存无限增长 |
| 单行上限 | 1 MiB | 超长行被切成多条 `output` 事件，不丢内容 |
| flush 批量 | 每 64 条或每 100ms | 兼顾吞吐与时延 |
| 退出时 | 强制 flush + sync | 进程退出即可见完整日志 |

三条硬性保证：

1. **不阻塞子进程**：输出流用 `try_send`，失败只累计丢弃计数。若改成 await，子进程会因写满管道而卡死；
   （状态与退出事件走 `await send`，见上文「单 writer 所有权」——它们数量极少，丢掉才是真丢信息。）
2. **不静默丢行**：丢弃计数以 `overflow` 事件写入 JSONL；
3. **不因非法 UTF-8 丢行**：用 `read_until(b'\n')` 读原始字节再解码，而不是 `lines()`（后者遇非法 UTF-8 会整行丢弃）。

### 编码处理：严格解码 + 平台回退

读取仍是 `read_until(b'\n')` 拿**原始字节**，但解码不再是 `from_utf8_lossy` 一把梭，
而是逐行按固定顺序判定（实现见 `runtime::capture::decode_line_lossless`）：

| 步骤 | 判定 | 写入 `encoding` |
| --- | --- | --- |
| 1 | `std::str::from_utf8` 严格解码成功 | `utf-8` |
| 2 | 判定为超长行按字节切开的 UTF-8 残片（开头孤立续字节 / 结尾不完整序列） | `utf-8` 或 `lossy` |
| 3 | 按**平台回退编码**解码成功 —— Windows 用 `encoding_rs::GBK`（cp936） | `gbk` |
| 4 | 都失败 | `lossy`（有损，且显式标记） |

**为什么不用 lossy 兜底**：`from_utf8_lossy` 把每个非法字节替换成 U+FFFD，对 GBK 中文就是
当场销毁原始信息——`d6 d0 ce c4 b2 e2 ca d4`（「中文测试」）会变成一串替换字符，事后无法还原。
Windows 中文环境下大量 Python 项目（GPT-SoVITS、VITS、字幕工具）默认 cp936 输出，
用 lossy 兜底等于把这些会话的日志全部损坏，直接违反「只凭会话文件即可完整还原输出」的验收标准。
因此 lossy 只在**真正无法解码**时发生，并且必定带上 `encoding: "lossy"`，绝不静默损坏。

**为什么回退 GBK 而不是 cp1252**：GBK/GB18030 是简体中文环境的实际默认代码页，解出的是正确汉字；
cp1252 / latin-1 虽然「总能解码」，但产出西欧字母对中文日志毫无意义，且永远不会触发 lossy，
等于用另一种方式掩盖损坏。本项目只做与目标平台匹配的回退链。

Linux/macOS 暂无平台回退编码（系统默认 UTF-8），会直接进入 lossy 分支并如实标记。

## 状态采集（status 事件）

需求基线要求「新增状态项不改核心流程」。做法是把每个状态项做成一个独立的 `StatusProbe`，
由 `CompositeProbe` 组合：

```text
StatusProbe (trait)                     一个状态项 = 一个 probe
   ├── WindowTitleProbe    -> window_title   （EnumWindows + GetWindowTextW）
   ├── ListeningPortsProbe -> ports          （GetExtendedTcpTable）
   └── （未来）CpuProbe    -> 只实现 trait 并注册进组合，不改 SessionManager
CompositeProbe                          顺序执行，字段级合并，单项失败不影响其它
   └── StatusSnapshot                   { window_title, ports }
```

| 配置项 | 默认值 | 说明 |
| --- | --- | --- |
| `CaptureConfig::status_interval` | 3s | 采集间隔；`None` 或零表示**禁用**状态采集（不写任何 status 事件） |
| `CaptureConfig::max_status_events` | 12 次/分钟 | 滑动 60 秒窗口限流；`None` 表示不限 |

三条约定：

1. **变化才写**：与上一次采样结果比较，只有不同才追加 `status` 事件，避免日志刷屏；
2. **快照始终最新**：无论是否写事件，最新值都写进 `SessionSnapshot::latest_status`，
   限流只影响日志，不影响 GUI 读到的实时状态；
3. **失败静默**：probe 只返回字段值，不返回 `Result`、不写错误事件、绝不 panic
   （`CompositeProbe` 用 `catch_unwind` 兜底单个 probe 的 panic）。

窗口标题按「整棵进程树」判定，并在多个可见窗口里**按面积取最大者**（排除带
`WS_EX_TOOLWINDOW` 的工具窗口）：GUI 应用常见多个可见窗口，主窗口通常是面积最大的那个；
而启动器直接拉起的往往只是 shim，真正的窗口属于它的子进程。

### 状态循环的退出信号

状态循环有**两条**退出路径：

```text
每轮开头：会话 state 已不是 Running ──► 返回
等下一轮：tokio::select! {
             sleep(status_interval)  ──► 继续下一轮
             exit_signal.notified()  ──► 立刻返回      ← 正常路径走这条
          }
```

`exit_signal` 是一个 `Arc<tokio::sync::Notify>`，由会话收尾任务在**等进程退出之后立刻**
`notify_one`。为什么必须有它：如果只靠 `sleep` + 轮询 `RunState`，进程退出后循环仍要睡满
一个采集间隔才醒来，而收尾又要等循环结束才能写 `exit` —— 一次停止就要白等 3 秒
（默认间隔），集成测试因此从 ~2s 退化到 51s。

`DURATION_STATUS_SHUTDOWN_BUDGET`（5s）仍然保留，但**正常路径不会消耗它**：
它只兜住「probe 卡在系统调用里、来不及响应信号」的极端情况，超时即 abort，绝不泄漏 task。

## Windows 进程后端

### Job Object 生命周期

每次 `spawn` 都新建一个匿名 Job Object：

1. `CreateJobObjectW` 创建匿名 Job；
2. `SetInformationJobObject` 设置 `JOB_OBJECT_LIMIT_KILL_ON_JOB_CLOSE`；
3. `AssignProcessToJobObject` 把子进程分配进去（需要 `PROCESS_SET_QUOTA | PROCESS_TERMINATE` 权限）。

`KILL_ON_JOB_CLOSE` 的意义：**Job 句柄关闭时内核回收整棵树**。因此即使启动器崩溃、来不及调用 `kill_tree`，也不会有孤儿进程残留。

Windows 8+ 支持嵌套 Job，子进程自行创建 Job 不受影响。

### 句柄管理

所有 `HANDLE` 都由 `OwnedHandle` RAII 封装，`Drop` 时 `CloseHandle`，错误路径同样释放，不存在泄漏分支。

### PID 复用判定（attach）

`pid` 会被操作系统复用，因此**单凭 pid 无法确认进程身份**。`attach(pid, start_time)` 的判定：

```text
OpenProcess(SYNCHRONIZE | PROCESS_QUERY_LIMITED_INFORMATION)
   ├── 失败（进程不存在）──────────────► Ok(None)
   └── 成功 → GetProcessTimes 取创建时间
          ├── 与传入 start_time 不一致 ──► Ok(None)   ← PID 已被复用，绝不误认
          └── 一致 ─────────────────────► Ok(Some(handle))
```

**关键陷阱**：Windows 的进程对象在还有句柄被打开时会继续存在，即使进程已终止。因此「`OpenProcess` 成功」**不等于**进程存活——判断是否运行必须看 `WaitForSingleObject` 的信号状态：`WAIT_TIMEOUT` 才是运行中，`WAIT_OBJECT_0` 表示已终止。

### kill_tree

| 场景 | 做法 |
| --- | --- |
| 本进程 spawn 的（有缓存） | 先记录子孙 pid → `TerminateJobObject` → 等待整棵树消失 |
| 重启后重关联的（无 Job 句柄） | 校验 `start_time` → 用 ToolHelp 快照枚举子孙 → 逐个终止 |

为什么必须先记录子孙 pid：父进程一死，子进程会被系统重新挂到别的父进程下，之后再按父子关系枚举就**找不到它们**了，也就无法确认是否真的被回收。

## 已知限制

1. **重启后无法接管旧 stdout/stderr**：管道句柄随父进程消失，重关联只能恢复「进程是否还活着」的状态，无法继续读取输出。这些会话的日志会停在重启前的位置。
2. **重关联后的 kill_tree 是尽力而为**：拿不到原 Job 句柄，只能靠父子关系枚举；若进程中途改变父子关系（如双 fork），可能有遗漏。
4. **非 Windows 平台未实现**：`FallbackProcessBackend` 所有方法返回 `Unsupported`，macOS 对齐在 M4。

## 相关文档

- [SESSION_FORMAT.md](./SESSION_FORMAT.md)：JSONL 事件格式、轮转分片命名、overflow 与 recover 语义
- [CONFIG.md](./CONFIG.md)：应用配置 TOML 格式
- [REQUIREMENTS.md](./REQUIREMENTS.md)：需求与决策基线
