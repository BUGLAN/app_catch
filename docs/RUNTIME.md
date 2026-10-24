# 会话运行时（RUNTIME）

本文档面向 **M2 GUI 对接**：说明会话的生命周期状态机、`SessionManager` 公开 API、退出分类规则，以及日志轮转与溢出策略。

事件字段的逐字段定义见 [SESSION_FORMAT.md](./SESSION_FORMAT.md)，本文只讲「怎么用、为什么这样设计」。

## 分层

```text
SessionManager（会话门面，async）
   └── CapturePipeline（stdout/stderr 读取 → 有界 channel → 串行写盘）
          └── LogWriter（JSONL 追加、批量 flush、按体积轮转、保留 N 片）
   └── ProcessBackend（平台抽象：spawn / kill_tree / attach / is_running / wait）
          └── WindowsProcessBackend（Job Object + 启动时间校验）
```

`SessionManager` 只依赖 `Arc<dyn ProcessBackend>`，因此可替换实现（macOS 在 M4 接入）。

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
| channel 容量 | 8192 行 | 有界队列，防止内存无限增长 |
| 单行上限 | 1 MiB | 超长行被切成多条 `output` 事件，不丢内容 |
| flush 批量 | 每 64 条或每 100ms | 兼顾吞吐与时延 |
| 退出时 | 强制 flush + sync | 进程退出即可见完整日志 |

三条硬性保证：

1. **不阻塞子进程**：channel 满时用 `try_send`，失败只累计丢弃计数。若改成 await，子进程会因写满管道而卡死；
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
3. **窗口标题与监听端口未采集**：`status` 事件的 `window_title` / `ports` 当前恒为 `null`，留待 M1c。
4. **非 Windows 平台未实现**：`FallbackProcessBackend` 所有方法返回 `Unsupported`，macOS 对齐在 M4。

## 相关文档

- [SESSION_FORMAT.md](./SESSION_FORMAT.md)：JSONL 事件格式、轮转分片命名、overflow 与 recover 语义
- [CONFIG.md](./CONFIG.md)：应用配置 TOML 格式
- [REQUIREMENTS.md](./REQUIREMENTS.md)：需求与决策基线
