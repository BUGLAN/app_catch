//! 会话变化推送：把 launcher-core 的 [`SessionNotice`] 转发成前端事件。
//!
//! 为什么要这一层（而不是前端每秒轮询）：
//! - 进程没有输出、状态没变时，前端什么都不用做；
//! - 有输出时界面在 ~50ms 内就能看到，而不是等下一个 1 秒轮询。
//!
//! 转发前先**合并**：收到第一条通知后再等 [`COALESCE_WINDOW`]，把这段时间里的通知攒成一批，
//! 每批最多发两条事件。高频输出的进程也只会让前端每秒收到二十来次事件。
//!
//! | 事件 | 载荷 | 含义 |
//! | --- | --- | --- |
//! | [`SESSIONS_CHANGED_EVENT`] | [`NoticePayload`] | 会话启动 / 退出 / 窗口标题或端口变化，前端重拉会话列表 |
//! | [`SESSION_LOG_EVENT`] | [`NoticePayload`] | 这些会话的日志有新内容，前端增量读取 |
//!
//! 事件只是提示，不带数据：前端照常调用命令取数据。漏掉一条事件最多是晚一点刷新，
//! 前端另有低频兜底轮询。

use std::collections::BTreeSet;
use std::time::Duration;

use serde::Serialize;
use tauri::{AppHandle, Emitter};
use tokio::sync::broadcast::{self, error::RecvError, error::TryRecvError};

use launcher_core::runtime::SessionNotice;

/// 会话列表变化事件名（与 `src/lib/api.ts` 的 `SESSIONS_CHANGED_EVENT` 一致）。
pub const SESSIONS_CHANGED_EVENT: &str = "sessions:changed";

/// 会话日志追加事件名（与 `src/lib/api.ts` 的 `SESSION_LOG_EVENT` 一致）。
pub const SESSION_LOG_EVENT: &str = "session:log-appended";

/// 合并窗口：收到第一条通知后再等这么久，把期间的通知并成一批。
pub const COALESCE_WINDOW: Duration = Duration::from_millis(50);

/// 事件载荷。
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct NoticePayload {
    /// 涉及的会话 id（升序去重）；`null` 表示通知有积压丢失，所有会话都可能变了。
    pub session_ids: Option<Vec<String>>,
}

/// 一个合并窗口内攒下的通知。
#[derive(Debug, Default)]
pub struct NoticeBatch {
    changed: BTreeSet<String>,
    appended: BTreeSet<String>,
    /// 接收端落后、丢过通知：这一批按「全部可能变了」发。
    lagged: bool,
}

impl NoticeBatch {
    /// 记一条通知。
    pub fn push(&mut self, notice: SessionNotice) {
        match notice {
            SessionNotice::Changed(id) => {
                self.changed.insert(id.to_string());
            }
            SessionNotice::LogAppended(id) => {
                self.appended.insert(id.to_string());
            }
        }
    }

    /// 标记这一批里丢过通知。
    pub fn mark_lagged(&mut self) {
        self.lagged = true;
    }

    /// 会话列表事件的载荷；这一批没有相关通知时为 `None`（不发事件）。
    pub fn changed_payload(&self) -> Option<NoticePayload> {
        Self::payload(&self.changed, self.lagged)
    }

    /// 日志事件的载荷；这一批没有相关通知时为 `None`（不发事件）。
    pub fn log_payload(&self) -> Option<NoticePayload> {
        Self::payload(&self.appended, self.lagged)
    }

    fn payload(ids: &BTreeSet<String>, lagged: bool) -> Option<NoticePayload> {
        if lagged {
            return Some(NoticePayload { session_ids: None });
        }
        if ids.is_empty() {
            return None;
        }
        Some(NoticePayload {
            session_ids: Some(ids.iter().cloned().collect()),
        })
    }
}

/// 启动转发任务：一直运行到管理器被丢弃（通道关闭）。
pub fn spawn_forwarder(app: AppHandle, mut receiver: broadcast::Receiver<SessionNotice>) {
    tauri::async_runtime::spawn(async move {
        loop {
            let mut batch = NoticeBatch::default();
            match receiver.recv().await {
                Ok(notice) => batch.push(notice),
                Err(RecvError::Lagged(_)) => batch.mark_lagged(),
                Err(RecvError::Closed) => return,
            }

            tokio::time::sleep(COALESCE_WINDOW).await;
            let mut closed = false;
            loop {
                match receiver.try_recv() {
                    Ok(notice) => batch.push(notice),
                    Err(TryRecvError::Lagged(_)) => batch.mark_lagged(),
                    Err(TryRecvError::Empty) => break,
                    Err(TryRecvError::Closed) => {
                        closed = true;
                        break;
                    }
                }
            }

            // 先发列表事件再发日志事件：前端处理日志时，快照已经是新的
            if let Some(payload) = batch.changed_payload() {
                let _ = app.emit(SESSIONS_CHANGED_EVENT, payload);
            }
            if let Some(payload) = batch.log_payload() {
                let _ = app.emit(SESSION_LOG_EVENT, payload);
            }
            if closed {
                return;
            }
        }
    });
}

#[cfg(test)]
mod tests {
    use super::*;
    use launcher_core::session::SessionId;

    #[test]
    fn empty_batch_emits_nothing() {
        let batch = NoticeBatch::default();
        assert_eq!(batch.changed_payload(), None);
        assert_eq!(batch.log_payload(), None);
    }

    #[test]
    fn batch_dedupes_and_splits_by_kind() {
        let a = SessionId::new_v4();
        let b = SessionId::new_v4();
        let mut batch = NoticeBatch::default();
        batch.push(SessionNotice::LogAppended(a));
        batch.push(SessionNotice::LogAppended(a));
        batch.push(SessionNotice::LogAppended(b));
        batch.push(SessionNotice::Changed(b));

        let mut both = vec![a.to_string(), b.to_string()];
        both.sort();
        assert_eq!(
            batch.log_payload(),
            Some(NoticePayload {
                session_ids: Some(both)
            })
        );
        assert_eq!(
            batch.changed_payload(),
            Some(NoticePayload {
                session_ids: Some(vec![b.to_string()])
            })
        );
    }

    #[test]
    fn lagged_batch_asks_for_full_refresh() {
        let mut batch = NoticeBatch::default();
        batch.mark_lagged();
        let full = Some(NoticePayload { session_ids: None });
        assert_eq!(batch.changed_payload(), full);
        assert_eq!(batch.log_payload(), full);
        let json = serde_json::to_string(&batch.log_payload()).expect("序列化失败");
        assert_eq!(json, "{\"session_ids\":null}");
    }
}
