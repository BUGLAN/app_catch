//! 会话变化通知：让上层（GUI）由「每秒轮询」改为「有变化才去读」。
//!
//! [`crate::runtime::SessionManager::subscribe`] 返回一个广播接收端，管理器在下列时刻发通知：
//!
//! | 通知 | 时刻 |
//! | --- | --- |
//! | [`SessionNotice::Changed`] | 会话启动 / 退出 / 被重关联，或状态采集发现窗口标题、端口变化 |
//! | [`SessionNotice::LogAppended`] | 写盘任务排空一次队列（一批新事件已对其他读者可见） |
//!
//! 约定：
//! - 通知只说「谁变了」，不带数据——接收方自己调 `status` / 读日志，
//!   因此丢一条通知最多是晚一点刷新，不会读到错误数据；
//! - 接收方落后超过 [`NOTICE_CAPACITY`] 条时会收到 `RecvError::Lagged`，
//!   应当按「所有会话都可能变了」处理；
//! - 没有订阅者时发送失败是正常情况，直接忽略。

use crate::session::SessionId;

/// 广播通道容量。
///
/// 日志通知按「排空一次队列」合并，高频输出时也只是每批一条；
/// 256 条足够吸收界面短暂卡顿，再落后就走 `Lagged` 的全量刷新。
pub const NOTICE_CAPACITY: usize = 256;

/// 一条会话变化通知。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum SessionNotice {
    /// 会话快照变了：启动、退出、重关联，或窗口标题 / 监听端口变化。
    Changed(SessionId),
    /// 会话日志追加了新事件，且此刻打开文件就能读到。
    LogAppended(SessionId),
}

impl SessionNotice {
    /// 通知对应的会话 id。
    pub fn session_id(&self) -> SessionId {
        match self {
            Self::Changed(id) | Self::LogAppended(id) => *id,
        }
    }
}
