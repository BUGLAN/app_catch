//! 会话日志写入器：JSONL 追加、批量 flush、按体积轮转、保留 N 个分片。
//!
//! 三条硬性约束：
//!
//! 1. **绝不切断一行**：一次写入的最小单位是「整行 JSON（含行尾换行）」，
//!    轮转只发生在两次写入之间，因此任何分片文件的最后一行都是完整可解析的。
//! 2. **批量 flush**：每 [`DEFAULT_FLUSH_EVERY`] 条或 [`DEFAULT_FLUSH_INTERVAL`] 触发一次
//!    flush + sync；会话结束时强制 flush，保证进程退出即可见完整日志。
//! 3. **轮转可追溯**：主文件恒为 `events.jsonl`（当前活跃片）；超限时把它改名为
//!    `events.<seq>.jsonl`（seq 从 1 递增）再新建空的主文件，并只保留最近 N 片，
//!    删除最旧的。删除失败只记录，不 panic，也不影响会话继续运行。

use std::path::{Path, PathBuf};
use std::time::{Duration, Instant};

use tokio::fs::{File, OpenOptions};
use tokio::io::AsyncWriteExt;

use crate::error::{AppCatchError, Result};
use crate::paths;
use crate::session::SessionEvent;

/// 默认单分片体积上限：50MB。
pub const DEFAULT_MAX_FILE_BYTES: u64 = 50 * 1024 * 1024;

/// 默认保留的分片数量上限（不含当前活跃的 `events.jsonl`）。
pub const DEFAULT_MAX_SHARDS: usize = 5;

/// 默认每写多少行触发一次 flush。
pub const DEFAULT_FLUSH_EVERY: usize = 64;

/// 默认最长多久触发一次 flush（避免低速率输出长时间不落盘）。
pub const DEFAULT_FLUSH_INTERVAL: Duration = Duration::from_millis(100);

/// 日志写入配置。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct LogWriterConfig {
    /// 单个分片的字节上限；超过即轮转。
    pub max_file_bytes: u64,
    /// 保留的历史分片数量（不含当前活跃片）。
    pub max_shards: usize,
    /// 每累计多少行 flush 一次。
    pub flush_every: usize,
    /// 距上次 flush 超过多久就强制 flush 一次。
    pub flush_interval: Duration,
}

impl Default for LogWriterConfig {
    fn default() -> Self {
        Self {
            max_file_bytes: DEFAULT_MAX_FILE_BYTES,
            max_shards: DEFAULT_MAX_SHARDS,
            flush_every: DEFAULT_FLUSH_EVERY,
            flush_interval: DEFAULT_FLUSH_INTERVAL,
        }
    }
}

/// 轮转分片的文件名：`events.<seq>.jsonl`。
pub fn shard_file_name(seq: u32) -> String {
    format!("events.{seq}.jsonl")
}

/// 会话日志写入器。
pub struct LogWriter {
    dir: PathBuf,
    config: LogWriterConfig,
    file: Option<File>,
    /// 当前活跃片已写入的字节数（用于判定轮转）。
    written_bytes: u64,
    /// 距上次 flush 的未落盘行数。
    pending_lines: usize,
    /// 上次 flush 的时间。
    last_flush: Instant,
    /// 已轮转出的分片数（决定下一个 seq）。
    shard_seq: u32,
    /// 历史分片文件名（按 seq 升序），用于保留策略。
    shards: Vec<String>,
    /// 累计写入的行数（含被轮转走的）。
    total_lines: u64,
}

impl std::fmt::Debug for LogWriter {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("LogWriter")
            .field("dir", &self.dir)
            .field("written_bytes", &self.written_bytes)
            .field("shard_seq", &self.shard_seq)
            .field("total_lines", &self.total_lines)
            .finish_non_exhaustive()
    }
}

impl LogWriter {
    /// 打开（或创建）会话目录下的主日志文件。
    pub async fn open(dir: impl Into<PathBuf>, config: LogWriterConfig) -> Result<Self> {
        let dir = dir.into();
        tokio::fs::create_dir_all(&dir)
            .await
            .map_err(|e| AppCatchError::io(&dir, e))?;
        let path = dir.join(paths::SESSION_LOG_FILE);
        let file = open_append(&path).await?;
        let written_bytes = file
            .metadata()
            .await
            .map(|m| m.len())
            .unwrap_or(0);

        Ok(Self {
            dir,
            config,
            file: Some(file),
            written_bytes,
            pending_lines: 0,
            last_flush: Instant::now(),
            shard_seq: 0,
            shards: Vec::new(),
            total_lines: 0,
        })
    }

    /// 当前活跃片路径。
    pub fn active_path(&self) -> PathBuf {
        self.dir.join(paths::SESSION_LOG_FILE)
    }

    /// 已经轮转出的分片数量。
    pub fn shard_count(&self) -> u32 {
        self.shard_seq
    }

    /// 累计写入行数。
    pub fn total_lines(&self) -> u64 {
        self.total_lines
    }

    /// 写入一条事件（自动序列化为单行并在需要时轮转/flush）。
    pub async fn write_event(&mut self, event: &SessionEvent) -> Result<()> {
        let line = event.to_jsonl_line()?;
        self.write_line(&line).await
    }

    /// 写入一行文本（不含行尾换行，内部补 `\n`）。
    ///
    /// 轮转判定发生在写入之前：若「当前片 + 本行」会超过上限，就先把当前片轮转走。
    /// 这样每片的最后一行永远是完整的。
    pub async fn write_line(&mut self, line: &str) -> Result<()> {
        let mut payload = String::with_capacity(line.len() + 1);
        payload.push_str(line);
        payload.push('\n');
        let bytes = payload.len() as u64;

        // 单行本身就超限时不做轮转（否则会无限轮转），直接写进当前片
        if self.written_bytes > 0
            && self.written_bytes + bytes > self.config.max_file_bytes
        {
            self.rotate().await?;
        }

        if self.file.is_none() {
            let path = self.active_path();
            self.file = Some(open_append(&path).await?);
            self.written_bytes = 0;
        }
        let Some(file) = self.file.as_mut() else {
            return Err(AppCatchError::Unsupported(
                "日志文件在轮转后未能重新打开".to_string(),
            ));
        };
        file.write_all(payload.as_bytes())
            .await
            .map_err(|e| AppCatchError::io(self.active_path(), e))?;

        self.written_bytes += bytes;
        self.total_lines += 1;
        self.pending_lines += 1;

        if self.pending_lines >= self.config.flush_every.max(1)
            || self.last_flush.elapsed() >= self.config.flush_interval
        {
            self.flush().await?;
        }
        Ok(())
    }

    /// flush + sync，把缓冲刷到磁盘。
    pub async fn flush(&mut self) -> Result<()> {
        // 先把路径取出来：file 的可变借用期间不能再借用 self
        let active = self.active_path();
        if let Some(file) = self.file.as_mut() {
            file.flush()
                .await
                .map_err(|e| AppCatchError::io(&active, e))?;
            // sync_all 让数据真正落到磁盘：进程被强杀后日志仍然完整
            file.sync_all()
                .await
                .map_err(|e| AppCatchError::io(&active, e))?;
        }
        self.pending_lines = 0;
        self.last_flush = Instant::now();
        Ok(())
    }

    /// 刷盘并关闭当前文件（会话结束时调用）。
    pub async fn close(&mut self) -> Result<()> {
        self.flush().await?;
        if let Some(mut file) = self.file.take() {
            file.shutdown()
                .await
                .map_err(|e| AppCatchError::io(self.active_path(), e))?;
        }
        Ok(())
    }

    /// 把当前活跃片改名为 `events.<seq>.jsonl`，并新建空的活跃片。
    async fn rotate(&mut self) -> Result<()> {
        // 先确保当前片落盘并关闭，否则 Windows 上 rename 会因占用失败
        self.flush().await?;
        if let Some(mut file) = self.file.take() {
            let _ = file.shutdown().await;
        }

        self.shard_seq += 1;
        let shard_name = shard_file_name(self.shard_seq);
        let shard_path = self.dir.join(&shard_name);
        let active = self.active_path();

        if let Err(e) = tokio::fs::rename(&active, &shard_path).await {
            // 改名失败：恢复写活跃片，避免整条会话日志中断
            self.file = Some(open_append(&active).await?);
            return Err(AppCatchError::io(&active, e));
        }
        self.shards.push(shard_name);
        self.written_bytes = 0;

        // 新建空的活跃片
        let file = open_truncate(&active).await?;
        self.file = Some(file);

        self.prune_shards().await;
        Ok(())
    }

    /// 只保留最近 `max_shards` 片，删除最旧的。删除失败仅忽略。
    async fn prune_shards(&mut self) {
        while self.shards.len() > self.config.max_shards {
            let oldest = self.shards.remove(0);
            let path = self.dir.join(&oldest);
            // 删除失败不影响会话继续写日志，最多多占一点磁盘
            let _ = tokio::fs::remove_file(&path).await;
        }
    }
}

/// 以追加模式打开文件（不存在则创建）。
async fn open_append(path: &Path) -> Result<File> {
    OpenOptions::new()
        .create(true)
        .append(true)
        .open(path)
        .await
        .map_err(|e| AppCatchError::io(path, e))
}

/// 以「创建或清空」模式打开文件。
async fn open_truncate(path: &Path) -> Result<File> {
    OpenOptions::new()
        .create(true)
        .write(true)
        .truncate(true)
        .open(path)
        .await
        .map_err(|e| AppCatchError::io(path, e))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::session::{ExitReason, SessionEvent, StreamKind};

    fn tiny_config(max_bytes: u64, max_shards: usize) -> LogWriterConfig {
        LogWriterConfig {
            max_file_bytes: max_bytes,
            max_shards,
            flush_every: 1,
            flush_interval: Duration::from_millis(0),
        }
    }

    /// 读一个文件的所有行，并断言每行都是可解析的 JSON。
    fn assert_lines_are_valid_json(path: &Path) -> usize {
        let text = std::fs::read_to_string(path).expect("读文件失败");
        let mut count = 0;
        for line in text.lines() {
            serde_json::from_str::<serde_json::Value>(line)
                .unwrap_or_else(|e| panic!("{} 存在不可解析的行: {e}\n{line}", path.display()));
            count += 1;
        }
        count
    }

    #[tokio::test]
    async fn writes_events_in_order_with_single_line_json() {
        let dir = tempfile::tempdir().expect("创建临时目录失败");
        let mut writer = LogWriter::open(dir.path(), tiny_config(1024 * 1024, 5))
            .await
            .expect("打开写入器失败");

        for i in 0..10 {
            writer
                .write_event(&SessionEvent::output(StreamKind::Stdout, format!("line {i}")))
                .await
                .expect("写入失败");
        }
        writer.close().await.expect("关闭失败");

        let path = dir.path().join(paths::SESSION_LOG_FILE);
        assert_eq!(assert_lines_are_valid_json(&path), 10);
        assert_eq!(writer.total_lines(), 10);
        assert_eq!(writer.shard_count(), 0, "未超限时不应轮转");
    }

    #[tokio::test]
    async fn rotates_when_size_exceeded_and_keeps_lines_intact() {
        let dir = tempfile::tempdir().expect("创建临时目录失败");
        // 每条事件约 100 字节，上限 300 字节 -> 约每 3 条轮转一次。
        // max_shards 取足够大：本测试只关心「轮转不丢行」，保留策略由 retention 测试覆盖。
        let mut writer = LogWriter::open(dir.path(), tiny_config(300, 50))
            .await
            .expect("打开写入器失败");

        for i in 0..30 {
            writer
                .write_event(&SessionEvent::output(
                    StreamKind::Stdout,
                    format!("padding line number {i} xxxxxxxxxxxxxxxxxxxxxxxx"),
                ))
                .await
                .expect("写入失败");
        }
        writer.close().await.expect("关闭失败");

        assert!(writer.shard_count() > 0, "应发生轮转");

        // 注意：shard_count() 是累计轮转次数，被保留策略删掉的旧分片不在磁盘上，
        // 因此这里按目录里真实存在的文件统计，而不是按 seq 从 1 数到 shard_count。
        let active = dir.path().join(paths::SESSION_LOG_FILE);
        let mut total = assert_lines_are_valid_json(&active);
        let mut existing_shards = Vec::new();
        for entry in std::fs::read_dir(dir.path()).expect("读目录失败").flatten() {
            let name = entry.file_name().to_string_lossy().into_owned();
            if name != paths::SESSION_LOG_FILE && name.starts_with("events.") {
                existing_shards.push(entry.path());
            }
        }
        assert!(!existing_shards.is_empty(), "应存在分片文件");
        for shard in &existing_shards {
            total += assert_lines_are_valid_json(shard);
        }
        // max_shards=50 足够容纳全部分片，因此一行都不该被删
        assert_eq!(total, 30, "轮转不得丢行或产生半行");
    }

    #[tokio::test]
    async fn retention_keeps_only_max_shards() {
        let dir = tempfile::tempdir().expect("创建临时目录失败");
        let mut writer = LogWriter::open(dir.path(), tiny_config(200, 3))
            .await
            .expect("打开写入器失败");

        for i in 0..60 {
            writer
                .write_event(&SessionEvent::output(
                    StreamKind::Stdout,
                    format!("retention padding {i} yyyyyyyyyyyyyyyyyyyyyyyy"),
                ))
                .await
                .expect("写入失败");
        }
        writer.close().await.expect("关闭失败");

        assert!(writer.shard_count() > 3, "应轮转出多于保留上限的分片");
        let mut existing = 0;
        for entry in std::fs::read_dir(dir.path()).expect("读目录失败").flatten() {
            let name = entry.file_name().to_string_lossy().into_owned();
            if name != paths::SESSION_LOG_FILE && name.starts_with("events.") {
                existing += 1;
            }
        }
        assert_eq!(existing, 3, "保留策略应只留 3 个分片，实际 {existing}");
    }

    /// 超长单行（本身超过分片上限）必须照常写入，且不得触发无限轮转。
    #[tokio::test]
    async fn single_line_larger_than_limit_is_still_written() {
        let dir = tempfile::tempdir().expect("创建临时目录失败");
        let mut writer = LogWriter::open(dir.path(), tiny_config(64, 2))
            .await
            .expect("打开写入器失败");

        let huge = "x".repeat(500);
        writer
            .write_event(&SessionEvent::output(StreamKind::Stdout, huge.clone()))
            .await
            .expect("超长行应可写入");
        writer.close().await.expect("关闭失败");

        let path = dir.path().join(paths::SESSION_LOG_FILE);
        assert_eq!(assert_lines_are_valid_json(&path), 1);
        let text = std::fs::read_to_string(&path).expect("读失败");
        assert!(text.contains(&huge), "超长行内容应完整保留");
    }

    #[tokio::test]
    async fn exit_event_round_trips_through_writer() {
        let dir = tempfile::tempdir().expect("创建临时目录失败");
        let mut writer = LogWriter::open(dir.path(), tiny_config(1024 * 1024, 5))
            .await
            .expect("打开写入器失败");
        writer
            .write_event(&SessionEvent::exit(Some(7), ExitReason::Crashed))
            .await
            .expect("写入失败");
        writer.close().await.expect("关闭失败");

        let text = std::fs::read_to_string(dir.path().join(paths::SESSION_LOG_FILE))
            .expect("读失败");
        // 每次构造事件都会取当前时间戳，因此不能与新建事件整体比较，只比对语义字段
        match SessionEvent::from_jsonl_line(text.trim()).expect("解析失败") {
            SessionEvent::Exit { v, code, reason, ts } => {
                assert_eq!(v, crate::session::SESSION_EVENT_VERSION);
                assert_eq!(code, Some(7));
                assert_eq!(reason, ExitReason::Crashed);
                assert!(!ts.is_empty(), "时间戳不应为空");
            }
            other => panic!("应为 exit 事件，实际: {other:?}"),
        }
    }

    #[test]
    fn shard_file_name_is_stable() {
        assert_eq!(shard_file_name(1), "events.1.jsonl");
        assert_eq!(shard_file_name(42), "events.42.jsonl");
    }

    #[test]
    fn default_config_matches_documented_limits() {
        // 与 docs/SESSION_FORMAT.md、docs/RUNTIME.md 中声明的默认值保持一致
        let config = LogWriterConfig::default();
        assert_eq!(config.max_file_bytes, 50 * 1024 * 1024);
        assert_eq!(config.max_shards, 5);
        assert_eq!(config.flush_every, 64);
        assert_eq!(config.flush_interval, Duration::from_millis(100));
    }
}
