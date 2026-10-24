//! 会话运行时：进程启动、输出捕获、JSONL 写入/轮转、停止与重关联。
//!
//! 分层：
//!
//! | 模块 | 职责 |
//! | --- | --- |
//! | [`log_writer`] | JSONL 追加写入、批量 flush、按体积轮转、保留 N 片 |
//! | [`capture`] | 管道逐行读取（不丢非法 UTF-8 行）、有界 channel、overflow 补写 |
//! | [`status`] | 状态采集：`StatusProbe` 探针接口、窗口标题 / 监听端口采集、变化才写事件 |
//! | [`session_manager`] | 会话生命周期门面：start / stop / restart / status / recover |
//!
//! 平台进程能力由 [`crate::platform::ProcessBackend`] 提供，本层只依赖 trait。

pub mod capture;
pub mod log_writer;
pub mod session_manager;
pub mod status;

pub use capture::{CaptureConfig, CapturePipeline, CaptureSummary};
pub use log_writer::{shard_file_name, LogWriter, LogWriterConfig};
pub use session_manager::{
    classify_exit, render_command, state_for_reason, ExitInfo, SessionHandle, SessionManager,
    SessionSnapshot,
};
pub use status::{
    default_status_probe, CompositeProbe, EmptyProbe, ProbeContext, StatusProbe, StatusSnapshot,
};
