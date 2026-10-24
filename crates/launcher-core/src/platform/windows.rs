//! Windows 平台实现骨架（M1a 只有占位）。
//!
//! M1b 在此填充：
//! - 用 Job Object 保证子进程整棵树可回收（需求基线 F4）；
//! - `CreateProcessW` 或 `std::process::Command` + Job 分配；
//! - `QueryFullProcessImageNameW` / `GetProcessTimes` 取启动时间做 attach 校验；
//! - 从 exe 资源与窗口图标抓取 PNG（需求基线 F5，M3 落地）。

use crate::error::{AppCatchError, Result};
use crate::platform::{
    ExitStatusInfo, IconCapture, IconTarget, ProcessBackend, ProcessHandle, ProcessStartTime,
    SpawnRequest, SpawnedProcess,
};

/// Windows 进程后端骨架。
#[derive(Debug, Default, Clone, Copy)]
pub struct WindowsProcessBackend;

impl WindowsProcessBackend {
    /// 构造后端实例。
    pub fn new() -> Self {
        Self
    }
}

/// 统一的「尚未实现」错误，便于 M1b 前调用方识别。
fn not_implemented(op: &str) -> AppCatchError {
    AppCatchError::Unsupported(format!(
        "Windows 进程后端 {op} 尚未实现（M1b 填充），M1a 只提供接口骨架"
    ))
}

impl ProcessBackend for WindowsProcessBackend {
    fn spawn(&self, _request: &SpawnRequest) -> Result<SpawnedProcess> {
        Err(not_implemented("spawn"))
    }

    fn kill_tree(&self, _handle: &ProcessHandle) -> Result<()> {
        Err(not_implemented("kill_tree"))
    }

    fn attach(&self, _pid: u32, _start_time: ProcessStartTime) -> Result<Option<ProcessHandle>> {
        Err(not_implemented("attach"))
    }

    fn is_running(&self, _handle: &ProcessHandle) -> Result<bool> {
        Err(not_implemented("is_running"))
    }

    fn wait(&self, _handle: &ProcessHandle) -> Result<ExitStatusInfo> {
        Err(not_implemented("wait"))
    }
}

/// Windows 图标捕获骨架。
#[derive(Debug, Default, Clone, Copy)]
pub struct WindowsIconCapture;

impl WindowsIconCapture {
    /// 构造图标捕获实现。
    pub fn new() -> Self {
        Self
    }
}

impl IconCapture for WindowsIconCapture {
    fn capture(&self, _target: &IconTarget) -> Option<std::path::PathBuf> {
        // 尽力而为的接口约定：抓不到就返回 None，由调用方走内置兜底图
        None
    }
}
