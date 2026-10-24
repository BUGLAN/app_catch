//! 输出捕获管线：子进程 stdout/stderr -> 有界 channel -> 串行写盘。
//!
//! 结构（每个输出流一条流水线，两条流汇入同一个 writer）：
//!
//! ```text
//! 子进程 stdout --reader task--+
//!                              +--> mpsc(capacity) --> writer task --> events.jsonl
//! 子进程 stderr --reader task--+
//! ```
//!
//! 三条不可让步的性质：
//!
//! 1. **绝不阻塞子进程**：channel 满时用 try_send，失败只累计丢弃计数并继续读管道。
//!    若改成 await send，子进程会因写满管道而卡死，等于用日志把应用冻住。
//! 2. **绝不静默丢行**：丢弃计数会以 overflow 事件写进 JSONL，读者能明确知道数据不完整。
//! 3. **不因非法 UTF-8 丢行**：用 read_until(b'\n') 读原始字节再 from_utf8_lossy，
//!    而不是 lines()（后者遇到非法 UTF-8 会直接返回错误并丢掉整行）。

use std::path::PathBuf;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::Arc;

use tokio::io::{AsyncBufReadExt, AsyncRead, BufReader};
use tokio::sync::mpsc;

use crate::error::{AppCatchError, Result};
use crate::runtime::log_writer::{LogWriter, LogWriterConfig};
use crate::session::{SessionEvent, StreamKind};

/// 默认的 channel 容量（行数）。
pub const DEFAULT_CHANNEL_CAPACITY: usize = 8192;

/// 单行最大字节数：超过就强制切断，避免超长行把内存吃穿。
pub const MAX_LINE_BYTES: usize = 1024 * 1024;

/// 捕获管线配置。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CaptureConfig {
    /// 有界 channel 容量（行数）。
    pub channel_capacity: usize,
    /// 日志文件配置（轮转、flush 策略）。
    pub writer: LogWriterConfig,
    /// 单行最大字节数，超出后强制切断为多条 output 事件。
    pub max_line_bytes: usize,
}

impl Default for CaptureConfig {
    fn default() -> Self {
        Self {
            channel_capacity: DEFAULT_CHANNEL_CAPACITY,
            writer: LogWriterConfig::default(),
            max_line_bytes: MAX_LINE_BYTES,
        }
    }
}

/// 去掉行尾换行后的一行文本。
pub fn decode_line(raw: &[u8]) -> String {
    let mut end = raw.len();
    while end > 0 && (raw[end - 1] == b'\n' || raw[end - 1] == b'\r') {
        end -= 1;
    }
    String::from_utf8_lossy(&raw[..end]).into_owned()
}

/// 从任意异步读取源逐行读取，每读一行调用一次 on_line，返回读到的总行数。
///
/// 超过 max_line_bytes 的行会被切成多条（truncated 只是给调用方留的信息，
/// 落盘时仍是独立的 output 事件），因此超长输出不会撑爆内存，也不会被吞掉。
pub async fn read_lines<R, F>(reader: R, max_line_bytes: usize, mut on_line: F) -> Result<u64>
where
    R: AsyncRead + Unpin,
    F: FnMut(String),
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
                on_line(decode_line(&buf));
                total += 1;
            }
            return Ok(total);
        }

        if buf.len() <= limit {
            on_line(decode_line(&buf));
            total += 1;
            buf.clear();
            continue;
        }

        // 超长行：切出前 limit 字节作为一条输出，剩下的留到下一轮继续读
        let rest = buf.split_off(limit);
        on_line(decode_line(&buf));
        total += 1;
        buf = rest;
    }
}

/// 捕获管线：持有两条输出流的读取任务与唯一的写盘任务。
///
/// 生命周期：[`CapturePipeline::finish`] 等待 reader 读完、writer 写完并 flush，
/// 返回统计信息；所有 JoinHandle 都在那里被 join，不会泄漏。
pub struct CapturePipeline {
    stdout_task: Option<tokio::task::JoinHandle<Result<u64>>>,
    stderr_task: Option<tokio::task::JoinHandle<Result<u64>>>,
    writer_task: Option<tokio::task::JoinHandle<Result<u64>>>,
    dropped: Arc<AtomicU64>,
}

impl CapturePipeline {
    /// 为一路输出启动「reader -> channel」。
    ///
    /// 通道满或接收端已关闭时只累计丢弃计数，绝不阻塞子进程。
    pub fn spawn_reader<R>(
        stream: StreamKind,
        reader: R,
        sender: mpsc::Sender<SessionEvent>,
        dropped: Arc<AtomicU64>,
        config: CaptureConfig,
    ) -> tokio::task::JoinHandle<Result<u64>>
    where
        R: AsyncRead + Unpin + Send + 'static,
    {
        tokio::spawn(async move {
            let max_line = config.max_line_bytes;
            read_lines(reader, max_line, |text| {
                let event = SessionEvent::output(stream, text);
                match sender.try_send(event) {
                    Ok(()) => {}
                    // 通道满：累计丢弃并继续读。若这里 await，子进程会被写满的管道卡死。
                    Err(mpsc::error::TrySendError::Full(_)) => {
                        dropped.fetch_add(1, Ordering::Relaxed);
                    }
                    // 接收端已关闭（会话正在收尾）：同样计入丢弃
                    Err(mpsc::error::TrySendError::Closed(_)) => {
                        dropped.fetch_add(1, Ordering::Relaxed);
                    }
                }
            })
            .await
        })
    }

    /// 启动写盘任务：从 channel 取事件，串行写入 LogWriter，并补写 overflow 事件。
    pub fn spawn_writer(
        mut writer: LogWriter,
        mut receiver: mpsc::Receiver<SessionEvent>,
        dropped: Arc<AtomicU64>,
    ) -> tokio::task::JoinHandle<Result<u64>> {
        tokio::spawn(async move {
            let mut written = 0u64;
            let mut reported = 0u64;

            while let Some(event) = receiver.recv().await {
                writer.write_event(&event).await?;
                written += 1;

                // 期间发生过丢弃就补写 overflow 事件，读者能明确知道数据不完整
                let current = dropped.load(Ordering::Relaxed);
                if current > reported {
                    writer.write_event(&SessionEvent::overflow(current - reported)).await?;
                    written += 1;
                    reported = current;
                }
            }

            // channel 关闭后再补一次，确保最后一段丢弃也被记录
            let current = dropped.load(Ordering::Relaxed);
            if current > reported {
                writer.write_event(&SessionEvent::overflow(current - reported)).await?;
                written += 1;
            }

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
        let (sender, receiver) = mpsc::channel::<SessionEvent>(capacity);
        let dropped = Arc::new(AtomicU64::new(0));

        let writer_task = Self::spawn_writer(writer, receiver, Arc::clone(&dropped));

        // sender 的两份克隆分别交给两个读任务，任务结束后自动 drop，channel 随之关闭，
        // writer 读到 None 即收尾 —— 这是整条管线唯一的退出路径。
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
        }
    }

    /// 累计丢弃行数（实时值）。
    pub fn dropped_lines(&self) -> u64 {
        self.dropped.load(Ordering::Relaxed)
    }

    /// 等待读取任务结束（两条流都 EOF），再等 writer 写完并 flush。
    ///
    /// 所有 JoinHandle 都在这里被 join，不会泄漏。
    pub async fn finish(mut self) -> Result<CaptureSummary> {
        let mut stdout_lines = 0u64;
        let mut stderr_lines = 0u64;

        if let Some(task) = self.stdout_task.take() {
            stdout_lines = join_reader(task, "stdout").await?;
        }
        if let Some(task) = self.stderr_task.take() {
            stderr_lines = join_reader(task, "stderr").await?;
        }

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
        let mut lines = Vec::new();
        read_lines(input, max_line_bytes, |line| lines.push(line))
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

    /// CRLF 的行尾 \r 必须去掉，否则 Windows 输出会带上多余回车。
    #[tokio::test]
    async fn strips_carriage_return_from_crlf() {
        let lines = collect_lines(b"alpha\r\nbeta\r\n", MAX_LINE_BYTES).await;
        assert_eq!(lines, vec!["alpha", "beta"]);
    }

    /// 非法 UTF-8 不得丢行：用替换字符兜住，行数与内容结构保持不变。
    #[tokio::test]
    async fn invalid_utf8_does_not_drop_the_line() {
        let mut input: Vec<u8> = Vec::new();
        input.extend_from_slice(b"ok-before\n");
        input.extend_from_slice(&[0x66, 0x6f, 0x80, 0x81, 0x6f]); // "fo<非法><非法>o"
        input.extend_from_slice(b"\nok-after\n");

        let lines = collect_lines(&input, MAX_LINE_BYTES).await;
        assert_eq!(lines.len(), 3, "非法 UTF-8 行不得被丢弃: {lines:?}");
        assert_eq!(lines[0], "ok-before");
        assert!(lines[1].starts_with("fo"), "应保留可解码前缀: {lines:?}");
        assert!(lines[1].contains('\u{FFFD}'), "应以替换字符表示非法字节");
        assert_eq!(lines[2], "ok-after");
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
        let (sender, receiver) = mpsc::channel::<SessionEvent>(1);

        // reader 侧：容量 1，writer 尚未消费，因此绝大多数行会被丢弃
        let mut input: Vec<u8> = Vec::new();
        for i in 0..200 {
            input.extend_from_slice(format!("flood-{i}\n").as_bytes());
        }

        let config = CaptureConfig {
            channel_capacity: 1,
            writer: fast_writer_config(),
            max_line_bytes: MAX_LINE_BYTES,
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

    #[test]
    fn default_capture_config_matches_documented_values() {
        let config = CaptureConfig::default();
        assert_eq!(config.channel_capacity, 8192);
        assert_eq!(config.max_line_bytes, 1024 * 1024);
    }
}
