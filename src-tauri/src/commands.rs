//! Tauri 命令层：薄封装 launcher-core 的业务能力，并统一错误为中文结构化错误。
//!
//! 分层约定（M2 边界）：
//! - 本模块**不做业务**：校验交给 dto.rs，进程/会话交给 launcher-core；
//! - 每个命令只做三件事：解析入参 -> 调用核心 -> 转成 DTO；
//! - 所有命令返回 Result<T, CommandError>，前端拿到的永远是 { kind, message }。
//!
//! 命令清单一览见 docs/GUI.md「命令层 API」。

use std::sync::Arc;

use tauri::State;

use launcher_core::config::AppConfig;
use launcher_core::platform::IconCapture;
use launcher_core::runtime::{CaptureConfig, SessionManager};
use launcher_core::session::SessionId;

use crate::app_state::{parse_app_id, AppState};
use crate::dto::{
    AppConfigInput, BootReport, CaptureConfigDto, SessionSnapshotDto, WriterConfigDto,
};
use crate::error::CommandError;
use crate::events_reader::{SessionEventsPage, DEFAULT_PAGE_LINES};

/// 解析会话 id；失败时给出中文原因而不是 uuid 库的英文报错。
pub fn parse_session_id(raw: &str) -> Result<SessionId, CommandError> {
    raw.trim().parse::<SessionId>().map_err(|e| {
        CommandError::new(
            "invalid_id",
            format!("会话 id 不合法（应为 uuid）：{}", trim_reason(&e.to_string())),
        )
    })
}

/// 去掉底层错误里重复的前缀，只保留原因部分，让文案保持简短。
fn trim_reason(raw: &str) -> String {
    match raw.split_once("): ") {
        Some((_, rest)) => rest.to_string(),
        None => raw.to_string(),
    }
}

/// 取当前管理器与捕获配置。
///
/// 单独抽出来是为了让每个命令都拿到**同一时刻**的一致视图，
/// 且不会把 RwLock 的读锁持有到 await 之后。
fn current(state: &AppState) -> (Arc<SessionManager>, CaptureConfig) {
    (state.manager(), state.capture_config())
}

// ---------------------------------------------------------------------------
// 应用配置
// ---------------------------------------------------------------------------

/// 列出全部应用（按 id 升序）。
#[tauri::command]
pub fn list_apps(state: State<'_, AppState>) -> Result<Vec<AppConfig>, CommandError> {
    state.store().list().map_err(CommandError::from)
}

/// 读取单个应用配置。
#[tauri::command]
pub fn get_app(state: State<'_, AppState>, id: String) -> Result<AppConfig, CommandError> {
    state.get_app(&id)
}

/// 新建应用配置。
#[tauri::command]
pub fn create_app(
    state: State<'_, AppState>,
    config: AppConfigInput,
) -> Result<AppConfig, CommandError> {
    let config = config.into_config()?;
    let store = state.store();
    store.ensure_dir().map_err(CommandError::from)?;
    store.create(config).map_err(CommandError::from)
}

/// 覆盖已有应用配置。
#[tauri::command]
pub fn update_app(
    state: State<'_, AppState>,
    config: AppConfigInput,
) -> Result<AppConfig, CommandError> {
    let config = config.into_config()?;
    state.store().update(config).map_err(CommandError::from)
}

/// 删除应用配置。
///
/// 只删配置，**不碰**已有会话目录：历史日志属于审计材料，删配置不该连带抹掉。
#[tauri::command]
pub fn delete_app(state: State<'_, AppState>, id: String) -> Result<(), CommandError> {
    let app_id = parse_app_id(&id)?;
    state.store().delete(&app_id).map_err(CommandError::from)
}

// ---------------------------------------------------------------------------
// 会话生命周期
// ---------------------------------------------------------------------------

/// 启动一个应用，返回新会话快照。
#[tauri::command]
pub async fn start_app(
    state: State<'_, AppState>,
    id: String,
) -> Result<SessionSnapshotDto, CommandError> {
    let app = state.get_app(&id)?;
    let (manager, _) = current(&state);
    let handle = manager.start(&app).await.map_err(CommandError::from)?;
    Ok(SessionSnapshotDto::from(handle.snapshot))
}

/// 停止一个会话（回收整棵进程树）。
#[tauri::command]
pub async fn stop_session(
    state: State<'_, AppState>,
    session_id: String,
) -> Result<(), CommandError> {
    let session_id = parse_session_id(&session_id)?;
    let (manager, _) = current(&state);
    manager.stop(session_id).await.map_err(CommandError::from)
}

/// 重启：先停旧会话，再用同一份配置起一个新会话（新 session_id）。
#[tauri::command]
pub async fn restart_session(
    state: State<'_, AppState>,
    session_id: String,
    app_id: String,
) -> Result<SessionSnapshotDto, CommandError> {
    let session_id = parse_session_id(&session_id)?;
    let app = state.get_app(&app_id)?;
    let (manager, _) = current(&state);
    let handle = manager
        .restart(session_id, &app)
        .await
        .map_err(CommandError::from)?;
    Ok(SessionSnapshotDto::from(handle.snapshot))
}

/// 列出仍在运行的会话（按 session_id 升序）。
#[tauri::command]
pub async fn list_active_sessions(
    state: State<'_, AppState>,
) -> Result<Vec<SessionSnapshotDto>, CommandError> {
    let (manager, _) = current(&state);
    Ok(manager
        .list_active()
        .await
        .into_iter()
        .map(SessionSnapshotDto::from)
        .collect())
}

/// 查询单个会话状态；未知会话返回 not_found。
#[tauri::command]
pub async fn get_session_status(
    state: State<'_, AppState>,
    session_id: String,
) -> Result<SessionSnapshotDto, CommandError> {
    let session_id = parse_session_id(&session_id)?;
    let (manager, _) = current(&state);
    let snapshot = manager
        .status(session_id)
        .await
        .map_err(CommandError::from)?;
    Ok(SessionSnapshotDto::from(snapshot))
}

// ---------------------------------------------------------------------------
// 会话事件读取
// ---------------------------------------------------------------------------

/// 从会话目录按分片顺序增量读取事件。
///
/// 边跑边读安全：末行未写完时不消费它，游标停在它之前（详见 events_reader.rs）。
#[tauri::command]
pub fn read_session_events(
    state: State<'_, AppState>,
    session_id: String,
    from_line: u64,
    max_lines: u64,
) -> Result<SessionEventsPage, CommandError> {
    let session_id = parse_session_id(&session_id)?;
    let dir = launcher_core::paths::session_dir(&session_id).map_err(CommandError::from)?;
    let limit = if max_lines == 0 {
        DEFAULT_PAGE_LINES
    } else {
        max_lines
    };
    state.reader().read_page(&dir, from_line, limit)
}

// ---------------------------------------------------------------------------
// 捕获配置与启动报告
// ---------------------------------------------------------------------------

/// 读取当前捕获配置。
#[tauri::command]
pub fn get_capture_config(state: State<'_, AppState>) -> Result<CaptureConfigDto, CommandError> {
    Ok(CaptureConfigDto::from(state.capture_config()))
}

/// 写入捕获配置；有会话在跑时拒绝（重建管理器会丢掉会话表）。
#[tauri::command]
pub async fn set_capture_config(
    state: State<'_, AppState>,
    config: CaptureConfigDto,
) -> Result<CaptureConfigDto, CommandError> {
    let (manager, _) = current(&state);
    let running = manager.list_active().await.len();
    let mut capture = CaptureConfig::default();
    capture.channel_capacity = config.channel_capacity;
    capture.max_line_bytes = config.max_line_bytes;
    capture.status_interval = config
        .status_interval_ms
        .map(std::time::Duration::from_millis);
    capture.max_status_events = config.max_status_events;
    capture.writer = writer_config_from(&config.writer);
    state.replace_capture_config(capture, running)?;
    Ok(CaptureConfigDto::from(state.capture_config()))
}

/// 把 DTO 还原成核心的日志写入配置。
fn writer_config_from(dto: &WriterConfigDto) -> launcher_core::runtime::LogWriterConfig {
    let mut config = launcher_core::runtime::LogWriterConfig::default();
    // 0 视为「不覆盖」：界面没有暴露这些字段时不应把默认值抹成 0
    if dto.max_file_bytes > 0 {
        config.max_file_bytes = dto.max_file_bytes;
    }
    if dto.max_shards > 0 {
        config.max_shards = dto.max_shards;
    }
    if dto.flush_every > 0 {
        config.flush_every = dto.flush_every;
    }
    if dto.flush_interval_ms > 0 {
        config.flush_interval = std::time::Duration::from_millis(dto.flush_interval_ms);
    }
    config
}

/// 启动期恢复：扫描残留会话并重新关联，返回一次性报告。
///
/// 前端在首屏调用它，把「重启前还在跑的会话」呈现出来。
#[tauri::command]
pub async fn recover_sessions(state: State<'_, AppState>) -> Result<BootReport, CommandError> {
    let home = launcher_core::paths::app_catch_home()
        .map(|path| path.to_string_lossy().into_owned())
        .unwrap_or_default();
    let (manager, _) = current(&state);
    match manager.recover_sessions().await {
        Ok(list) => Ok(BootReport {
            home,
            recovered: list.into_iter().map(SessionSnapshotDto::from).collect(),
            error: None,
        }),
        // 恢复失败不让 GUI 起不来：报告里带原因，界面照常可用
        Err(err) => Ok(BootReport {
            home,
            recovered: Vec::new(),
            error: Some(CommandError::from(err).message),
        }),
    }
}


// ---------------------------------------------------------------------------
// 图标
// ---------------------------------------------------------------------------

/// 取某个应用的图标，返回可直接喂给 \`<img src>\` 的 **data URL**。
///
/// 返回 \`Ok(None)\` 表示「抓不到图标」——这不是错误，前端应显示内置默认图
/// （Python 服务类应用没有窗口，一定会走到这里）。
///
/// 三个刻意的设计：
/// 1. **走 data URL 而不是 \`asset:\` 协议**：图标本身只有几 KB 到几十 KB，
///    base64 之后仍远小于一次 IPC 的合理开销；而 \`asset:\` 协议需要在
///    \`tauri.conf.json\` 里配 \`assetProtocol.scope\`、在 capabilities 里加
///    \`core:asset:default\`，任何一处漏配都只在运行时暴露成「图裂」，调试成本高。
/// 2. **放在 \`spawn_blocking\` 里跑**：exe 图标会走 \`PrivateExtractIconsW\` /
///    \`SHGetFileInfoW\`，后者是 Shell API，遇到网络盘或冷缓存时可能几十毫秒起步，
///    不能占住 tokio 的 worker 线程。
/// 3. **失败绝不报错**：整条链路返回 \`Option\`，任何异常都收敛成 \`None\`。
///
/// `force = true` 时忽略磁盘缓存强制重抓一次；只在「会话刚启动」那一刻由前端传，
/// 因为窗口是这时才出现的，窗口档位（\`WM_GETICON\`）第一次可能拿到应用自设的真图标，
/// 必须允许它覆盖更早抓到的 \`pnpm.cmd\` 通用图标。默认 \`false\`，保持原有缓存优先行为。
#[tauri::command]
pub async fn get_app_icon(
    state: State<'_, AppState>,
    app_id: String,
    force: Option<bool>,
) -> Result<Option<String>, CommandError> {
    let app_id = parse_app_id(&app_id)?;
    let app = state.store().get(&app_id).map_err(CommandError::from)?;

    // 有运行中会话就带上 pid：优先抓窗口上的运行时真图标
    let pid = state.running_pid_for_app(app.id.as_str()).await;
    let executable = launcher_core::platform::icon::resolve_executable(&app.command);

    let target = launcher_core::platform::IconTarget::for_app(app.id.clone())
        .with_custom_icon(app.icon.clone())
        .with_executable(executable)
        .with_pid(pid);

    // 图标抓取是纯尽力而为：即使 spawn_blocking 本身失败也只是「没有图标」
    let force = force.unwrap_or(false);
    let captured = tokio::task::spawn_blocking(move || {
        let capture = launcher_core::platform::default_icon_capture();
        if force {
            capture.recapture(&target)
        } else {
            capture.capture(&target)
        }
    })
    .await
    .ok()
    .flatten();

    let Some(path) = captured else {
        return Ok(None);
    };
    let Ok(bytes) = std::fs::read(&path) else {
        // 缓存文件读不出来（例如刚好被清理）：当作没有图标
        return Ok(None);
    };
    Ok(Some(launcher_core::platform::icon::to_data_url(&bytes)))
}

/// 内置默认图标的 data URL（前端拿不到应用图标时用它，避免纯色占位块）。
///
/// 图标本身编译进二进制（\`include_bytes!\`），因此永远可用、不依赖磁盘。
#[tauri::command]
pub fn default_icon() -> String {
    launcher_core::platform::icon::to_data_url(
        launcher_core::platform::icon::DEFAULT_ICON_PNG,
    )
}

/// 返回 launcher-core 的版本号（M0 打通链路用，保留以兼容既有前端）。
#[tauri::command]
pub fn core_version() -> String {
    launcher_core::version().to_string()
}

/// 返回应用自身版本号。
#[tauri::command]
pub fn app_version() -> String {
    env!("CARGO_PKG_VERSION").to_string()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn valid_session_id_parses() {
        let id = SessionId::new_v4();
        let parsed = parse_session_id(&id.to_string()).expect("合法 uuid 应解析成功");
        assert_eq!(parsed, id);
        // 前后空白应被容忍（前端偶尔带空格）
        let padded = format!("  {id}  ");
        assert_eq!(parse_session_id(&padded).expect("应容忍空白"), id);
    }

    #[test]
    fn invalid_session_id_reports_chinese_message() {
        let err = parse_session_id("not-a-uuid").expect_err("非法 uuid 应失败");
        assert_eq!(err.kind, "invalid_id");
        assert!(err.message.contains("会话 id 不合法"), "文案: {}", err.message);
        assert!(
            !err.message.contains("InvalidId"),
            "不应把底层调试名透给用户: {}",
            err.message
        );
    }

    #[test]
    fn writer_config_from_ignores_zero_fields() {
        let defaults = launcher_core::runtime::LogWriterConfig::default();
        let zeroed = WriterConfigDto {
            max_file_bytes: 0,
            max_shards: 0,
            flush_every: 0,
            flush_interval_ms: 0,
        };
        let config = writer_config_from(&zeroed);
        assert_eq!(config.max_file_bytes, defaults.max_file_bytes, "0 不应覆盖默认值");
        assert_eq!(config.max_shards, defaults.max_shards);
        assert_eq!(config.flush_every, defaults.flush_every);
        assert_eq!(config.flush_interval, defaults.flush_interval);
    }

    #[test]
    fn writer_config_from_applies_explicit_values() {
        let dto = WriterConfigDto {
            max_file_bytes: 1024,
            max_shards: 2,
            flush_every: 8,
            flush_interval_ms: 50,
        };
        let config = writer_config_from(&dto);
        assert_eq!(config.max_file_bytes, 1024);
        assert_eq!(config.max_shards, 2);
        assert_eq!(config.flush_every, 8);
        assert_eq!(config.flush_interval, std::time::Duration::from_millis(50));
    }

    #[test]
    fn versions_are_exposed_for_legacy_frontend() {
        assert_eq!(core_version(), launcher_core::version());
        assert_eq!(app_version(), env!("CARGO_PKG_VERSION"));
    }

    /// 内置默认图标必须是合法 PNG 的 data URL（前端在无图标时直接用它）。
    #[test]
    fn default_icon_is_a_png_data_url() {
        let url = default_icon();
        assert!(
            url.starts_with("data:image/png;base64,"),
            "应是 PNG 的 data URL，实际前缀: {}",
            &url[..url.len().min(40)]
        );
        let encoded = url.trim_start_matches("data:image/png;base64,");
        // PNG 魔数的 base64 前缀：证明解出来确实是 PNG 而不是别的字节
        assert!(
            encoded.starts_with("iVBORw0KGgo"),
            "base64 解出来应是 PNG 魔数，实际前缀: {}",
            &encoded[..encoded.len().min(20)]
        );
        assert!(encoded.len() > 1000, "默认图标不应是空壳");
    }

    /// 图标 data URL 生成链路：真实 PNG 字节 -> data URL 可被前端解码。
    #[test]
    fn to_data_url_round_trips_png_bytes() {
        let bytes = launcher_core::platform::icon::DEFAULT_ICON_PNG;
        assert!(
            launcher_core::platform::icon::looks_like_png(bytes),
            "内置默认图标必须是 PNG"
        );
        let url = launcher_core::platform::icon::to_data_url(bytes);
        assert!(url.starts_with("data:image/png;base64,"));
        // base64 长度应是 4 的倍数
        let encoded = url.trim_start_matches("data:image/png;base64,");
        assert_eq!(encoded.len() % 4, 0, "base64 长度必须是 4 的倍数");
    }

    /// 命令层的可执行文件解析：绝对路径直接用，找不到的命令不猜。
    #[test]
    fn executable_resolution_used_by_icon_command() {
        // 绝对路径 + 存在 -> 原样返回
        let exe = launcher_core::platform::icon::resolve_executable("C:/Windows/System32/notepad.exe");
        if std::path::Path::new("C:/Windows/System32/notepad.exe").is_file() {
            assert!(exe.is_some(), "存在的绝对路径应解析成功");
        }
        // 不存在的绝对路径 -> None（不猜测）
        assert!(
            launcher_core::platform::icon::resolve_executable("C:/no/such/app.exe").is_none(),
            "不存在的路径不应被猜测"
        );
    }
}