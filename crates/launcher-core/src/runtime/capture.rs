//! 会话日志的唯一写盘任务：所有事件 -> 有界 channel -> 串行写盘。
//!
//! 结构（每个输出流一条读者流水线，全部事件汇入**同一个**写入任务）：
//!
//! ```text
//! 子进程 stdout --reader task--+
//!                              |
//! 子进程 stderr --reader task--+--> mpsc(capacity) --> writer task --> events.jsonl
//!                              |
//! 状态循环 --Status------------+
//! 退出收尾 --Finish(exit)------+（写完 exit 即关闭文件并结束任务）
//! ```
//!
//! **单 writer 所有权**（这是硬约束，不是建议）：
//! 一个会话目录下同一时刻**只能有一个 LogWriter**。原因在于 `LogWriter` 把 `shard_seq`
//! 与已轮转分片列表保存在自己的内存里——多开一个 writer 就等于多一份「自己以为的分片现状」，
//! 序号会从 0 重新起算、保留策略会按不完整的列表删分片，结果旧分片永远删不掉、序号乱跳，
//! 长跑服务最终撑爆磁盘。因此除本模块的写入任务外，**任何地方都不得再 `LogWriter::open`
//! 同一个会话目录**；状态事件与 exit 事件都必须经 [`CaptureSink`] 排到同一条写入路径上。
//!
//! 三条不可让步的性质：
//!
//! 1. **绝不阻塞子进程**：输出行用 try_send，通道满时只累计丢弃计数并继续读管道。
//!    若改成 await send，子进程会因写满管道而卡死，等于用日志把应用冻住。
//!    （状态/退出事件数量极少，用 await send 换「绝不丢」，见 [`CaptureEvent`]。）
//! 2. **绝不静默丢行**：丢弃计数会以 overflow 事件写进 JSONL，读者能明确知道数据不完整。
//! 3. **不因非法 UTF-8 丢行**：用 read_until(b'\n') 读原始字节再 from_utf8_lossy，
//!    而不是 lines()（后者遇到非法 UTF-8 会直接返回错误并丢掉整行）。

use std::borrow::Cow;
use std::path::PathBuf;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::Arc;
use std::time::Duration;

use tokio::io::{AsyncBufReadExt, AsyncRead, BufReader};
use tokio::sync::{mpsc, oneshot};

use crate::error::{AppCatchError, Result};
use crate::runtime::log_writer::{LogWriter, LogWriterConfig};
use crate::session::{OutputEncoding, SessionEvent, StreamKind};

/// 默认的 channel 容量（行数）。
pub const DEFAULT_CHANNEL_CAPACITY: usize = 8192;

/// 单行最大字节数：超过就强制切断，避免超长行把内存吃穿。
pub const MAX_LINE_BYTES: usize = 1024 * 1024;

/// 默认的状态采集间隔：3 秒。
pub const DEFAULT_STATUS_INTERVAL: Duration = Duration::from_secs(3);

/// 默认的 status 事件速率上限：每分钟最多 12 条。
///
/// 正常情况远达不到：`status` 事件只在「状态真的变了」时写，
/// 一个来回切窗口的应用一分钟也就十几条。这个上限是异常保护——
/// 比如某个进程疯狂重绑端口，不能让日志被刷爆。
pub const DEFAULT_MAX_STATUS_EVENTS_PER_MINUTE: u32 = 12;

/// 捕获管线配置。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CaptureConfig {
    /// 有界 channel 容量（行数）。
    pub channel_capacity: usize,
    /// 日志文件配置（轮转、flush 策略）。
    pub writer: LogWriterConfig,
    /// 单行最大字节数，超出后强制切断为多条 output 事件。
    pub max_line_bytes: usize,
    /// 状态采集间隔；`None` 表示**禁用**状态采集循环（不写任何 status 事件）。
    ///
    /// 采集在 `spawn_blocking` 中执行，不占用 tokio worker；
    /// 只有状态真的变化时才追加 `status` 事件，详见 `runtime::status` 与 docs/RUNTIME.md。
    pub status_interval: Option<Duration>,
    /// 每分钟最多写多少条 `status` 事件；`None` 表示不限制。
    ///
    /// 规则：按滑动 60 秒窗口计数。达到上限后，该窗口内**状态变化不再写事件，
    /// 但仍会更新 `SessionSnapshot::latest_status`**——GUI 读到的快照始终是最新值，
    /// 只是日志被限流。
    pub max_status_events: Option<u32>,
}

impl Default for CaptureConfig {
    fn default() -> Self {
        Self {
            channel_capacity: DEFAULT_CHANNEL_CAPACITY,
            writer: LogWriterConfig::default(),
            max_line_bytes: MAX_LINE_BYTES,
            status_interval: Some(DEFAULT_STATUS_INTERVAL),
            max_status_events: Some(DEFAULT_MAX_STATUS_EVENTS_PER_MINUTE),
        }
    }
}

impl CaptureConfig {
    /// 状态采集是否启用（间隔为 `None` 或零时禁用）。
    pub fn status_enabled(&self) -> bool {
        matches!(self.status_interval, Some(interval) if !interval.is_zero())
    }
}

/// 去掉行尾换行后的原始字节（同时剥掉 `\r`，兼容 CRLF）。
pub fn strip_line_ending(raw: &[u8]) -> &[u8] {
    let mut end = raw.len();
    while end > 0 && (raw[end - 1] == b'\n' || raw[end - 1] == b'\r') {
        end -= 1;
    }
    &raw[..end]
}

/// 剥掉一行文本里的 ANSI 转义序列（CSI / OSC / 单字符 ESC）。
///
/// 为什么要在**落盘前**剥：子进程一旦不是 TTY（我们就是这种情况——标准流全被重定向成
/// 管道），理智的程序本该关掉彩色输出，但很多 CLI（vite / pnpm / concurrently / cargo）
/// 仍然无条件吐 ANSI 码。这些字节会原样进 events.jsonl，导致：
/// - 界面显示成 `[33m` 之类的残渣（真身是 `ESC[33m`）；
/// - 日志文件无法 grep，结构化事件里的文本字段被污染；
/// - 任何后续消费者（导出、搜索、别的 GUI）都得各自再剥一遍。
/// 因此在唯一入口剥一次，让落盘内容就是干净文本。
///
/// 处理三类序列（其余原样保留，宁可少剥也不误删正文）：
/// 1. CSI：`ESC [` + 参数（0x30-0x3F）* + 中间字节（0x20-0x2F）* + 终止字节（0x40-0x7E）。
///    覆盖颜色（`ESC[33m`）、光标移动、清屏等绝大多数控制序列。
/// 2. OSC：`ESC ]` ... 以 `BEL` 或 `ESC \` 结尾。用于设置终端标题等；
///    这类序列可能很长，若未遇到终止符则**丢弃到行尾**（残缺序列本来也无意义）。
/// 3. 其余 `ESC` + 单个字符（如 `ESC(B` 字符集切换、`ESC 7` 保存光标）。
///
/// 不含 `ESC` 的行零成本直接返回借用，不产生分配。
fn strip_ansi(text: Cow<'_, str>) -> Cow<'_, str> {
    if !text.contains('\u{1b}') {
        return text;
    }
    let bytes = text.as_bytes();
    let mut out: Vec<u8> = Vec::with_capacity(bytes.len());
    let mut i = 0;
    while i < bytes.len() {
        if bytes[i] != 0x1b {
            out.push(bytes[i]);
            i += 1;
            continue;
        }
        // ESC 出现在行尾：单独一个 ESC 没有意义，丢掉
        let Some(&next) = bytes.get(i + 1) else {
            break;
        };
        match next {
            b'[' => {
                // CSI：跳过参数与中间字节，直到终止字节
                let mut j = i + 2;
                while j < bytes.len() {
                    let b = bytes[j];
                    if (0x40..=0x7e).contains(&b) {
                        j += 1;
                        break;
                    }
                    // 参数区/中间区之外出现奇怪字节：当作序列在此中断，原地恢复
                    if !(0x20..=0x3f).contains(&b) {
                        break;
                    }
                    j += 1;
                }
                i = j;
            }
            b']' => {
                // OSC：直到 BEL 或 ST(ESC \)；找不到终止符就丢弃到行尾
                let mut j = i + 2;
                loop {
                    if j >= bytes.len() {
                        i = bytes.len();
                        break;
                    }
                    if bytes[j] == 0x07 {
                        i = j + 1;
                        break;
                    }
                    if bytes[j] == 0x1b && bytes.get(j + 1) == Some(&b'\\') {
                        i = j + 2;
                        break;
                    }
                    j += 1;
                }
            }
            // 单字符 ESC 序列：ESC + 一个字节（含 ESC(B 这类三字节的近似处理，
            // 多出的 '(' 'B' 会成为可见字符——但这种序列在应用输出里极罕见，
            // 宁可保留一个可读残渣也不冒险吞掉正文）
            _ => {
                i += 2;
            }
        }
    }
    // SAFETY 边界：只在 ASCII 控制字节处切分，未切断任何多字节字符，
    // 因此结果仍是合法 UTF-8。用 from_utf8_lossy 兜底而非 expect，
    // 避免任何未预料的输入让读日志这条路径 panic。
    match String::from_utf8(out) {
        Ok(clean) => Cow::Owned(clean),
        Err(e) => Cow::Owned(String::from_utf8_lossy(e.as_bytes()).into_owned()),
    }
}
/// 按「严格 UTF-8 → 平台回退编码 → lossy」的顺序解码一行，并给出编码标记。
///
/// 为什么不用 `from_utf8_lossy` 兜底：它会把 GBK 等非 UTF-8 字节静默替换成 U+FFFD，
/// 原始信息当场不可还原。Windows 中文环境下 Python 项目默认以 cp936(GBK) 输出，
/// 那等于把这类会话的日志全部损坏。
///
/// 判定顺序：
/// 1. `std::str::from_utf8` 严格解码成功 → `utf-8`（绝大多数行，零成本）；
/// 2. 失败则按平台回退编码解码：Windows 用 GBK/GB18030(cp936)，其余平台无回退 → `gbk`；
/// 3. 仍然失败才 `from_utf8_lossy` 兜底，**并显式标记 `lossy`**，绝不静默损坏。
pub fn decode_line_lossless(raw: &[u8]) -> (String, OutputEncoding) {
    let bytes = strip_line_ending(raw);

    // 1) 严格 UTF-8：合法即无损返回
    if let Ok(text) = std::str::from_utf8(bytes) {
        return (strip_ansi(Cow::Borrowed(text)).into_owned(), OutputEncoding::Utf8);
    }

    // 2) 超长行按字节切分留下的残片：必须按 UTF-8 保留，不能整体改判成 GBK
    //    （否则一条 UTF-8 中文长行被切开后，后半段会变成整段 GBK 乱码）
    if let Some((text, clean)) = decode_utf8_split_artifact(bytes) {
        let encoding = if clean {
            OutputEncoding::Utf8
        } else {
            OutputEncoding::Lossy
        };
        return (strip_ansi(Cow::Owned(text)).into_owned(), encoding);
    }

    // 3) 平台回退编码
    match decode_with_platform_fallback(bytes) {
        Some(text) => (strip_ansi(Cow::Owned(text)).into_owned(), OutputEncoding::Gbk),
        // 4) 都不行：lossy 兜底，但必须如实标记
        None => (
            strip_ansi(Cow::Owned(String::from_utf8_lossy(bytes).into_owned())).into_owned(),
            OutputEncoding::Lossy,
        ),
    }
}

/// 识别「超长行被按字节切开」留下的残片，并尽力按 UTF-8 还原。
///
/// 切分策略是按**字节**切的（见 [`read_lines`]），因此多字节字符可能被切断，
/// 残片有两种形态：
/// - 开头是若干孤立续字节（`0x80..=0xBF`）：上一个片段切掉了字符的前几字节；
/// - 结尾是不完整序列：剩余字节要留到下一个片段。
///
/// 只要「跳过开头孤立续字节后整段是合法 UTF-8」，或「只在结尾不完整」，
/// 就判定它是 UTF-8 残片而不是 GBK。判定失败（中间出现非法字节）则返回 `None`，
/// 交回给平台回退编码——真正的 GBK 行正是这种情况（如 `d6 d0` 在首位就不合法）。
///
/// 返回 `(文本, 是否完全无损)`：残片本身有字节损失，`clean == false` 时调用方标记 lossy。
fn decode_utf8_split_artifact(bytes: &[u8]) -> Option<(String, bool)> {
    let mut start = 0;
    while start < bytes.len() && (bytes[start] & 0xC0) == 0x80 {
        start += 1;
    }
    let rest = &bytes[start..];

    let (valid_len, truncated_tail) = match std::str::from_utf8(rest) {
        Ok(_) => (rest.len(), false),
        // 只在结尾不完整：合法的行被切开，尾部字节属于下一个片段
        Err(e) if e.error_len().is_none() => (e.valid_up_to(), true),
        // 中间就有非法字节：不是切分痕迹，交给平台回退编码判断
        Err(_) => return None,
    };

    let mut text = String::new();
    for _ in 0..start {
        text.push(char::REPLACEMENT_CHARACTER);
    }
    text.push_str(&String::from_utf8_lossy(&rest[..valid_len]));
    if truncated_tail {
        text.push(char::REPLACEMENT_CHARACTER);
    }

    Some((text, start == 0 && !truncated_tail))
}

/// 用平台回退编码解码；返回 `None` 表示该编码也解不了。
///
/// Windows 用 `encoding_rs::GBK`（即 GB18030 的超集实现，覆盖 cp936 全部字节组合）；
/// 其它平台暂无回退编码（系统默认是 UTF-8），直接交给 lossy 并标记。
#[cfg(windows)]
fn decode_with_platform_fallback(bytes: &[u8]) -> Option<String> {
    let (text, had_errors) = encoding_rs::GBK.decode_without_bom_handling(bytes);
    if had_errors {
        None
    } else {
        Some(text.into_owned())
    }
}

/// 非 Windows：没有平台回退编码，返回 `None` 走 lossy 分支并标记。
#[cfg(not(windows))]
fn decode_with_platform_fallback(_bytes: &[u8]) -> Option<String> {
    None
}

/// 去掉行尾换行后的一行文本（旧的公开入口，等价于严格解码后丢弃编码标记）。
///
/// 保留它是为了不破坏既有 API；捕获管线一律走 [`decode_line_lossless`]。
pub fn decode_line(raw: &[u8]) -> String {
    decode_line_lossless(raw).0
}

/// 从任意异步读取源逐行读取，每读一行调用一次 on_line，返回读到的总行数。
///
/// 超过 max_line_bytes 的行会被切成多条（truncated 只是给调用方留的信息，
/// 落盘时仍是独立的 output 事件），因此超长输出不会撑爆内存，也不会被吞掉。
///
/// 回调拿到的是「解码后的文本 + 该行的原始编码标记」，见 [`decode_line_lossless`]。
pub async fn read_lines<R, F>(reader: R, max_line_bytes: usize, mut on_line: F) -> Result<u64>
where
    R: AsyncRead + Unpin,
    F: FnMut(String, OutputEncoding),
{
    let mut reader = BufReader::new(reader);
    let mut buf: Vec<u8> = Vec::with_capacity(256);
    let limit = max_line_bytes.max(1);
    let mut total = 0u64;

    loop {
        // read_until 读到 \n 或 EOF 才返回：既不会拆行，也不会因非法 UTF-8 报错
        let read = reader
            .read_until(b'\n', &mut buf)
            .await
            .map_err(|e| AppCatchError::io(PathBuf::from("<pipe>"), e))?;

        if read == 0 {
            // EOF：子进程可能没写换行就退出，残留内容仍算最后一行
            if !buf.is_empty() {
                emit(&buf, &mut on_line);
                total += 1;
            }
            return Ok(total);
        }

        if buf.len() <= limit {
            emit(&buf, &mut on_line);
            total += 1;
            buf.clear();
            continue;
        }

        // 超长行：切出前 limit 字节作为一条输出，剩下的留到下一轮继续读。
        //
        // 切分位置严格等于 limit（策略不变）；若切点落在多字节字符中间，
        // 残片由 `decode_utf8_split_artifact` 识别为 UTF-8 残片而不是 GBK，
        // 断口处最多损失 1 个字符，但绝不整段误判成乱码。
        let rest = buf.split_off(limit);
        emit(&buf, &mut on_line);
        total += 1;
        buf = rest;
    }
}

/// 解码一行并回调（把「解码 + 发事件」的重复收拢成一处）。
fn emit<F>(raw: &[u8], on_line: &mut F)
where
    F: FnMut(String, OutputEncoding),
{
    let (text, encoding) = decode_line_lossless(raw);
    on_line(text, encoding);
}

/// 写入任务能处理的几类事件。
///
/// 把「内容」与「终止」放进同一个枚举，是为了让 **exit 一定是最后一条事件**：
/// 处理 `Finish` 之后写入任务立刻关闭文件并退出，之后再没有任何代码能往这个会话追加。
#[derive(Debug)]
enum CaptureEvent {
    /// 普通事件（output / overflow / status），按到达顺序落盘。
    Write {
        /// 要落盘的事件。
        event: SessionEvent,
        /// 可选回执：写入方可以借此等到「确实落盘」再继续（见 `CaptureSink::write`）。
        ack: Option<oneshot::Sender<()>>,
    },
    /// 收尾事件：写完它（可为 `None`，即只关闭不写内容）就结束写盘任务。
    Finish(Option<SessionEvent>),
}

/// 写盘权柄：整个会话唯一的日志写入入口。
///
/// 谁持有它谁就能往会话日志追加事件，但它只克隆 `Sender`，
/// 真正的 `LogWriter` 始终只存在于写盘任务内部——这就是「单 writer 所有权」的落地方式。
///
/// 两类发送语义：
/// - [`CaptureSink::try_write`]：**非阻塞**，给输出流用（通道满宁可丢并记账，也不阻塞子进程）；
/// - [`CaptureSink::write`] / [`CaptureSink::finish`]：**背压等待**，给状态与退出收尾用
///   （这些事件数量极少，丢掉才是真的丢信息）。
#[derive(Debug, Clone)]
pub struct CaptureSink {
    sender: mpsc::Sender<CaptureEvent>,
    dropped: Arc<AtomicU64>,
}

impl CaptureSink {
    /// 非阻塞写入：通道满或已关闭时只累计丢弃计数，返回是否成功入队。
    ///
    /// 输出流专用。**绝不能**改成 `await`——子进程会因为写满管道而卡死。
    pub fn try_write(&self, event: SessionEvent) -> bool {
        match self.sender.try_send(CaptureEvent::Write { event, ack: None }) {
            Ok(()) => true,
            // 通道满 / 写盘任务已结束：计入丢弃，由 overflow 事件如实反映
            Err(_) => {
                self.dropped.fetch_add(1, Ordering::Relaxed);
                false
            }
        }
    }

    /// 背压 + **落盘确认**写入：仅用于状态事件等极少量控制事件。
    ///
    /// 与 [`CaptureSink::try_write`] 的区别有两个，都是刻意的：
    /// - 用 `send` 而不是 `try_send`：这些事件量极小，宁可等一下也不能丢；
    /// - 带 `oneshot` 回执：写盘任务写完并 flush 后才返回，
    ///   因此「`write` 返回 true」等价于「此刻打开日志文件就能读到这条事件」。
    ///   重关联补写 status(running)、退出写 exit 都依赖这一点。
    ///
    /// 返回 `false` 表示写盘任务已经结束（会话正在收尾），事件没有落盘。
    pub async fn write(&self, event: SessionEvent) -> bool {
        let (ack, done) = oneshot::channel();
        if self
            .sender
            .send(CaptureEvent::Write {
                event,
                ack: Some(ack),
            })
            .await
            .is_err()
        {
            return false;
        }
        // 写盘任务已退出时会 drop 掉 ack，此时 recv 报错——同样算「没写成」
        done.await.is_ok()
    }

    /// 写入收尾事件，并让写盘任务关闭文件、退出循环。
    ///
    /// 这是会话日志的**唯一**收尾入口：调用之后（通道排空后）不会再有事件落盘，
    /// 因此 `exit` 必然位于事件流末尾。
    ///
    /// 返回 `false` 表示写盘任务早已结束（例如输出读取阶段就出错），收尾事件未落盘。
    pub async fn finish(&self, last_event: Option<SessionEvent>) -> bool {
        self.sender
            .send(CaptureEvent::Finish(last_event))
            .await
            .is_ok()
    }

    /// 构造一个「已经死掉」的写盘权柄：通道接收端已关闭，任何写入都会失败。
    ///
    /// 供不需要再写日志的会话记录占位（例如重关联判定为已崩溃的会话），
    /// 避免为了一个字段去伪造一条完整的捕获管线。
    pub fn dead() -> Self {
        let (sender, receiver) = mpsc::channel(1);
        // 接收端立刻关闭：此后任何 send 都会失败，不会凭空创建日志文件
        drop(receiver);
        Self {
            sender,
            dropped: Arc::new(AtomicU64::new(0)),
        }
    }

    /// 累计丢弃行数（实时值）。
    pub fn dropped_lines(&self) -> u64 {
        self.dropped.load(Ordering::Relaxed)
    }
}

/// 捕获管线：持有两条输出流的读取任务、写盘权柄与唯一的写盘任务。
///
/// 生命周期：[`CapturePipeline::finish`] 先等 reader 读完（通道随之自然关闭），
/// 再等写盘任务写完并 flush，返回统计信息；所有 JoinHandle 都在那里被 join，不会泄漏。
pub struct CapturePipeline {
    stdout_task: Option<tokio::task::JoinHandle<Result<u64>>>,
    stderr_task: Option<tokio::task::JoinHandle<Result<u64>>>,
    writer_task: Option<tokio::task::JoinHandle<Result<u64>>>,
    dropped: Arc<AtomicU64>,
    sink: CaptureSink,
}

impl CapturePipeline {
    /// 为一路输出启动「reader -> channel」。
    ///
    /// 通道满或接收端已关闭时只累计丢弃计数，绝不阻塞子进程。
    fn spawn_reader<R>(
        stream: StreamKind,
        reader: R,
        sender: mpsc::Sender<CaptureEvent>,
        dropped: Arc<AtomicU64>,
        config: CaptureConfig,
    ) -> tokio::task::JoinHandle<Result<u64>>
    where
        R: AsyncRead + Unpin + Send + 'static,
    {
        let sink = CaptureSink {
            sender,
            dropped: Arc::clone(&dropped),
        };
        tokio::spawn(async move {
            let max_line = config.max_line_bytes;
            read_lines(reader, max_line, |text, encoding| {
                // 非阻塞发送：通道满时只记账，绝不卡住子进程
                sink.try_write(SessionEvent::output_with_encoding(stream, text, encoding));
            })
            .await
        })
    }

    /// 启动唯一的写盘任务：从通道取事件，串行写入 LogWriter，并补写 overflow 事件。
    ///
    /// 退出条件只有两个：收到 [`CaptureEvent::Finish`]，或全部 `Sender` 被 drop（通道关闭）。
    /// 两条路径都会 flush + close，不存在「writer 还开着但没人管」的状态。
    fn spawn_writer(
        mut writer: LogWriter,
        mut receiver: mpsc::Receiver<CaptureEvent>,
        dropped: Arc<AtomicU64>,
    ) -> tokio::task::JoinHandle<Result<u64>> {
        tokio::spawn(async move {
            let mut written = 0u64;
            let mut reported = 0u64;

            // 把「截至此刻」的丢弃计数补写成 overflow 事件；返回写了多少条
            async fn report_overflow(
                writer: &mut LogWriter,
                dropped: &AtomicU64,
                reported: &mut u64,
            ) -> Result<u64> {
                let current = dropped.load(Ordering::Relaxed);
                if current <= *reported {
                    return Ok(0);
                }
                writer
                    .write_event(&SessionEvent::overflow(current - *reported))
                    .await?;
                *reported = current;
                Ok(1)
            }

            loop {
                let Some(item) = receiver.recv().await else {
                    // 通道关闭（所有 Sender 已 drop）：补记最后一段丢弃后收尾
                    written += report_overflow(&mut writer, &dropped, &mut reported).await?;
                    break;
                };

                match item {
                    CaptureEvent::Write { event, ack } => {
                        writer.write_event(&event).await?;
                        written += 1;
                        // 期间发生过丢弃就补写 overflow 事件，读者能明确知道数据不完整
                        written += report_overflow(&mut writer, &dropped, &mut reported).await?;

                        // 带 ack 的写入必须「确实落盘」后才回执，调用方据此获得可见性保证
                        if let Some(ack) = ack {
                            writer.flush().await?;
                            // 接收端已丢弃（调用方不再等）也无所谓：事件已经写进日志了
                            let _ = ack.send(());
                        }
                    }
                    CaptureEvent::Finish(last) => {
                        // 顺序不可换：先补 overflow，再写 exit —— exit 必须是最后一条
                        written += report_overflow(&mut writer, &dropped, &mut reported).await?;
                        if let Some(event) = last {
                            writer.write_event(&event).await?;
                            written += 1;
                        }
                        break;
                    }
                }
            }

            // 无论从哪条路径退出，都必须 flush + sync + close
            writer.close().await?;
            Ok(written)
        })
    }

    /// 组装一条完整管线并立即启动。
    pub fn start(
        stdout: Option<Box<dyn AsyncRead + Send + Unpin>>,
        stderr: Option<Box<dyn AsyncRead + Send + Unpin>>,
        writer: LogWriter,
        config: CaptureConfig,
    ) -> CapturePipeline {
        let capacity = config.channel_capacity.max(1);
        let (sender, receiver) = mpsc::channel::<CaptureEvent>(capacity);
        let dropped = Arc::new(AtomicU64::new(0));

        let writer_task = Self::spawn_writer(writer, receiver, Arc::clone(&dropped));

        // sink 保留一份在管线里（交给状态循环与退出收尾）；
        // 其余克隆交给读任务，全部 drop 后通道关闭，写盘任务自然收尾。
        let sink = CaptureSink {
            sender: sender.clone(),
            dropped: Arc::clone(&dropped),
        };

        let stdout_task = stdout.map(|reader| {
            Self::spawn_reader(
                StreamKind::Stdout,
                reader,
                sender.clone(),
                Arc::clone(&dropped),
                config.clone(),
            )
        });
        let stderr_task = stderr.map(|reader| {
            Self::spawn_reader(StreamKind::Stderr, reader, sender, Arc::clone(&dropped), config)
        });

        Self {
            stdout_task,
            stderr_task,
            writer_task: Some(writer_task),
            dropped,
            sink,
        }
    }

    /// 写盘权柄的克隆：状态循环、退出收尾都用它写事件，**不得**自己开 `LogWriter`。
    pub fn sink(&self) -> CaptureSink {
        self.sink.clone()
    }

    /// 累计丢弃行数（实时值）。
    pub fn dropped_lines(&self) -> u64 {
        self.dropped.load(Ordering::Relaxed)
    }

    /// 等待读取任务结束（两条流都 EOF），再等写盘任务写完并 flush。
    ///
    /// 所有 JoinHandle 都在这里被 join，不会泄漏。
    pub async fn finish(self) -> Result<CaptureSummary> {
        self.close_with(None).await
    }

    /// 收尾：读完输出 -> 把 `last_event` 排到队尾 -> 关闭文件 -> 等写盘任务退出。
    ///
    /// 这是「让 exit 成为日志最后一条」的唯一正确做法：
    /// 1. 先等两条 reader 结束——此刻起不会再有 `output` 进入通道；
    /// 2. 再发 [`CaptureEvent::Finish`]——它排在队列里所有已入队事件之后；
    /// 3. 写盘任务处理完 `Finish`（含补写 overflow、写 exit）立刻 close 并退出，
    ///    之后任何 `sink.write` 都会失败，物理上不可能再追加事件。
    ///
    /// 无输出管道的会话（重关联接管）同样走这里：reader 为 `None`，直接进入第 2 步。
    pub async fn close_with(mut self, last_event: Option<SessionEvent>) -> Result<CaptureSummary> {
        let mut stdout_lines = 0u64;
        let mut stderr_lines = 0u64;

        if let Some(task) = self.stdout_task.take() {
            stdout_lines = join_reader(task, "stdout").await?;
        }
        if let Some(task) = self.stderr_task.take() {
            stderr_lines = join_reader(task, "stderr").await?;
        }

        // reader 全部结束后，剩下的 Sender 只有 self.sink：先发收尾事件，
        // 再 drop 自己，保证写盘任务不会等一个永远不会到来的事件。
        let _ = self.sink.finish(last_event).await;
        drop(self.sink);

        let written_events = match self.writer_task.take() {
            Some(task) => join_reader(task, "writer").await?,
            None => 0,
        };

        Ok(CaptureSummary {
            stdout_lines,
            stderr_lines,
            written_events,
            dropped_lines: self.dropped.load(Ordering::Relaxed),
        })
    }
}

/// 等待一个任务结束并把 panic 折叠成可读错误。
async fn join_reader(task: tokio::task::JoinHandle<Result<u64>>, name: &str) -> Result<u64> {
    match task.await {
        Ok(Ok(lines)) => Ok(lines),
        Ok(Err(e)) => Err(e),
        Err(e) => Err(AppCatchError::Unsupported(format!(
            "{name} 任务异常结束: {e}"
        ))),
    }
}

/// 捕获结束后的统计。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct CaptureSummary {
    /// stdout 读到的行数。
    pub stdout_lines: u64,
    /// stderr 读到的行数。
    pub stderr_lines: u64,
    /// 实际写进 JSONL 的事件数（含 overflow 事件）。
    pub written_events: u64,
    /// 因 channel 满被丢弃的行数。
    pub dropped_lines: u64,
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::runtime::log_writer::LogWriterConfig;
    use std::time::Duration;

    fn fast_writer_config() -> LogWriterConfig {
        LogWriterConfig {
            max_file_bytes: 50 * 1024 * 1024,
            max_shards: 5,
            flush_every: 1,
            flush_interval: Duration::from_millis(0),
        }
    }

    /// 收集 read_lines 读到的全部行。
    async fn collect_lines(input: &[u8], max_line_bytes: usize) -> Vec<String> {
        collect_lines_and_encodings(input, max_line_bytes)
            .await
            .into_iter()
            .map(|(line, _)| line)
            .collect()
    }

    /// 收集 read_lines 读到的「文本 + 编码标记」。
    async fn collect_lines_and_encodings(
        input: &[u8],
        max_line_bytes: usize,
    ) -> Vec<(String, OutputEncoding)> {
        let mut lines = Vec::new();
        read_lines(input, max_line_bytes, |line, encoding| {
            lines.push((line, encoding))
        })
        .await
        .expect("读取失败");
        lines
    }

    #[tokio::test]
    async fn reads_plain_lines_without_line_ending() {
        let lines = collect_lines(b"alpha\nbeta\ngamma\n", MAX_LINE_BYTES).await;
        assert_eq!(lines, vec!["alpha", "beta", "gamma"]);
    }

    /// 最后一行没有换行也要读出来（子进程可能不写结尾换行就退出）。
    #[tokio::test]
    async fn reads_trailing_line_without_newline() {
        let lines = collect_lines(b"alpha\nbeta", MAX_LINE_BYTES).await;
        assert_eq!(lines, vec!["alpha", "beta"]);
    }

    /// ANSI 颜色码必须在落盘前被剥掉，否则界面显示成 `[33m` 残渣。
    ///
    /// 这是真实案例：vite 的 CJS 警告 `ESC[33m...ESC[39m` 曾被原样写进 events.jsonl。
    #[test]
    fn strips_ansi_color_codes() {
        let raw = "\u{1b}[33mThe CJS build of Vite is deprecated.\u{1b}[39m";
        let (text, encoding) = decode_line_lossless(raw.as_bytes());
        assert_eq!(text, "The CJS build of Vite is deprecated.");
        assert_eq!(encoding, OutputEncoding::Utf8);
        assert!(!text.contains('\u{1b}'), "不得残留 ESC: {text:?}");
    }

    /// 同一行里有多段颜色时全部剥掉，且正文顺序不变。
    #[test]
    fn strips_multiple_ansi_spans_in_one_line() {
        let raw = "\u{1b}[32mok\u{1b}[0m and \u{1b}[1;31mfail\u{1b}[0m";
        let (text, _) = decode_line_lossless(raw.as_bytes());
        assert_eq!(text, "ok and fail");
    }

    /// CSI 光标控制（如清行 ESC[2K）也属于转义序列，同样剥掉。
    #[test]
    fn strips_ansi_cursor_control() {
        let raw = "progress\u{1b}[2K\u{1b}[1Gdone";
        let (text, _) = decode_line_lossless(raw.as_bytes());
        assert_eq!(text, "progressdone");
    }

    /// OSC（设置终端标题）整段剥掉，包括 BEL 终止符。
    #[test]
    fn strips_ansi_osc_sequence() {
        let raw = "\u{1b}]0;window title\u{7}after";
        let (text, _) = decode_line_lossless(raw.as_bytes());
        assert_eq!(text, "after");
    }

    /// 没有 ESC 的行必须原样返回，且不得因剥 ANSI 而改变（含中文）。
    #[test]
    fn plain_text_is_untouched_by_ansi_stripping() {
        let (text, _) = decode_line_lossless("中文测试".as_bytes());
        assert_eq!(text, "中文测试");
    }

    /// 行尾只剩一个孤立 ESC 时不能 panic，正文保留。
    #[test]
    fn trailing_lone_esc_does_not_panic() {
        let (text, _) = decode_line_lossless("tail\u{1b}".as_bytes());
        assert_eq!(text, "tail");
    }

    /// CRLF 的行尾 \r 必须去掉，否则 Windows 输出会带上多余回车。
    #[tokio::test]
    async fn strips_carriage_return_from_crlf() {
        let lines = collect_lines(b"alpha\r\nbeta\r\n", MAX_LINE_BYTES).await;
        assert_eq!(lines, vec!["alpha", "beta"]);
    }

    /// 非法 UTF-8 不得丢行：行数与内容结构保持不变。
    ///
    /// 注意：这条测试在本次编码修复前断言「必须出现替换字符」。修正后语义变了——
    /// `0x80 0x81 0x6f` 其实是**合法 GBK**（0x80 是欧元符号，0x81 0x6f 是双字节字符），
    /// 因此现在会被无损解出并标记 `gbk`，不再出现替换字符。不丢行这条不变量保持不变。
    // GBK 回退只在 Windows 生效（见 decode_with_platform_fallback），其它平台按 UTF-8 + lossy 处理
    #[cfg(windows)]
    #[tokio::test]
    async fn invalid_utf8_does_not_drop_the_line() {
        let mut input: Vec<u8> = Vec::new();
        input.extend_from_slice(b"ok-before\n");
        input.extend_from_slice(&[0x66, 0x6f, 0x80, 0x81, 0x6f]); // "fo" + 非 UTF-8 字节
        input.extend_from_slice(b"\nok-after\n");

        let lines = collect_lines(&input, MAX_LINE_BYTES).await;
        assert_eq!(lines.len(), 3, "非法 UTF-8 行不得被丢弃: {lines:?}");
        assert_eq!(lines[0], "ok-before");
        assert!(lines[1].starts_with("fo"), "应保留可解码前缀: {lines:?}");
        assert_eq!(lines[2], "ok-after");

        // 该行不是合法 UTF-8，因此必须被标注为按 GBK 解码（而非静默损坏）
        let encodings = collect_lines_and_encodings(&input, MAX_LINE_BYTES).await;
        assert_eq!(encodings[0].1, OutputEncoding::Utf8);
        assert_eq!(
            encodings[1].1,
            OutputEncoding::Gbk,
            "非 UTF-8 但可被 GBK 解出的行应标记 gbk: {encodings:?}"
        );
        assert!(encodings[1].0.contains("\u{20AC}"), "0x80 在 GBK 中是欧元符号");
        assert!(
            !encodings[1].0.contains(char::REPLACEMENT_CHARACTER),
            "合法 GBK 不应产生替换字符: {encodings:?}"
        );
    }

    /// 「中文测试」的 GBK(cp936) 字节，Windows 中文环境控制台/日志的默认编码。
    const GBK_ZHONGWEN_CESHI: &[u8] = &[0xD6, 0xD0, 0xCE, 0xC4, 0xB2, 0xE2, 0xCA, 0xD4];

    /// 回归：GBK 字节必须无损还原并标记 gbk，绝不能被替换字符吃掉。
    // GBK 回退只在 Windows 生效（见 decode_with_platform_fallback），其它平台按 UTF-8 + lossy 处理
    #[cfg(windows)]
    #[test]
    fn decode_line_losslessly_decodes_gbk() {
        let (text, encoding) = decode_line_lossless(GBK_ZHONGWEN_CESHI);
        assert_eq!(text, "中文测试", "GBK 行应无损还原");
        assert_eq!(encoding, OutputEncoding::Gbk);
        assert!(
            !text.contains(char::REPLACEMENT_CHARACTER),
            "不得出现替换字符 U+FFFD"
        );
    }

    /// 合法的 UTF-8 行必须标记 utf-8，且不得被 GBK 分支误判。
    #[test]
    fn decode_line_losslessly_keeps_utf8() {
        let (text, encoding) = decode_line_lossless("中文测试".as_bytes());
        assert_eq!(text, "中文测试");
        assert_eq!(encoding, OutputEncoding::Utf8);
    }

    /// 两种编码都解不了时才 lossy，并且必须如实标记（绝不静默损坏）。
    #[test]
    fn decode_line_losslessly_marks_lossy_only_when_undecodable() {
        // 0x81 0x30 既不是合法 UTF-8，也不是合法 GBK（GBK 尾字节须 0x40..=0xFE）
        let (text, encoding) = decode_line_lossless(&[0x81, 0x30]);
        assert_eq!(encoding, OutputEncoding::Lossy);
        assert!(!text.is_empty(), "lossy 也要给出内容，不得丢行");
        assert!(text.contains(char::REPLACEMENT_CHARACTER));
    }

    /// CRLF 的行尾 \r 在新解码路径下仍要被剥掉。
    // GBK 回退只在 Windows 生效（见 decode_with_platform_fallback），其它平台按 UTF-8 + lossy 处理
    #[cfg(windows)]
    #[test]
    fn decode_line_losslessly_strips_crlf() {
        let mut bytes = GBK_ZHONGWEN_CESHI.to_vec();
        bytes.extend_from_slice(b"\r\n");
        let (text, encoding) = decode_line_lossless(&bytes);
        assert_eq!(text, "中文测试");
        assert_eq!(encoding, OutputEncoding::Gbk);
    }

    /// 超过 max_line_bytes 的行被切成多条，且内容不丢。
    #[tokio::test]
    async fn overlong_line_is_split_not_dropped() {
        let input = format!("{}\n", "z".repeat(25));
        let lines = collect_lines(input.as_bytes(), 10).await;
        assert!(lines.len() > 1, "超长行应被切开");
        assert_eq!(lines.concat(), "z".repeat(25), "切开后内容不得丢失");
    }

    /// 端到端：reader -> channel -> writer 全部落盘，行数一致。
    ///
    /// 同时验证超长行切分后每段仍各自完成编码判定（切分先于解码）。
    #[tokio::test]
    async fn pipeline_writes_every_line_to_disk() {
        let dir = tempfile::tempdir().expect("创建临时目录失败");
        let writer = LogWriter::open(dir.path(), fast_writer_config())
            .await
            .expect("打开写入器失败");

        let mut input: Vec<u8> = Vec::new();
        for i in 0..500 {
            input.extend_from_slice(format!("line-{i}\n").as_bytes());
        }

        let config = CaptureConfig {
            channel_capacity: 1024,
            writer: fast_writer_config(),
            max_line_bytes: MAX_LINE_BYTES,
            // 这几条测试只验证捕获管线本身，不启动状态采集循环
            status_interval: None,
            max_status_events: None,
        };
        let pipeline = CapturePipeline::start(
            Some(Box::new(std::io::Cursor::new(input))),
            None,
            writer,
            config,
        );
        let summary = pipeline.finish().await.expect("收尾失败");

        assert_eq!(summary.stdout_lines, 500);
        assert_eq!(summary.dropped_lines, 0, "容量足够时不应丢行");

        let text = std::fs::read_to_string(dir.path().join(crate::paths::SESSION_LOG_FILE))
            .expect("读日志失败");
        let count = text.lines().filter(|l| !l.is_empty()).count();
        assert_eq!(count, 500, "500 行应全部落盘");
    }

    /// channel 满 + 慢 writer：必须丢行（不阻塞），并且以 overflow 事件如实记录。
    ///
    /// 构造方式：容量 1 的 channel，writer 侧不消费直到 reader 读完，
    /// 于是 reader 必然遇到 Full 并累计丢弃。
    #[tokio::test]
    async fn overflow_is_recorded_when_channel_is_full() {
        let dropped = Arc::new(AtomicU64::new(0));
        let (sender, receiver) = mpsc::channel::<CaptureEvent>(1);

        // reader 侧：容量 1，writer 尚未消费，因此绝大多数行会被丢弃
        let mut input: Vec<u8> = Vec::new();
        for i in 0..200 {
            input.extend_from_slice(format!("flood-{i}\n").as_bytes());
        }

        let config = CaptureConfig {
            channel_capacity: 1,
            writer: fast_writer_config(),
            max_line_bytes: MAX_LINE_BYTES,
            // 这几条测试只验证捕获管线本身，不启动状态采集循环
            status_interval: None,
            max_status_events: None,
        };
        let reader_task = CapturePipeline::spawn_reader(
            StreamKind::Stdout,
            std::io::Cursor::new(input),
            sender,
            Arc::clone(&dropped),
            config,
        );

        // reader 读完前不消费 channel
        let read_lines_count = reader_task.await.expect("读任务失败").expect("读取出错");
        assert_eq!(read_lines_count, 200, "reader 不应因 channel 满而停止读管道");

        let dropped_now = dropped.load(Ordering::Relaxed);
        assert!(dropped_now > 0, "容量为 1 时应发生丢弃，实际 0");

        // writer 侧必须把丢弃如实写成 overflow 事件
        let dir = tempfile::tempdir().expect("创建临时目录失败");
        let writer = LogWriter::open(dir.path(), fast_writer_config())
            .await
            .expect("打开写入器失败");
        let writer_task = CapturePipeline::spawn_writer(writer, receiver, Arc::clone(&dropped));

        // reader 结束时 sender 已随之 drop，receiver 关闭，writer 会自然收尾
        let written = writer_task.await.expect("writer 任务失败").expect("写盘出错");
        assert!(written > 0);

        let text = std::fs::read_to_string(dir.path().join(crate::paths::SESSION_LOG_FILE))
            .expect("读日志失败");
        let mut overflow_total = 0u64;
        for line in text.lines().filter(|l| !l.is_empty()) {
            let event = SessionEvent::from_jsonl_line(line).expect("日志行应可解析");
            if let SessionEvent::Overflow { dropped, .. } = event {
                overflow_total += dropped;
            }
        }
        assert_eq!(
            overflow_total, dropped_now,
            "overflow 事件应如实记录丢弃行数"
        );
    }


    #[tokio::test]
    async fn pipeline_reports_per_stream_line_counts() {
        let dir = tempfile::tempdir().expect("创建临时目录失败");
        let writer = LogWriter::open(dir.path(), fast_writer_config())
            .await
            .expect("打开写入器失败");

        let config = CaptureConfig {
            channel_capacity: 256,
            writer: fast_writer_config(),
            max_line_bytes: MAX_LINE_BYTES,
            // 这几条测试只验证捕获管线本身，不启动状态采集循环
            status_interval: None,
            max_status_events: None,
        };
        let pipeline = CapturePipeline::start(
            Some(Box::new(std::io::Cursor::new(b"out-1\nout-2\n".to_vec()))),
            Some(Box::new(std::io::Cursor::new(b"err-1\n".to_vec()))),
            writer,
            config,
        );
        let summary = pipeline.finish().await.expect("收尾失败");

        assert_eq!(summary.stdout_lines, 2);
        assert_eq!(summary.stderr_lines, 1);
        assert_eq!(summary.written_events, 3);
        assert_eq!(summary.dropped_lines, 0);

        // stream 字段必须区分正确
        let text = std::fs::read_to_string(dir.path().join(crate::paths::SESSION_LOG_FILE))
            .expect("读日志失败");
        let mut stdout_count = 0;
        let mut stderr_count = 0;
        for line in text.lines().filter(|l| !l.is_empty()) {
            match SessionEvent::from_jsonl_line(line).expect("解析失败") {
                SessionEvent::Output { stream, .. } => match stream {
                    StreamKind::Stdout => stdout_count += 1,
                    StreamKind::Stderr => stderr_count += 1,
                },
                _ => {}
            }
        }
        assert_eq!(stdout_count, 2);
        assert_eq!(stderr_count, 1);
    }

    /// 超长行切分策略不变：切分发生在解码之前，且每段独立做编码判定。
    // GBK 回退只在 Windows 生效（见 decode_with_platform_fallback），其它平台按 UTF-8 + lossy 处理
    #[cfg(windows)]
    #[tokio::test]
    async fn overlong_gbk_line_is_split_before_decoding() {
        // 8 个字节 = 4 个汉字；每 4 字节切一段，正好每段 2 个汉字（都是合法 GBK）
        let mut input = Vec::new();
        input.extend_from_slice(GBK_ZHONGWEN_CESHI);
        input.extend_from_slice(b"\n");

        let pieces = collect_lines_and_encodings(&input, 4).await;
        assert!(pieces.len() > 1, "超长 GBK 行应被切开: {pieces:?}");
        assert_eq!(
            pieces.iter().map(|(t, _)| t.as_str()).collect::<String>(),
            "中文测试",
            "切开后内容不得丢失"
        );
        assert!(
            pieces.iter().all(|(_, e)| *e == OutputEncoding::Gbk),
            "每段都应标记 gbk: {pieces:?}"
        );
    }

    /// 64KB 以上的超长行：切分策略不变（不因新解码逻辑回归）。
    ///
    /// 切分发生在解码之前，按**字节**切，因此用 ASCII 验证「切了也不丢内容」；
    /// 多字节字符被切开时的行为另见下面的中文用例。
    #[tokio::test]
    async fn overlong_utf8_line_beyond_64k_is_split_without_loss() {
        let payload = "z".repeat(100 * 1024); // 100KB，超过 64KB
        let input = format!("{payload}\n");
        let pieces = collect_lines_and_encodings(input.as_bytes(), 64 * 1024).await;

        assert!(pieces.len() > 1, "超过 64KB 的行应被切开");
        assert_eq!(
            pieces.iter().map(|(t, _)| t.as_str()).collect::<String>(),
            payload,
            "切开后内容不得丢失"
        );
        assert!(
            pieces.iter().all(|(_, e)| *e == OutputEncoding::Utf8),
            "ASCII 片段应全部标记 utf-8"
        );
    }

    /// 中文超长行被按字节切开时可能切断多字节字符：仍不丢行，且如实标记。
    ///
    /// 这是既有切分策略（按字节）的固有行为，本次修复不改变它，只保证
    /// 「不丢行、不静默损坏」——断口处的残字节要么解出，要么标记 gbk/lossy。
    #[tokio::test]
    async fn overlong_multibyte_line_is_split_without_dropping_lines() {
        let payload = "汉".repeat(30_000); // 90KB
        let input = format!("{payload}\n");
        let pieces = collect_lines_and_encodings(input.as_bytes(), 64 * 1024).await;

        assert!(pieces.len() > 1, "超过 64KB 的行应被切开");
        // 切点落在汉字中间时该片段无法单独解码，但必须被标记且不丢片段
        assert!(
            pieces.iter().all(|(_, e)| matches!(
                e,
                OutputEncoding::Utf8 | OutputEncoding::Gbk | OutputEncoding::Lossy
            )),
            "每段都必须有合法编码标记"
        );
        // 切分严格按 limit 字节（策略不变），断口处最多损失 1 个字形，
        // 因此「完整汉字数」下界是 总数 - 断口数。
        let rejoined: String = pieces.iter().map(|(t, _)| t.as_str()).collect();
        let boundaries = pieces.len() - 1;
        let kept = rejoined.chars().filter(|c| *c == '汉').count();
        assert!(
            kept >= 30_000 - boundaries,
            "汉字丢失数不得超过断口数：kept={kept}, 断口={boundaries}"
        );
        // 关键回归：切分不得把 UTF-8 后半段误判成 GBK 乱码
        assert!(
            !pieces.iter().any(|(_, e)| *e == OutputEncoding::Gbk),
            "UTF-8 长行的切分残片不得被误判为 gbk: {:?}",
            pieces.iter().map(|(_, e)| e.as_str()).collect::<Vec<_>>()
        );
    }

    #[test]
    fn default_capture_config_matches_documented_values() {
        let config = CaptureConfig::default();
        assert_eq!(config.channel_capacity, 8192);
        assert_eq!(config.max_line_bytes, 1024 * 1024);
    }
}
