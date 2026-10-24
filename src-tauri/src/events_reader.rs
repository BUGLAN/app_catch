//! 会话事件读取：按分片顺序合并 + 增量游标 + 边写边读安全。
//!
//! 会话目录里的文件形态（见 `crates/launcher-core/src/runtime/log_writer.rs`）：
//!
//! ```text
//! <home>/sessions/<session_id>/
//!     events.1.jsonl   轮转出的历史分片（seq 升序，seq 越大越新）
//!     events.2.jsonl
//!     events.jsonl     当前活跃片（正在被写入）
//! ```
//!
//! 三条必须成立的性质：
//!
//! 1. **全局行号连续**：所有文件按「分片 seq 升序 → 活跃片最后」串起来，行号从 0 连续递增。
//!    游标 `from_line` 就是这个全局行号，因此跨文件读取对调用方是透明的。
//! 2. **边写边读不报错**：活跃片随时可能被追加。若末行还没写完整（没有行尾 `\n`），
//!    本轮**不消费**它，游标停在它前面，下一轮再读——绝不把半行交给解析器。
//! 3. **轮转竞态静默降级**：文件在「列目录」与「打开」之间被改名/删除时按空文件处理，
//!    不让一次竞态冒泡成界面错误。
//!
//! 性能策略（两级缓存，都是纯优化、不影响返回内容）：
//! - 已收尾分片不可变，其行数按「路径 + 长度」缓存，游标越过整片时 O(1) 跳过；
//! - 活跃片记录「上次读到 EOF 时的字节偏移 + 行号」，下一轮直接 seek 过去，
//!   避免每秒重新扫描整个活跃片。缓存带**文件创建时间**校验，
//!   轮转后同名新文件不会命中旧偏移。

use std::collections::HashMap;
use std::fs::File;
use std::io::{BufRead, BufReader, Seek, SeekFrom};
use std::path::{Path, PathBuf};
use std::sync::Mutex;
use std::time::SystemTime;

use serde::Serialize;

use launcher_core::error::AppCatchError;
use launcher_core::paths;
use launcher_core::runtime::log_writer::shard_file_name;
use launcher_core::session::SessionEvent;

use crate::error::CommandError;

/// 默认单次返回的最大事件条数（前端一轮轮询的预算）。
pub const DEFAULT_PAGE_LINES: u64 = 500;

/// 单次返回的事件条数硬上限，防止前端误传巨大值把内存打满。
pub const MAX_PAGE_LINES: u64 = 5000;

/// 分片行数缓存的最大条目数；超过就整体清空（缓存只是优化，清空永远安全）。
const SHARD_CACHE_CAPACITY: usize = 512;

/// 一次分页读取的结果。
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct SessionEventsPage {
    /// 本次读到的事件（按全局行号升序）。
    pub events: Vec<SessionEvent>,
    /// 下一次读取应使用的游标（= 已消费到的全局行号）。
    ///
    /// 正常情况下单调递增；若它**小于**调用方传入的游标，说明历史分片已被
    /// 保留策略裁剪、行号整体前移，调用方应就地重新对齐（见 docs/GUI.md「已知限制」）。
    pub next_line: u64,
    /// 是否还有未读完的事件（本轮被 `max_lines` 截断）。
    pub has_more: bool,
    /// 本轮因无法解析而跳过的行数（例如启动器被强杀留下的半行）。
    ///
    /// 单列出来是为了**不静默丢数据**：界面据此提示「有 N 行无法解析」。
    pub skipped_lines: u64,
}

/// 已收尾分片的行数缓存值。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
struct ShardLineIndex {
    /// 计算行数时的文件长度；长度变了就说明这份缓存不可信，需重算。
    len: u64,
    /// 该文件的行数。
    lines: u64,
}

/// 活跃片读取位置缓存值。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
struct ActiveCursor {
    /// 记录缓存时的文件创建时间；None 表示平台不提供，此时不写缓存。
    created: Option<SystemTime>,
    /// 该偏移处对应的全局行号。
    line: u64,
    /// 字节偏移（必然是某一行之后的边界）。
    offset: u64,
}

/// 会话事件读取器（线程安全，可放进 `tauri::State` 共享）。
///
/// 不持有长期文件句柄：每次读取都重新打开文件，
/// 因此会话结束后仍可继续用同一个实例做历史回放。
#[derive(Debug, Default)]
pub struct SessionEventReader {
    /// 已收尾分片的行数缓存（path -> 行数）。
    shard_index: Mutex<HashMap<PathBuf, ShardLineIndex>>,
    /// 活跃片的读取位置缓存（path -> 偏移）。
    active_cursor: Mutex<HashMap<PathBuf, ActiveCursor>>,
}

impl SessionEventReader {
    /// 构造一个空缓存的读取器。
    pub fn new() -> Self {
        Self {
            shard_index: Mutex::new(HashMap::new()),
            active_cursor: Mutex::new(HashMap::new()),
        }
    }

    /// 读取一页事件。
    ///
    /// `dir` 是会话目录；`from_line` 是全局行号游标；`max_lines` 是本轮条数上限
    /// （0 表示使用 [`DEFAULT_PAGE_LINES`]，超过 [`MAX_PAGE_LINES`] 时被夹到上限）。
    pub fn read_page(
        &self,
        dir: &Path,
        from_line: u64,
        max_lines: u64,
    ) -> Result<SessionEventsPage, CommandError> {
        let budget_limit = if max_lines == 0 {
            DEFAULT_PAGE_LINES
        } else {
            max_lines.min(MAX_PAGE_LINES)
        };

        let files = discover_log_files(dir)?;
        let last_index = files.len().saturating_sub(1);

        let mut line_no: u64 = 0;
        let mut events: Vec<SessionEvent> = Vec::new();
        let mut skipped_lines: u64 = 0;
        let mut budget = budget_limit;
        let mut has_more = false;
        let mut stop = false;

        for (index, path) in files.iter().enumerate() {
            if stop {
                break;
            }
            let is_active = index == last_index;

            // 整片跳过：已收尾分片不可变，缓存的行数足以证明整片都在游标之前
            if !is_active {
                if let Some(cached) = self.cached_shard_lines(path) {
                    if line_no + cached <= from_line {
                        line_no += cached;
                        continue;
                    }
                }
            }

            let file = match File::open(path) {
                Ok(file) => file,
                // 轮转竞态：列目录后被改名/删除，按「这片没有内容」处理
                Err(e) if e.kind() == std::io::ErrorKind::NotFound => continue,
                Err(e) => return Err(CommandError::from(AppCatchError::io(path, e))),
            };
            let meta = file.metadata().ok();
            let file_len = meta.as_ref().map(|m| m.len()).unwrap_or(0);
            let created = meta.as_ref().and_then(|m| m.created().ok());

            let start_line = line_no;
            let mut reader = BufReader::with_capacity(64 * 1024, file);

            // 活跃片：若上一轮留下了与当前行号对齐的 EOF 位置，直接 seek 过去，
            // 免去每秒重扫整个文件。创建时间不同（轮转出了同名新文件）则不命中。
            let mut offset: u64 = 0;
            if is_active && line_no <= from_line {
                if let Some(cursor) = self.active_cursor_of(path) {
                    let same_file = match (cursor.created, created) {
                        (Some(a), Some(b)) => a == b,
                        _ => false,
                    };
                    if same_file && cursor.line == line_no && cursor.offset <= file_len {
                        reader
                            .seek(SeekFrom::Start(cursor.offset))
                            .map_err(|e| CommandError::from(AppCatchError::io(path, e)))?;
                        offset = cursor.offset;
                    }
                }
            }

            let mut buf: Vec<u8> = Vec::with_capacity(256);
            // 是否读到了「干净的 EOF」（最后一行带换行符）
            let mut clean_eof = true;

            loop {
                buf.clear();
                let read = match reader.read_until(b'\n', &mut buf) {
                    Ok(read) => read,
                    Err(e) => return Err(CommandError::from(AppCatchError::io(path, e))),
                };
                if read == 0 {
                    break;
                }
                if buf.last() != Some(&b'\n') {
                    // 末行还没写完（边写边读）：本轮不消费，游标停在此行之前
                    clean_eof = false;
                    break;
                }
                offset += read as u64;

                // 去掉行尾换行与可能的 \r（兼容 CRLF）
                let mut end = buf.len() - 1;
                if end > 0 && buf[end - 1] == b'\r' {
                    end -= 1;
                }
                let raw = &buf[..end];

                if line_no < from_line {
                    line_no += 1;
                    continue;
                }
                if budget == 0 {
                    // 预算用完但确实还有未消费的行
                    has_more = true;
                    stop = true;
                    break;
                }

                budget -= 1;
                line_no += 1;

                // 空行占一个行号，但不产生事件（写盘方不会写空行，
                // 这里只兜住被强杀等情况留下的空行）
                let text = String::from_utf8_lossy(raw);
                let trimmed = text.trim();
                if trimmed.is_empty() {
                    continue;
                }

                match SessionEvent::from_jsonl_line(trimmed) {
                    Ok(event) => events.push(event),
                    // 无法解析的行（半行、被截断、格式演进）：计数但不中断本轮读取
                    Err(_) => skipped_lines += 1,
                }
            }

            if !clean_eof {
                // 末行不完整：后续内容顺序已不可信，本轮到此为止
                stop = true;
            } else if !stop {
                if is_active {
                    // 记住 EOF 位置：下一轮从这里 seek 继续
                    if let Some(created) = created {
                        self.remember_active_cursor(
                            path,
                            ActiveCursor {
                                created: Some(created),
                                line: line_no,
                                offset,
                            },
                        );
                    }
                } else {
                    // 只有完整扫到片尾才记行数，避免把「读了一半的分片」缓存成固定值
                    self.remember_shard_lines(path, file_len, line_no - start_line);
                }
            }
        }

        Ok(SessionEventsPage {
            events,
            next_line: line_no,
            has_more,
            skipped_lines,
        })
    }

    /// 查询已收尾分片的缓存行数（长度不匹配视为未命中）。
    fn cached_shard_lines(&self, path: &Path) -> Option<u64> {
        let len = std::fs::metadata(path).ok()?.len();
        let guard = lock_or_recover(&self.shard_index);
        guard
            .get(path)
            .filter(|cached| cached.len == len)
            .map(|cached| cached.lines)
    }

    /// 记录一个已完整扫描的分片行数。
    fn remember_shard_lines(&self, path: &Path, len: u64, lines: u64) {
        let mut guard = lock_or_recover(&self.shard_index);
        if guard.len() >= SHARD_CACHE_CAPACITY {
            // 缓存只是加速手段：满了一键清空即可，不需要 LRU
            guard.clear();
        }
        guard.insert(path.to_path_buf(), ShardLineIndex { len, lines });
    }

    /// 查询活跃片的读取位置缓存。
    fn active_cursor_of(&self, path: &Path) -> Option<ActiveCursor> {
        let guard = lock_or_recover(&self.active_cursor);
        guard.get(path).copied()
    }

    /// 记录活跃片的读取位置。
    fn remember_active_cursor(&self, path: &Path, cursor: ActiveCursor) {
        let mut guard = lock_or_recover(&self.active_cursor);
        if guard.len() >= SHARD_CACHE_CAPACITY {
            guard.clear();
        }
        guard.insert(path.to_path_buf(), cursor);
    }
}

/// 取锁；持锁线程 panic 过也能继续用（缓存不是关键数据）。
fn lock_or_recover<T>(mutex: &Mutex<T>) -> std::sync::MutexGuard<'_, T> {
    match mutex.lock() {
        Ok(guard) => guard,
        Err(poisoned) => poisoned.into_inner(),
    }
}

/// 列出会话目录下的事件文件，按「分片 seq 升序 → 活跃片最后」排序。
///
/// 目录不存在时返回空列表（会话刚创建、还没落盘的正常中间态）。
pub fn discover_log_files(dir: &Path) -> Result<Vec<PathBuf>, CommandError> {
    let entries = match std::fs::read_dir(dir) {
        Ok(entries) => entries,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(Vec::new()),
        Err(e) => return Err(CommandError::from(AppCatchError::io(dir, e))),
    };

    let mut shards: Vec<(u32, PathBuf)> = Vec::new();
    let mut active: Option<PathBuf> = None;

    for entry in entries {
        let entry = entry.map_err(|e| CommandError::from(AppCatchError::io(dir, e)))?;
        let name = entry.file_name().to_string_lossy().into_owned();

        if name == paths::SESSION_LOG_FILE {
            active = Some(entry.path());
            continue;
        }
        let Some(rest) = name.strip_prefix("events.") else {
            continue;
        };
        let Some(seq_text) = rest.strip_suffix(".jsonl") else {
            continue;
        };
        let Ok(seq) = seq_text.parse::<u32>() else {
            continue;
        };
        shards.push((seq, entry.path()));
    }

    // 必须按数字排序：字典序会把 events.10.jsonl 排到 events.2.jsonl 之前
    shards.sort_by_key(|(seq, _)| *seq);

    let mut files: Vec<PathBuf> = shards.into_iter().map(|(_, path)| path).collect();
    if let Some(active) = active {
        files.push(active);
    }
    Ok(files)
}

/// 会话目录下某个轮转分片的预期路径（供测试与文档示例复用）。
pub fn shard_path(dir: &Path, seq: u32) -> PathBuf {
    dir.join(shard_file_name(seq))
}
#[cfg(test)]
mod tests {
    use super::*;
    use launcher_core::session::{ExitReason, SessionId, StreamKind};
    use std::io::Write;

    /// 建一个临时会话目录，并在其中写入若干事件行。
    fn session_dir(events: &[(&str, Vec<SessionEvent>)]) -> tempfile::TempDir {
        let dir = tempfile::tempdir().expect("创建临时目录失败");
        for (name, list) in events {
            let path = dir.path().join(name);
            let mut file = File::create(&path).expect("创建事件文件失败");
            for event in list {
                writeln!(file, "{}", event.to_jsonl_line().expect("序列化失败"))
                    .expect("写入失败");
            }
            file.flush().expect("flush 失败");
        }
        dir
    }

    fn outputs(prefix: &str, count: usize) -> Vec<SessionEvent> {
        (0..count)
            .map(|i| SessionEvent::output(StreamKind::Stdout, format!("{prefix}{i}")))
            .collect()
    }

    /// 取出 output 事件的行内容，便于断言顺序。
    fn lines_of(page: &SessionEventsPage) -> Vec<String> {
        page.events
            .iter()
            .filter_map(|event| match event {
                SessionEvent::Output { line, .. } => Some(line.clone()),
                _ => None,
            })
            .collect()
    }

    /// 往活跃片追加若干行（模拟会话边跑边写）。
    fn append(path: &Path, events: &[SessionEvent]) {
        let mut file = std::fs::OpenOptions::new()
            .create(true)
            .append(true)
            .open(path)
            .expect("打开活跃片失败");
        for event in events {
            writeln!(file, "{}", event.to_jsonl_line().expect("序列化失败"))
                .expect("追加失败");
        }
        file.flush().expect("flush 失败");
    }

    #[test]
    fn shard_discovery_orders_numerically_then_active() {
        let dir = session_dir(&[
            ("events.2.jsonl", outputs("b", 1)),
            ("events.10.jsonl", outputs("c", 1)),
            ("events.1.jsonl", outputs("a", 1)),
            ("events.jsonl", outputs("z", 1)),
        ]);
        let files = discover_log_files(dir.path()).expect("列目录失败");
        let names: Vec<String> = files
            .iter()
            .map(|p| {
                p.file_name()
                    .map(|n| n.to_string_lossy().into_owned())
                    .unwrap_or_default()
            })
            .collect();
        assert_eq!(
            names,
            vec![
                "events.1.jsonl".to_string(),
                "events.2.jsonl".to_string(),
                "events.10.jsonl".to_string(),
                "events.jsonl".to_string(),
            ],
            "分片必须按 seq 数字升序，活跃片最后"
        );
    }

    #[test]
    fn shard_discovery_ignores_unrelated_files_and_missing_dir() {
        let dir = tempfile::tempdir().expect("创建临时目录失败");
        std::fs::write(dir.path().join("notes.md"), "无关").expect("写失败");
        std::fs::write(dir.path().join("events.1.jsonl.tmp-1"), "半截").expect("写失败");
        std::fs::write(dir.path().join("events.abc.jsonl"), "非法 seq").expect("写失败");

        let files = discover_log_files(dir.path()).expect("列目录失败");
        assert!(files.is_empty(), "非事件文件不应进入列表: {files:?}");

        // 会话目录还不存在时必须是空列表而不是错误
        let missing = dir.path().join("not-exist");
        let files = discover_log_files(&missing).expect("缺失目录不应报错");
        assert!(files.is_empty());
    }

    #[test]
    fn reads_all_files_in_global_order() {
        let dir = session_dir(&[
            ("events.1.jsonl", outputs("a", 3)),
            ("events.2.jsonl", outputs("b", 2)),
            ("events.jsonl", outputs("c", 1)),
        ]);
        let reader = SessionEventReader::new();
        let page = reader.read_page(dir.path(), 0, 100).expect("读取失败");

        assert_eq!(
            lines_of(&page),
            vec!["a0", "a1", "a2", "b0", "b1", "c0"],
            "跨分片必须按 分片升序 -> 活跃片 拼接"
        );
        assert_eq!(page.next_line, 6);
        assert!(!page.has_more);
        assert_eq!(page.skipped_lines, 0);
    }

    #[test]
    fn cursor_pages_through_shards_without_duplicates() {
        let dir = session_dir(&[
            ("events.1.jsonl", outputs("a", 3)),
            ("events.jsonl", outputs("b", 3)),
        ]);
        let reader = SessionEventReader::new();

        let first = reader.read_page(dir.path(), 0, 4).expect("第一页失败");
        assert_eq!(lines_of(&first), vec!["a0", "a1", "a2", "b0"]);
        assert_eq!(first.next_line, 4);
        assert!(first.has_more, "未读完应标记 has_more");

        let second = reader
            .read_page(dir.path(), first.next_line, 4)
            .expect("第二页失败");
        assert_eq!(lines_of(&second), vec!["b1", "b2"]);
        assert_eq!(second.next_line, 6);
        assert!(!second.has_more, "读完后 has_more 应为 false");

        // 再读一次：游标已到末尾，不应再吐事件也不应报错
        let third = reader
            .read_page(dir.path(), second.next_line, 4)
            .expect("第三页失败");
        assert!(third.events.is_empty());
        assert_eq!(third.next_line, 6);
        assert!(!third.has_more);
    }

    #[test]
    fn partial_trailing_line_is_not_consumed_until_complete() {
        let dir = tempfile::tempdir().expect("创建临时目录失败");
        let path = dir.path().join(paths::SESSION_LOG_FILE);
        let complete = SessionEvent::output(StreamKind::Stdout, "完整行")
            .to_jsonl_line()
            .expect("序列化失败");
        let pending = SessionEvent::output(StreamKind::Stdout, "尚未写完的行")
            .to_jsonl_line()
            .expect("序列化失败");

        // 先写一整行 + 半行（没有换行符），模拟「正在写」
        let mut file = File::create(&path).expect("创建失败");
        write!(file, "{complete}\n{pending}").expect("写入失败");
        file.flush().expect("flush 失败");
        drop(file);

        let reader = SessionEventReader::new();
        let first = reader.read_page(dir.path(), 0, 100).expect("读取失败");
        assert_eq!(lines_of(&first), vec!["完整行"], "半行不得被消费");
        assert_eq!(first.next_line, 1, "游标停在半行之前");
        assert_eq!(first.skipped_lines, 0, "半行不是「无法解析」，不应计数");

        // 写盘方补齐剩下的换行后，同一游标必须能读到它
        let mut file = std::fs::OpenOptions::new()
            .append(true)
            .open(&path)
            .expect("打开失败");
        write!(file, "\n").expect("补写失败");
        file.flush().expect("flush 失败");
        drop(file);

        let second = reader
            .read_page(dir.path(), first.next_line, 100)
            .expect("二次读取失败");
        assert_eq!(lines_of(&second), vec!["尚未写完的行"]);
        assert_eq!(second.next_line, 2);
    }

    /// 边跑边读：多轮增量轮询必须既不重复也不丢行。
    #[test]
    fn incremental_polling_sees_appended_lines_exactly_once() {
        let dir = tempfile::tempdir().expect("创建临时目录失败");
        let path = dir.path().join(paths::SESSION_LOG_FILE);
        append(&path, &outputs("a", 2));

        let reader = SessionEventReader::new();
        let first = reader.read_page(dir.path(), 0, 100).expect("首轮失败");
        assert_eq!(lines_of(&first), vec!["a0", "a1"]);
        assert_eq!(first.next_line, 2);

        // 会话继续写：命中活跃片偏移缓存，只读新增部分
        append(&path, &outputs("b", 3));
        let second = reader
            .read_page(dir.path(), first.next_line, 100)
            .expect("第二轮失败");
        assert_eq!(lines_of(&second), vec!["b0", "b1", "b2"]);
        assert_eq!(second.next_line, 5);

        // 没有新内容时必须是空页（幂等，不重复吐旧行）
        let third = reader
            .read_page(dir.path(), second.next_line, 100)
            .expect("第三轮失败");
        assert!(third.events.is_empty(), "无新增时不应重复返回旧行");
        assert_eq!(third.next_line, 5);

        // 再追加仍然正确
        append(&path, &outputs("c", 1));
        let fourth = reader
            .read_page(dir.path(), third.next_line, 100)
            .expect("第四轮失败");
        assert_eq!(lines_of(&fourth), vec!["c0"]);
        assert_eq!(fourth.next_line, 6);
    }

    /// 活跃片中的未写完末行不能让偏移缓存跳过头。
    #[test]
    fn active_cursor_does_not_skip_partial_tail() {
        let dir = tempfile::tempdir().expect("创建临时目录失败");
        let path = dir.path().join(paths::SESSION_LOG_FILE);
        append(&path, &outputs("a", 2));

        let reader = SessionEventReader::new();
        let first = reader.read_page(dir.path(), 0, 100).expect("首轮失败");
        assert_eq!(first.next_line, 2);

        // 追加一整行 + 半行
        let complete = SessionEvent::output(StreamKind::Stdout, "b0")
            .to_jsonl_line()
            .expect("序列化失败");
        let partial = SessionEvent::output(StreamKind::Stdout, "b1")
            .to_jsonl_line()
            .expect("序列化失败");
        let mut file = std::fs::OpenOptions::new()
            .append(true)
            .open(&path)
            .expect("打开失败");
        write!(file, "{complete}\n{partial}").expect("写入失败");
        file.flush().expect("flush 失败");
        drop(file);

        let second = reader
            .read_page(dir.path(), first.next_line, 100)
            .expect("第二轮失败");
        assert_eq!(lines_of(&second), vec!["b0"], "半行不得被消费");
        assert_eq!(second.next_line, 3);

        // 补齐换行后，第三个游标必须正好读到那一行
        let mut file = std::fs::OpenOptions::new()
            .append(true)
            .open(&path)
            .expect("打开失败");
        write!(file, "\n").expect("补写失败");
        file.flush().expect("flush 失败");
        drop(file);

        let third = reader
            .read_page(dir.path(), second.next_line, 100)
            .expect("第三轮失败");
        assert_eq!(lines_of(&third), vec!["b1"]);
        assert_eq!(third.next_line, 4);
    }

    /// 活跃片被轮转成 events.N.jsonl 后，同名新文件不能命中旧偏移。
    #[test]
    fn rotation_resets_active_cursor() {
        let dir = tempfile::tempdir().expect("创建临时目录失败");
        let active = dir.path().join(paths::SESSION_LOG_FILE);
        append(&active, &outputs("a", 4));

        let reader = SessionEventReader::new();
        let first = reader.read_page(dir.path(), 0, 100).expect("首轮失败");
        assert_eq!(first.next_line, 4);

        // 轮转：活跃片改名为 events.1.jsonl，新建空活跃片并写入新内容
        std::fs::rename(&active, shard_path(dir.path(), 1)).expect("改名失败");
        std::fs::write(&active, "").expect("创建新活跃片失败");
        append(&active, &outputs("b", 2));

        // 让新文件的创建时间明显不同于旧文件（FAT 等粗粒度文件系统上才有必要，
        // 这里只是确保测试在任意文件系统上都稳定）
        std::thread::sleep(std::time::Duration::from_millis(20));

        let second = reader
            .read_page(dir.path(), first.next_line, 100)
            .expect("轮转后读取失败");
        assert_eq!(
            lines_of(&second),
            vec!["b0", "b1"],
            "轮转后必须从头读新活跃片，不能被旧偏移带过头"
        );
        assert_eq!(second.next_line, 6, "全局行号应连续衔接分片");
    }

    #[test]
    fn unparseable_lines_are_counted_not_fatal() {
        let dir = tempfile::tempdir().expect("创建临时目录失败");
        let path = dir.path().join(paths::SESSION_LOG_FILE);
        let good = SessionEvent::output(StreamKind::Stderr, "正常行")
            .to_jsonl_line()
            .expect("序列化失败");
        std::fs::write(&path, format!("{good}\n{{not json}}\n{good}\n")).expect("写失败");

        let reader = SessionEventReader::new();
        let page = reader.read_page(dir.path(), 0, 100).expect("读取失败");
        assert_eq!(lines_of(&page), vec!["正常行", "正常行"]);
        assert_eq!(page.skipped_lines, 1, "坏行应被计数");
        assert_eq!(page.next_line, 3, "坏行同样占一个全局行号");
    }

    #[test]
    fn blank_lines_consume_index_but_produce_no_event() {
        let dir = tempfile::tempdir().expect("创建临时目录失败");
        let path = dir.path().join(paths::SESSION_LOG_FILE);
        let good = SessionEvent::output(StreamKind::Stdout, "x")
            .to_jsonl_line()
            .expect("序列化失败");
        std::fs::write(&path, format!("{good}\n\n   \n{good}\n")).expect("写失败");

        let reader = SessionEventReader::new();
        let page = reader.read_page(dir.path(), 0, 100).expect("读取失败");
        assert_eq!(page.events.len(), 2);
        assert_eq!(page.next_line, 4, "空行也占行号，保证游标与物理行一致");
        assert_eq!(page.skipped_lines, 0, "空行不算解析失败");
    }

    #[test]
    fn cursor_beyond_end_reports_actual_length_for_resync() {
        let dir = session_dir(&[("events.jsonl", outputs("a", 2))]);
        let reader = SessionEventReader::new();
        let page = reader.read_page(dir.path(), 99, 100).expect("读取失败");
        assert!(page.events.is_empty());
        assert_eq!(
            page.next_line, 2,
            "游标超界说明历史被裁剪，应回落到真实行数供前端重新对齐"
        );
        assert!(!page.has_more);
    }

    #[test]
    fn missing_session_dir_yields_empty_page() {
        let dir = tempfile::tempdir().expect("创建临时目录失败");
        let reader = SessionEventReader::new();
        let page = reader
            .read_page(&dir.path().join("ghost"), 0, 10)
            .expect("缺失会话目录不应报错");
        assert!(page.events.is_empty());
        assert_eq!(page.next_line, 0);
        assert!(!page.has_more);
    }

    /// 分片行数缓存必须只影响「跳过多少行」，绝不改变返回内容。
    #[test]
    fn shard_cache_does_not_change_results() {
        let dir = session_dir(&[
            ("events.1.jsonl", outputs("a", 4)),
            ("events.2.jsonl", outputs("b", 4)),
            ("events.jsonl", outputs("c", 4)),
        ]);
        let reader = SessionEventReader::new();

        // 第一次：冷缓存，扫描全量建立缓存
        let cold = reader.read_page(dir.path(), 0, 100).expect("冷读失败");
        assert_eq!(cold.events.len(), 12);

        // 第二次：热缓存，跨过前两片只读活跃片
        let warm = reader.read_page(dir.path(), 8, 100).expect("热读失败");
        assert_eq!(
            lines_of(&warm),
            vec!["c0", "c1", "c2", "c3"],
            "热缓存下按游标返回值必须一致"
        );
        assert_eq!(warm.next_line, 12);

        // 同一读取器对「从中间开始」的游标同样正确
        let middle = reader.read_page(dir.path(), 5, 100).expect("中段读取失败");
        assert_eq!(
            lines_of(&middle),
            vec!["b1", "b2", "b3", "c0", "c1", "c2", "c3"]
        );
    }

    #[test]
    fn supports_crlf_line_endings() {
        let dir = tempfile::tempdir().expect("创建临时目录失败");
        let path = dir.path().join(paths::SESSION_LOG_FILE);
        let line = SessionEvent::output(StreamKind::Stdout, "crlf")
            .to_jsonl_line()
            .expect("序列化失败");
        std::fs::write(&path, format!("{line}\r\n{line}\r\n")).expect("写失败");

        let reader = SessionEventReader::new();
        let page = reader.read_page(dir.path(), 0, 10).expect("读取失败");
        assert_eq!(lines_of(&page), vec!["crlf", "crlf"], "CRLF 应被正确剥离");
        assert_eq!(page.next_line, 2);
        assert_eq!(page.skipped_lines, 0);
    }

    #[test]
    fn zero_and_huge_limits_are_clamped() {
        let dir = session_dir(&[("events.jsonl", outputs("a", 8))]);
        let reader = SessionEventReader::new();

        // 0 -> 默认值（这里事件不足默认上限，应全部返回）
        let page = reader.read_page(dir.path(), 0, 0).expect("读取失败");
        assert_eq!(page.events.len(), 8);

        // 超过硬上限时被夹到 MAX_PAGE_LINES（本条只验证不 panic 且读到全部）
        let page = reader.read_page(dir.path(), 0, u64::MAX).expect("读取失败");
        assert_eq!(page.events.len(), 8);
    }

    /// 完整会话（start + output + exit）跨分片阅读后仍保持事件语义。
    #[test]
    fn mixed_event_kinds_survive_shard_merge() {
        let session_id = SessionId::new_v4();
        let dir = session_dir(&[
            (
                "events.1.jsonl",
                vec![
                    SessionEvent::start(
                        session_id,
                        "demo",
                        "cmd /c echo hi",
                        "C:/",
                        vec!["cmd".to_string()],
                        42,
                        1,
                        123,
                    ),
                    SessionEvent::output(StreamKind::Stdout, "first"),
                ],
            ),
            (
                "events.jsonl",
                vec![
                    SessionEvent::overflow(7),
                    SessionEvent::exit(Some(0), ExitReason::Normal),
                ],
            ),
        ]);

        let reader = SessionEventReader::new();
        let page = reader.read_page(dir.path(), 0, 100).expect("读取失败");
        assert_eq!(page.events.len(), 4);
        assert_eq!(page.events[0].event_name(), "start");
        assert_eq!(page.events[1].event_name(), "output");
        assert_eq!(page.events[2].event_name(), "overflow");
        assert_eq!(page.events[3].event_name(), "exit");
        match &page.events[2] {
            SessionEvent::Overflow { dropped, .. } => assert_eq!(*dropped, 7),
            other => panic!("应为 overflow 事件，实际: {other:?}"),
        }
        assert_eq!(page.next_line, 4);
    }
}
