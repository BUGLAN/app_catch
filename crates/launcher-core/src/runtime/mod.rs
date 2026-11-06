//! 会话运行时：进程启动、输出捕获、JSONL 写入/轮转、停止与重关联。
//!
//! 分层：
//!
//! | 模块 | 职责 |
//! | --- | --- |
//! | [`log_writer`] | JSONL 追加写入、批量 flush、按体积轮转、保留 N 片 |
//! | [`capture`] | 管道逐行读取（不丢非法 UTF-8 行）、有界 channel、overflow 补写 |
//! | [`status`] | 状态采集：`StatusProbe` 探针接口、窗口标题 / 监听端口采集、变化才写事件 |
//! | [`hooks`] | 启动 hook：before（启动前，失败中止启动）/ after（启动后，后台执行） |
//! | [`notice`] | 会话变化通知：启动 / 退出 / 状态变化 / 日志追加，供 GUI 推送而不是轮询 |
//! | [`session_manager`] | 会话生命周期门面：start / stop / restart / status / recover |
//!
//! 平台进程能力由 [`crate::platform::ProcessBackend`] 提供，本层只依赖 trait。

pub mod capture;
pub mod hooks;
pub mod log_writer;
pub mod notice;
pub mod session_manager;
pub mod status;

pub use capture::{AppendNotify, CaptureConfig, CapturePipeline, CaptureSummary};
pub use log_writer::{shard_file_name, LogWriter, LogWriterConfig};
pub use notice::{SessionNotice, NOTICE_CAPACITY};
pub use session_manager::{
    classify_exit, classify_opened_exit, quick_exit_hint, render_command, state_for_reason,
    ExitInfo, SessionHandle, SessionManager, SessionSnapshot, APP_CATCH_NOTE_PREFIX,
    QUICK_EXIT_WINDOW, RECENT_EXITED_PER_APP,
};
pub use status::{
    default_status_probe, CompositeProbe, EmptyProbe, ProbeContext, StatusProbe, StatusSnapshot,
};
