//! 非 Windows 平台的占位实现（保证 `cargo check` 在任意平台全绿）。
//!
//! 需求基线把 macOS 对齐放在 M4，Linux 不在本期范围；这里的实现只提供可编译的空壳，
//! 所有操作统一返回 [`AppCatchError::Unsupported`]，绝不 panic，避免误调用时让宿主进程崩溃。

use crate::error::{AppCatchError, Result};
use crate::platform::{
    ExitStatusInfo, IconCapture, IconTarget, ProcessBackend, ProcessHandle, ProcessStartTime,
    SpawnRequest, SpawnedProcess,
};

/// 非 Windows 平台的进程后端占位实现。
#[derive(Debug, Default, Clone, Copy)]
pub struct FallbackProcessBackend;

impl FallbackProcessBackend {
    /// 构造占位后端。
    pub fn new() -> Self {
        Self
    }
}

/// 统一的「当前平台未实现」错误。
fn not_implemented(op: &str) -> AppCatchError {
    AppCatchError::Unsupported(format!(
        "当前平台的进程后端 {op} 尚未实现（Windows 见 M1b，macOS 见 M4）"
    ))
}

impl ProcessBackend for FallbackProcessBackend {
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

/// 非 Windows 平台的图标捕获占位实现：恒返回 `None`（走内置兜底图）。
#[derive(Debug, Default, Clone, Copy)]
pub struct FallbackIconCapture;

impl FallbackIconCapture {
    /// 构造占位实现。
    pub fn new() -> Self {
        Self
    }
}

impl IconCapture for FallbackIconCapture {
    fn capture(&self, _target: &IconTarget) -> Option<std::path::PathBuf> {
        None
    }
}
