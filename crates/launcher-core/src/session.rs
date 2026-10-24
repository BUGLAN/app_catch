//! 会话事件模型（需求基线 F3 的草案，M1a 只定义类型与序列化）。
//!
//! 关键约定：
//! - 每次启动 = 一个 `session_id`，一个会话目录，一个 JSONL 事件流；
//! - **每条事件自带 `v` 版本字段**，使事件流自描述：后续格式演进时，读取方可以按行判断版本；
//! - 序列化结果是**单行 JSON**（[`SessionEvent::to_jsonl_line`]）：不得包含换行符，
//!   否则一条事件会被拆成两行，破坏 JSONL 的行语义；
//! - 时间戳统一为 RFC3339 字符串（UTC，带 `Z` 后缀），由 [`now_rfc3339`] 生成。
//!
//! 本模块不做任何写盘：真实的追加写入、轮转、溢出处理在 M1b 落地。

use std::fmt;
use std::str::FromStr;

use serde::{Deserialize, Serialize};
use time::format_description::well_known::Rfc3339;
use time::OffsetDateTime;
use uuid::Uuid;

use crate::error::{AppCatchError, Result};

/// 当前事件格式版本。所有事件都写这个值；演进规则见 `docs/SESSION_FORMAT.md`。
pub const SESSION_EVENT_VERSION: u32 = 1;

/// 生成当前 UTC 时间的 RFC3339 字符串，例如 `2026-10-24T18:30:00.123456789Z`。
///
/// 只在极端情况下（系统时间早于 UNIX 纪元且库无法格式化）回退为空字符串，
/// 不让格式化失败中断启动流程。
pub fn now_rfc3339() -> String {
    OffsetDateTime::now_utc()
        .format(&Rfc3339)
        .unwrap_or_else(|_| String::new())
}

/// 会话标识：uuid v4，可读性上按标准连字符形式展示。
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct SessionId(Uuid);

impl SessionId {
    /// 生成一个新的 v4 会话 id。
    pub fn new_v4() -> Self {
        Self(Uuid::new_v4())
    }

    /// 借用底层 uuid，便于与其它库互操作。
    pub fn as_uuid(&self) -> Uuid {
        self.0
    }
}

impl Default for SessionId {
    fn default() -> Self {
        Self::new_v4()
    }
}

impl fmt::Display for SessionId {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}", self.0)
    }
}

impl FromStr for SessionId {
    type Err = AppCatchError;

    fn from_str(s: &str) -> Result<Self> {
        Uuid::parse_str(s).map(Self).map_err(|e| AppCatchError::InvalidId {
            value: s.to_string(),
            reason: format!("不是合法的 uuid: {e}"),
        })
    }
}

impl From<Uuid> for SessionId {
    fn from(value: Uuid) -> Self {
        Self(value)
    }
}

impl Serialize for SessionId {
    fn serialize<S: serde::Serializer>(&self, serializer: S) -> std::result::Result<S::Ok, S::Error> {
        serializer.serialize_str(&self.0.to_string())
    }
}

impl<'de> Deserialize<'de> for SessionId {
    fn deserialize<D: serde::Deserializer<'de>>(
        deserializer: D,
    ) -> std::result::Result<Self, D::Error> {
        let raw = String::deserialize(deserializer)?;
        raw.parse().map_err(serde::de::Error::custom)
    }
}

/// 输出流类型：stdout / stderr。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum StreamKind {
    /// 标准输出。
    Stdout,
    /// 标准错误。
    Stderr,
}

impl StreamKind {
    /// 序列化后的字符串形式（与 JSON 中的取值一致）。
    pub fn as_str(&self) -> &'static str {
        match self {
            StreamKind::Stdout => "stdout",
            StreamKind::Stderr => "stderr",
        }
    }
}

/// `output` 事件的默认编码标记。
///
/// 旧日志（M1b 之前）没有 `encoding` 字段，读取时一律按 UTF-8 解读：
/// 那时捕获层用的是 `from_utf8_lossy`，非 UTF-8 字节已经被替换成 U+FFFD，
/// 因此「按 utf-8 解读」是对旧数据唯一诚实的默认值。
pub const DEFAULT_OUTPUT_ENCODING: &str = "utf-8";

/// 一行输出的原始编码标记。
///
/// 值域与 JSON 中的字符串一致：`"utf-8"` / `"gbk"` / `"lossy"`。
/// - `utf-8`：原始字节本身是合法 UTF-8，无损；
/// - `gbk`：原始字节不是合法 UTF-8，但按 GBK/GB18030（cp936）无损解码成功；
/// - `lossy`：两种解码都失败，内容已被替换字符替代，**原始字节不可还原**。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum OutputEncoding {
    /// 合法 UTF-8，无损解码。
    #[serde(rename = "utf-8")]
    Utf8,
    /// 按 GBK/GB18030（cp936）无损解码。
    #[serde(rename = "gbk")]
    Gbk,
    /// 无法解码，已用替换字符兜底（有损）。
    #[serde(rename = "lossy")]
    Lossy,
}

impl OutputEncoding {
    /// 序列化后的字符串形式（与 JSON 中的取值一致）。
    pub fn as_str(&self) -> &'static str {
        match self {
            OutputEncoding::Utf8 => "utf-8",
            OutputEncoding::Gbk => "gbk",
            OutputEncoding::Lossy => "lossy",
        }
    }
}

impl Default for OutputEncoding {
    fn default() -> Self {
        Self::Utf8
    }
}

impl fmt::Display for OutputEncoding {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(self.as_str())
    }
}

/// 退出原因：正常退出 / 被停止（含整棵进程树被回收）/ 崩溃。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum ExitReason {
    /// 进程主动正常退出（退出码可为 0 或非 0）。
    Normal,
    /// 被启动器或外部信号杀死。
    Killed,
    /// 异常终止（如未捕获的崩溃、非零退出码被判定为崩溃，M1b 定规则）。
    Crashed,
}

impl ExitReason {
    /// 序列化后的字符串形式（与 JSON 中的取值一致）。
    pub fn as_str(&self) -> &'static str {
        match self {
            ExitReason::Normal => "normal",
            ExitReason::Killed => "killed",
            ExitReason::Crashed => "crashed",
        }
    }
}

/// 运行时状态（status 事件的 `state` 字段）。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum RunState {
    /// 已启动并在运行。
    Running,
    /// 已退出（含正常退出与被杀）。
    Exited,
    /// 已崩溃。
    Crashed,
}

impl RunState {
    /// 序列化后的字符串形式（与 JSON 中的取值一致）。
    pub fn as_str(&self) -> &'static str {
        match self {
            RunState::Running => "running",
            RunState::Exited => "exited",
            RunState::Crashed => "crashed",
        }
    }
}

/// 会话事件：JSONL 中的一行 = 一个事件。
///
/// 序列化形态（`event` 为标签字段，写在最前）：
/// ```json
/// {"event":"start","v":1,"ts":"...","session_id":"...","app_id":"...","cmd":"...","cwd":"...","argv":[],"pid":1,"ppid":0,"start_time":0}
/// ```
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "event", rename_all = "snake_case")]
pub enum SessionEvent {
    /// 会话开始：记录启动命令、工作目录、参数与进程信息。
    Start {
        /// 事件格式版本，当前恒为 [`SESSION_EVENT_VERSION`]。
        v: u32,
        /// RFC3339 时间戳。
        ts: String,
        /// 会话 id（一次启动一个）。
        session_id: SessionId,
        /// 关联的应用配置 id。
        app_id: String,
        /// 实际执行的命令行（可读形式，便于人看）。
        cmd: String,
        /// 进程工作目录。
        cwd: String,
        /// 完整参数向量（含 argv[0]），便于精确还原启动方式。
        argv: Vec<String>,
        /// 子进程 pid。
        pid: u32,
        /// 启动器自身 pid，便于回溯是谁拉起的。
        ppid: u32,
        /// 子进程启动时间的平台原始值（Windows 为 FILETIME，100ns 粒度）。
        ///
        /// 与 `pid` 一起构成「进程身份」：重启启动器后据此判断该 pid 是否已被复用，
        /// 是 F4 重关联（attach）的唯一依据。缺省为 0 表示旧事件未记录（视为无法重关联）。
        #[serde(default)]
        start_time: u64,
    },
    /// 一行输出（stdout 或 stderr）。
    Output {
        /// 事件格式版本。
        v: u32,
        /// RFC3339 时间戳。
        ts: String,
        /// 输出流类型。
        stream: StreamKind,
        /// 单行内容（不含行尾换行；内容中的控制字符由 JSON 转义，保证仍是单行）。
        line: String,
        /// 该行原始字节的编码标记（`utf-8` / `gbk` / `lossy`）。
        ///
        /// 缺省为 `utf-8`：旧日志没有这个字段，而旧捕获层已对非 UTF-8 字节做过有损替换，
        /// 按 UTF-8 解读是唯一诚实的默认值。详见 `docs/SESSION_FORMAT.md`「编码处理策略」。
        #[serde(default)]
        encoding: OutputEncoding,
    },
    /// 运行状态变化（窗口标题与监听端口为尽力而为项）。
    Status {
        /// 事件格式版本。
        v: u32,
        /// RFC3339 时间戳。
        ts: String,
        /// 当前状态。
        state: RunState,
        /// 窗口标题；取不到时为 `null`。
        #[serde(default)]
        window_title: Option<String>,
        /// 监听端口；取不到时为 `null`。
        #[serde(default)]
        ports: Option<Vec<u16>>,
    },
    /// 进程退出。
    Exit {
        /// 事件格式版本。
        v: u32,
        /// RFC3339 时间戳。
        ts: String,
        /// 退出码；被信号杀死或取不到时为 `null`。
        #[serde(default)]
        code: Option<i32>,
        /// 退出原因。
        reason: ExitReason,
    },
    /// 显式溢出事件：写盘跟不上时记录丢弃的行数，绝不静默丢行。
    Overflow {
        /// 事件格式版本。
        v: u32,
        /// RFC3339 时间戳。
        ts: String,
        /// 本次丢弃的行数。
        dropped: u64,
    },
}

impl SessionEvent {
    /// 构造 start 事件（自动填版本与时间戳）。
    ///
    /// `start_time` 是子进程启动时间的平台原始值，与 `pid` 一起构成进程身份；
    /// 无法取得时传 0（该会话将无法被重关联）。
    #[allow(clippy::too_many_arguments)]
    pub fn start(
        session_id: SessionId,
        app_id: impl Into<String>,
        cmd: impl Into<String>,
        cwd: impl Into<String>,
        argv: Vec<String>,
        pid: u32,
        ppid: u32,
        start_time: u64,
    ) -> Self {
        Self::Start {
            v: SESSION_EVENT_VERSION,
            ts: now_rfc3339(),
            session_id,
            app_id: app_id.into(),
            cmd: cmd.into(),
            cwd: cwd.into(),
            argv,
            pid,
            ppid,
            start_time,
        }
    }

    /// 构造 output 事件（编码标记取默认值 `utf-8`）。
    pub fn output(stream: StreamKind, line: impl Into<String>) -> Self {
        Self::output_with_encoding(stream, line, OutputEncoding::Utf8)
    }

    /// 构造带编码标记的 output 事件。
    ///
    /// 捕获层用这个构造函数如实标注每行原始字节的编码，使 agent 能区分
    /// 「本来就是 UTF-8」「按 GBK 无损解出」与「已不可还原（lossy）」。
    pub fn output_with_encoding(
        stream: StreamKind,
        line: impl Into<String>,
        encoding: OutputEncoding,
    ) -> Self {
        Self::Output {
            v: SESSION_EVENT_VERSION,
            ts: now_rfc3339(),
            stream,
            line: line.into(),
            encoding,
        }
    }

    /// 构造 status 事件。
    pub fn status(state: RunState, window_title: Option<String>, ports: Option<Vec<u16>>) -> Self {
        Self::Status {
            v: SESSION_EVENT_VERSION,
            ts: now_rfc3339(),
            state,
            window_title,
            ports,
        }
    }

    /// 构造 exit 事件。
    pub fn exit(code: Option<i32>, reason: ExitReason) -> Self {
        Self::Exit {
            v: SESSION_EVENT_VERSION,
            ts: now_rfc3339(),
            code,
            reason,
        }
    }

    /// 构造 overflow 事件。
    pub fn overflow(dropped: u64) -> Self {
        Self::Overflow {
            v: SESSION_EVENT_VERSION,
            ts: now_rfc3339(),
            dropped,
        }
    }

    /// 事件的版本号（所有变体都有）。
    pub fn version(&self) -> u32 {
        match self {
            SessionEvent::Start { v, .. }
            | SessionEvent::Output { v, .. }
            | SessionEvent::Status { v, .. }
            | SessionEvent::Exit { v, .. }
            | SessionEvent::Overflow { v, .. } => *v,
        }
    }

    /// 事件时间戳（RFC3339）。
    pub fn timestamp(&self) -> &str {
        match self {
            SessionEvent::Start { ts, .. }
            | SessionEvent::Output { ts, .. }
            | SessionEvent::Status { ts, .. }
            | SessionEvent::Exit { ts, .. }
            | SessionEvent::Overflow { ts, .. } => ts,
        }
    }

    /// 事件名（与 JSON 中 `event` 字段一致）。
    pub fn event_name(&self) -> &'static str {
        match self {
            SessionEvent::Start { .. } => "start",
            SessionEvent::Output { .. } => "output",
            SessionEvent::Status { .. } => "status",
            SessionEvent::Exit { .. } => "exit",
            SessionEvent::Overflow { .. } => "overflow",
        }
    }

    /// 序列化为 JSONL 的一行（**不含**行尾换行，且保证中间没有换行）。
    ///
    /// 若内容中出现换行符（理论上 serde_json 会转义，仍显式校验一次），返回
    /// [`AppCatchError::Serialize`]，避免写出破坏 JSONL 行语义的数据。
    pub fn to_jsonl_line(&self) -> Result<String> {
        let text = serde_json::to_string(self).map_err(|e| AppCatchError::Serialize(e.to_string()))?;
        if text.contains('\n') || text.contains('\r') {
            return Err(AppCatchError::Serialize(
                "序列化结果包含换行符，会破坏 JSONL 行结构".to_string(),
            ));
        }
        Ok(text)
    }

    /// 从 JSONL 的一行解析事件（M1b 读取回放用）。
    pub fn from_jsonl_line(line: &str) -> Result<Self> {
        serde_json::from_str::<Self>(line)
            .map_err(|e| AppCatchError::parse(std::path::Path::new("<jsonl-line>"), e))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn sample_start() -> SessionEvent {
        SessionEvent::start(
            SessionId::new_v4(),
            "demo-app",
            "python main.py",
            "E:/repo/demo",
            vec![
                "python".to_string(),
                "main.py".to_string(),
                "--port".to_string(),
                "8000".to_string(),
            ],
            4242,
            1234,
            133_000_000_000_000_000,
        )
    }

    /// 五类事件的统一样例，供单行/版本/往返测试复用。
    fn all_events() -> Vec<SessionEvent> {
        vec![
            sample_start(),
            SessionEvent::output(StreamKind::Stdout, "服务已启动，监听 8000"),
            SessionEvent::output(StreamKind::Stderr, "warn: 端口被占用"),
            SessionEvent::status(
                RunState::Running,
                Some("Demo - 主窗口".to_string()),
                Some(vec![8000, 8001]),
            ),
            SessionEvent::exit(Some(1), ExitReason::Crashed),
            SessionEvent::overflow(128),
        ]
    }

    #[test]
    fn session_id_round_trips_through_string_and_json() {
        let id = SessionId::new_v4();
        let text = id.to_string();
        let parsed: SessionId = text.parse().expect("解析失败");
        assert_eq!(parsed, id);

        let json = serde_json::to_string(&id).expect("序列化失败");
        assert_eq!(json, format!("\"{text}\""));
        let back: SessionId = serde_json::from_str(&json).expect("反序列化失败");
        assert_eq!(back, id);

        assert!(matches!(
            "not-a-uuid".parse::<SessionId>().expect_err("应失败"),
            AppCatchError::InvalidId { .. }
        ));
    }

    #[test]
    fn every_event_serializes_to_single_line_with_version() {
        for event in all_events() {
            let line = event.to_jsonl_line().expect("序列化失败");
            assert!(!line.contains('\n'), "不得包含换行: {line}");
            assert!(!line.contains('\r'), "不得包含回车: {line}");
            assert!(line.contains("\"v\":1"), "应包含版本字段 v=1: {line}");
            assert!(
                line.contains(&format!("\"event\":\"{}\"", event.event_name())),
                "应包含事件名: {line}"
            );
            assert_eq!(event.version(), SESSION_EVENT_VERSION);
            assert!(!event.timestamp().is_empty(), "时间戳不应为空");
        }
    }

    #[test]
    fn every_event_round_trips_through_json() {
        for event in all_events() {
            let line = event.to_jsonl_line().expect("序列化失败");
            let back = SessionEvent::from_jsonl_line(&line).expect("反序列化失败");
            assert_eq!(back, event, "往返后应等值: {line}");
        }
    }

    #[test]
    fn start_event_contains_required_fields() {
        let line = sample_start().to_jsonl_line().expect("序列化失败");
        for key in [
            "\"v\":1",
            "\"ts\":",
            "\"session_id\":",
            "\"app_id\":",
            "\"cmd\":",
            "\"cwd\":",
            "\"argv\":",
            "\"pid\":",
            "\"ppid\":",
            "\"start_time\":",
        ] {
            assert!(line.contains(key), "start 事件缺少字段 {key}: {line}");
        }
    }

    #[test]
    fn enum_variants_use_lowercase_wire_format() {
        assert_eq!(
            serde_json::to_string(&StreamKind::Stdout).expect("序列化失败"),
            "\"stdout\""
        );
        assert_eq!(
            serde_json::to_string(&StreamKind::Stderr).expect("序列化失败"),
            "\"stderr\""
        );
        assert_eq!(
            serde_json::to_string(&ExitReason::Normal).expect("序列化失败"),
            "\"normal\""
        );
        assert_eq!(
            serde_json::to_string(&ExitReason::Killed).expect("序列化失败"),
            "\"killed\""
        );
        assert_eq!(
            serde_json::to_string(&ExitReason::Crashed).expect("序列化失败"),
            "\"crashed\""
        );
        assert_eq!(
            serde_json::to_string(&RunState::Running).expect("序列化失败"),
            "\"running\""
        );
        assert_eq!(RunState::Exited.as_str(), "exited");
        assert_eq!(ExitReason::Killed.as_str(), "killed");
        assert_eq!(StreamKind::Stderr.as_str(), "stderr");
    }

    #[test]
    fn timestamp_is_rfc3339_with_z_suffix() {
        let ts = now_rfc3339();
        assert!(ts.ends_with('Z'), "UTC 时间戳应以 Z 结尾: {ts}");
        assert!(ts.contains('T'), "RFC3339 应含 T 分隔符: {ts}");
        // 可被 time crate 反过来解析，证明格式合法
        assert!(
            OffsetDateTime::parse(&ts, &Rfc3339).is_ok(),
            "时间戳应可被解析: {ts}"
        );
    }

    #[test]
    fn output_line_with_control_chars_stays_single_line() {
        let event = SessionEvent::output(StreamKind::Stdout, "第一段\n第二段\ttab");
        let line = event.to_jsonl_line().expect("序列化失败");
        assert!(!line.contains('\n'), "换行应被 JSON 转义: {line}");
        let back = SessionEvent::from_jsonl_line(&line).expect("反序列化失败");
        assert_eq!(back, event);
        assert_eq!(event.event_name(), "output");
    }

    #[test]
    fn optional_status_and_exit_fields_accept_null() {
        let status = SessionEvent::status(RunState::Running, None, None);
        let line = status.to_jsonl_line().expect("序列化失败");
        assert!(line.contains("\"window_title\":null"));
        assert!(line.contains("\"ports\":null"));
        assert_eq!(SessionEvent::from_jsonl_line(&line).expect("反序列化失败"), status);

        // 缺失可选字段时也应能解析（向后兼容旧事件）
        let minimal = r#"{"event":"exit","v":1,"ts":"2026-01-01T00:00:00Z","reason":"killed"}"#;
        let parsed = SessionEvent::from_jsonl_line(minimal).expect("最小 exit 应可解析");
        match parsed {
            SessionEvent::Exit { code, reason, .. } => {
                assert_eq!(code, None);
                assert_eq!(reason, ExitReason::Killed);
            }
            other => panic!("应为 exit 事件，实际: {other:?}"),
        }
    }

    /// 启动时间必须能从 JSONL 原样复原：这是 F4 重关联判定 PID 复用的唯一依据。
    #[test]
    fn start_time_survives_json_round_trip() {
        let event = sample_start();
        let line = event.to_jsonl_line().expect("序列化失败");
        assert!(
            line.contains("\"start_time\":133000000000000000"),
            "start 事件应包含原始 start_time: {line}"
        );

        match SessionEvent::from_jsonl_line(&line).expect("反序列化失败") {
            SessionEvent::Start { start_time, pid, .. } => {
                assert_eq!(start_time, 133_000_000_000_000_000);
                assert_eq!(pid, 4242);
            }
            other => panic!("应为 start 事件，实际: {other:?}"),
        }
    }

    /// 旧事件（M1a 产物）没有 start_time 字段，读取时应回落为 0 而不是解析失败。
    #[test]
    fn start_event_without_start_time_defaults_to_zero() {
        let legacy = r#"{"event":"start","v":1,"ts":"2026-01-01T00:00:00Z","session_id":"e6b562fe-91e5-45f3-8a34-8c42e0be9cef","app_id":"demo","cmd":"cmd","cwd":"C:/","argv":["cmd"],"pid":10,"ppid":20}"#;
        match SessionEvent::from_jsonl_line(legacy).expect("旧事件应可解析") {
            SessionEvent::Start { start_time, .. } => assert_eq!(start_time, 0),
            other => panic!("应为 start 事件，实际: {other:?}"),
        }
    }

    /// 编码标记的线上表示：agent 直接读这个字符串判断该行原始编码。
    #[test]
    fn output_encoding_wire_values_are_stable() {
        assert_eq!(OutputEncoding::Utf8.as_str(), "utf-8");
        assert_eq!(OutputEncoding::Gbk.as_str(), "gbk");
        assert_eq!(OutputEncoding::Lossy.as_str(), "lossy");
        assert_eq!(DEFAULT_OUTPUT_ENCODING, "utf-8");
        assert_eq!(OutputEncoding::default(), OutputEncoding::Utf8);
        assert_eq!(OutputEncoding::Gbk.to_string(), "gbk");

        for (variant, wire) in [
            (OutputEncoding::Utf8, "\"utf-8\""),
            (OutputEncoding::Gbk, "\"gbk\""),
            (OutputEncoding::Lossy, "\"lossy\""),
        ] {
            let json = serde_json::to_string(&variant).expect("序列化失败");
            assert_eq!(json, wire, "编码标记的 JSON 表示不得漂移");
            let back: OutputEncoding = serde_json::from_str(&json).expect("反序列化失败");
            assert_eq!(back, variant);
        }
    }

    /// output 事件必须显式带 encoding 字段，且取值为约定字符串。
    #[test]
    fn output_event_carries_encoding_field() {
        let event = SessionEvent::output_with_encoding(
            StreamKind::Stdout,
            "中文测试",
            OutputEncoding::Gbk,
        );
        let line = event.to_jsonl_line().expect("序列化失败");
        assert!(line.contains("\"encoding\":\"gbk\""), "应包含 encoding 字段: {line}");
        assert!(line.contains("\"line\":\"中文测试\""), "行内容不得损坏: {line}");
        assert_eq!(SessionEvent::from_jsonl_line(&line).expect("反序列化失败"), event);
    }

    /// 向后兼容：旧日志的 output 事件没有 encoding 字段，必须能解析且默认 utf-8。
    #[test]
    fn legacy_output_event_without_encoding_defaults_to_utf8() {
        let legacy = concat!(
            "{\"event\":\"output\",\"v\":1,\"ts\":\"2026-01-01T00:00:00Z\",",
            "\"stream\":\"stdout\",\"line\":\"服务已启动\"}"
        );
        match SessionEvent::from_jsonl_line(legacy).expect("旧 output 事件应可解析") {
            SessionEvent::Output {
                line, encoding, ..
            } => {
                assert_eq!(line, "服务已启动");
                assert_eq!(
                    encoding,
                    OutputEncoding::Utf8,
                    "缺失 encoding 时应按 utf-8 解读"
                );
                assert_eq!(encoding.as_str(), DEFAULT_OUTPUT_ENCODING);
            }
            other => panic!("应为 output 事件，实际: {other:?}"),
        }
    }

    #[test]
    fn invalid_json_line_reports_parse_error() {
        let err = SessionEvent::from_jsonl_line("{not json}").expect_err("应失败");
        assert!(
            matches!(err, AppCatchError::Parse { .. }),
            "应为 Parse 错误，实际: {err:?}"
        );
    }
}
