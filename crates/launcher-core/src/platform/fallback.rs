//! 非 Windows 平台的兜底实现。
//!
//! - 图标捕获：只支持配置里的自定义图标与项目图标（`.png` / `.icns` / `.ico`）；macOS 有完整实现（`icon_macos`），
//!   这里服务于 Linux 等其余平台，抓不到时由前端内置兜底图承接；
//! - 进程后端：macOS / Linux 已由 [`super::unix`] 实现，这里的空壳只服务于既非 Windows 也非 Unix 的目标，
//!   所有操作统一返回 [`AppCatchError::Unsupported`]，绝不 panic。

use crate::error::{AppCatchError, Result};
use crate::platform::{
    BackendFuture, ExitStatusInfo, IconCapture, IconTarget, ProcessBackend, ProcessHandle,
    ProcessStartTime, SpawnRequest, SpawnedProcess,
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
    fn spawn<'a>(&'a self, _request: &'a SpawnRequest) -> BackendFuture<'a, Result<SpawnedProcess>> {
        Box::pin(async move { Err(not_implemented("spawn")) })
    }

    fn kill_tree<'a>(&'a self, _handle: &'a ProcessHandle) -> BackendFuture<'a, Result<()>> {
        Box::pin(async move { Err(not_implemented("kill_tree")) })
    }

    fn attach<'a>(
        &'a self,
        _pid: u32,
        _start_time: ProcessStartTime,
    ) -> BackendFuture<'a, Result<Option<ProcessHandle>>> {
        Box::pin(async move { Err(not_implemented("attach")) })
    }

    fn is_running<'a>(&'a self, _handle: &'a ProcessHandle) -> BackendFuture<'a, Result<bool>> {
        Box::pin(async move { Err(not_implemented("is_running")) })
    }

    fn wait<'a>(&'a self, _handle: &'a ProcessHandle) -> BackendFuture<'a, Result<ExitStatusInfo>> {
        Box::pin(async move { Err(not_implemented("wait")) })
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

impl crate::platform::icon::IconSource for FallbackIconCapture {
    /// 自定义图标 → 项目图标：没有平台 API 可以问窗口或可执行文件。
    fn produce(&self, target: &IconTarget) -> Option<crate::platform::icon::CapturedIcon> {
        use crate::platform::icon::{load_image_file, CapturedIcon, IconTier};
        if let Some(image) = target
            .custom_icon
            .as_deref()
            .and_then(load_image_file)
            .filter(|image| !image.is_fully_transparent())
        {
            return Some(CapturedIcon::new(image, IconTier::Custom));
        }
        let image = crate::platform::project_icon::project_icon(target.project_dir.as_deref()?)?;
        Some(CapturedIcon::new(image, IconTier::Project))
    }
}

impl IconCapture for FallbackIconCapture {
    fn capture(&self, target: &IconTarget) -> Option<std::path::PathBuf> {
        let icons_dir = crate::paths::icons_dir().ok()?;
        crate::platform::icon::resolve_and_cache(self, &icons_dir, target)
    }
}
