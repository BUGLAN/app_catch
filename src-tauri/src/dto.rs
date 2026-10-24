//! 命令层的数据传输对象（DTO）。
//!
//! 为什么不让 launcher-core 的类型直接上线：
//! - [`launcher_core::runtime::SessionSnapshot`] 等结构没有实现 `Serialize`，
//!   而它们又属于核心库的稳定 API，不该为了界面反序列化需求去改核心（M2 边界）；
//! - DTO 把「界面看到什么」与「核心怎么实现」解耦：核心换内部表示时只改这一层；
//! - 线上字段名**统一 snake_case**，与 launcher-core 的 `AppConfig` / `SessionEvent`
//!   保持一致，避免前端同时处理两套命名风格。
//!
//! 枚举（[RunState`] / [`ExitReason`]）**直接复用**核心类型：它们的 wire 形式
//! 已经是稳定契约（`running` / `exited` / `crashed` 等），重新定义一份只会漂移。

use std::collections::BTreeMap;
use std::path::PathBuf;

use serde::{Deserialize, Serialize};

use launcher_core::config::{AppConfig, AppId};
use launcher_core::runtime::{
    CaptureConfig, ExitInfo, LogWriterConfig, SessionSnapshot, StatusSnapshot,
};
use launcher_core::session::{ExitReason, RunState};

use crate::error::CommandError;

/// 会话快照（界面侧）。
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct SessionSnapshotDto {
    /// 会话 id（会话目录名）。
    pub session_id: String,
    /// 关联应用 id。
    pub app_id: String,
    /// 子进程 pid。
    pub pid: u32,
    /// 运行状态：`running` / `exited` / `crashed`。
    pub state: RunState,
    /// 启动时间（RFC3339，UTC）。
    pub started_at: String,
    /// 已确定的退出信息；运行中为 `null`。
    pub last_exit: Option<ExitInfoDto>,
    /// 最近一次采集到的窗口标题与监听端口；未采集过为 `null`。
    pub latest_status: Option<StatusDto>,
}

impl From<SessionSnapshot> for SessionSnapshotDto {
    fn from(snapshot: SessionSnapshot) -> Self {
        Self {
            session_id: snapshot.session_id.to_string(),
            app_id: snapshot.app_id.to_string(),
            pid: snapshot.pid,
            state: snapshot.state,
            started_at: snapshot.started_at,
            last_exit: snapshot.last_exit.map(ExitInfoDto::from),
            latest_status: snapshot.latest_status.map(StatusDto::from),
        }
    }
}

/// 退出信息。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
pub struct ExitInfoDto {
    /// 退出码；被信号杀死或取不到时为 `null`。
    pub code: Option<i32>,
    /// 退出原因：`normal` / `killed` / `crashed`。
    pub reason: ExitReason,
}

impl From<ExitInfo> for ExitInfoDto {
    fn from(info: ExitInfo) -> Self {
        Self {
            code: info.code,
            reason: info.reason,
        }
    }
}

/// 状态采集结果（窗口标题 + 监听端口）。
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct StatusDto {
    /// 主窗口标题；无窗口应用为 `null`。
    pub window_title: Option<String>,
    /// 监听端口（去重升序）。
    pub ports: Vec<u16>,
}

impl From<StatusSnapshot> for StatusDto {
    fn from(snapshot: StatusSnapshot) -> Self {
        Self {
            window_title: snapshot.window_title,
            ports: snapshot.ports,
        }
    }
}

/// 日志写入配置。
///
/// 同时实现 `Serialize`（`get_capture_config` 的返回值）与 `Deserialize`
/// （`set_capture_config` 的入参）：读写用同一个形状，前端不必维护两份类型。
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct WriterConfigDto {
    /// 单分片字节上限。
    pub max_file_bytes: u64,
    /// 保留的历史分片数。
    pub max_shards: usize,
    /// 每多少行 flush 一次。
    pub flush_every: usize,
    /// 最长多久 flush 一次（毫秒）。
    pub flush_interval_ms: u64,
}

impl From<LogWriterConfig> for WriterConfigDto {
    fn from(config: LogWriterConfig) -> Self {
        Self {
            max_file_bytes: config.max_file_bytes,
            max_shards: config.max_shards,
            flush_every: config.flush_every,
            flush_interval_ms: duration_ms(config.flush_interval),
        }
    }
}

/// 捕获配置（`get_capture_config` 返回、`set_capture_config` 接收）。
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct CaptureConfigDto {
    /// 有界 channel 容量（行）。
    pub channel_capacity: usize,
    /// 单行最大字节数。
    pub max_line_bytes: usize,
    /// 状态采集间隔（毫秒）；`null` 表示禁用状态采集。
    pub status_interval_ms: Option<u64>,
    /// 每分钟最多写多少条 status 事件；`null` 表示不限。
    pub max_status_events: Option<u32>,
    /// 日志写入配置。
    pub writer: WriterConfigDto,
}

impl From<CaptureConfig> for CaptureConfigDto {
    fn from(config: CaptureConfig) -> Self {
        Self {
            channel_capacity: config.channel_capacity,
            max_line_bytes: config.max_line_bytes,
            status_interval_ms: config.status_interval.map(duration_ms),
            max_status_events: config.max_status_events,
            writer: WriterConfigDto::from(config.writer),
        }
    }
}

/// `Duration` -> 毫秒，饱和转换（`as_millis` 返回 u128，超过 u64 时取 u64::MAX）。
fn duration_ms(duration: std::time::Duration) -> u64 {
    u64::try_from(duration.as_millis()).unwrap_or(u64::MAX)
}

/// 启动时的一次性报告：运行根目录 + 重启前残留的会话 + 恢复失败原因。
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize)]
pub struct BootReport {
    /// `APP_CATCH_HOME` 解析出的根目录，便于用户核对数据位置。
    pub home: String,
    /// 重启前残留、已被重新关联或收尾的会话。
    pub recovered: Vec<SessionSnapshotDto>,
    /// 恢复过程出错时的中文原因；成功为 `null`。
    pub error: Option<String>,
}

/// 新建 / 编辑应用的输入。
///
/// 与 [`AppConfig`] 的差别只在「宽进严出」：
/// - `id` 用 `String` 接收，交给 [`AppId::new`] 校验，从而拿到中文错误文案
///   （直接反序列化成 `AppId` 只会得到 serde 的英文报错）；
/// - `cwd` / `icon` 用 `Option<String>` 接收，空串按「未设置」处理，
///   避免前端把空字符串落盘成 `cwd = ""`。
#[derive(Debug, Clone, Default, Deserialize)]
pub struct AppConfigInput {
    /// 应用 id，仅允许 `[A-Za-z0-9_-]`。
    pub id: String,
    /// 展示名称。
    pub name: String,
    /// 启动命令。
    pub command: String,
    /// 参数列表。
    #[serde(default)]
    pub args: Vec<String>,
    /// 工作目录；空表示继承启动器当前目录。
    #[serde(default)]
    pub cwd: Option<String>,
    /// 环境变量增量。
    #[serde(default)]
    pub env: BTreeMap<String, String>,
    /// 自定义图标路径。
    #[serde(default)]
    pub icon: Option<String>,
    /// 标签。
    #[serde(default)]
    pub tags: Vec<String>,
}

impl AppConfigInput {
    /// 校验并转换成 [`AppConfig`]。
    ///
    /// 校验顺序按「用户最可能出错的字段优先」：id -> 名称 -> 命令 -> 参数 -> 环境变量。
    /// 所有错误都是中文 `invalid_input`，前端可直接显示在对应输入框下方。
    pub fn into_config(self) -> Result<AppConfig, CommandError> {
        let id = AppId::new(self.id.trim()).map_err(CommandError::from)?;

        let name = self.name.trim();
        if name.is_empty() {
            return Err(CommandError::invalid_input("应用名称不能为空"));
        }

        let command = self.command.trim();
        if command.is_empty() {
            return Err(CommandError::invalid_input("启动命令不能为空"));
        }

        // 参数：去掉首尾空白，丢弃空行（表单多行输入常见空行）
        let mut args = Vec::with_capacity(self.args.len());
        for arg in self.args {
            let trimmed = arg.trim();
            if !trimmed.is_empty() {
                args.push(trimmed.to_string());
            }
        }

        let cwd = match self.cwd {
            Some(raw) if !raw.trim().is_empty() => PathBuf::from(raw.trim()),
            _ => PathBuf::new(),
        };

        let icon = match self.icon {
            Some(raw) if !raw.trim().is_empty() => Some(PathBuf::from(raw.trim())),
            _ => None,
        };

        // 环境变量：键必须非空且不含空白（`KEY = value` 解析残留的键会在这里被挡住）
        let mut env = BTreeMap::new();
        for (key, value) in self.env {
            let trimmed = key.trim();
            if trimmed.is_empty() {
                return Err(CommandError::invalid_input("环境变量的键不能为空"));
            }
            if trimmed.chars().any(char::is_whitespace) {
                return Err(CommandError::invalid_input(format!(
                    "环境变量名不能包含空白字符：{trimmed}"
                )));
            }
            env.insert(trimmed.to_string(), value);
        }

        // 标签：去空白、去重但保持输入顺序
        let mut tags: Vec<String> = Vec::with_capacity(self.tags.len());
        for tag in self.tags {
            let trimmed = tag.trim();
            if !trimmed.is_empty() && !tags.iter().any(|existing| existing == trimmed) {
                tags.push(trimmed.to_string());
            }
        }

        Ok(AppConfig {
            id,
            name: name.to_string(),
            command: command.to_string(),
            args,
            cwd,
            env,
            icon,
            tags,
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use launcher_core::session::SessionId;

    fn valid_input() -> AppConfigInput {
        AppConfigInput {
            id: "demo-app".to_string(),
            name: "演示应用".to_string(),
            command: "python".to_string(),
            args: vec!["main.py".to_string(), "--port".to_string(), "8000".to_string()],
            cwd: Some("E:/repo/demo".to_string()),
            env: BTreeMap::from([("PYTHONUNBUFFERED".to_string(), "1".to_string())]),
            icon: Some("E:/repo/demo/icon.png".to_string()),
            tags: vec!["python".to_string(), "tts".to_string()],
        }
    }

    #[test]
    fn valid_input_converts_to_config() {
        let config = valid_input().into_config().expect("合法输入应转换成功");
        assert_eq!(config.id.to_string(), "demo-app");
        assert_eq!(config.name, "演示应用");
        assert_eq!(config.command, "python");
        assert_eq!(config.args.len(), 3);
        assert_eq!(config.cwd, PathBuf::from("E:/repo/demo"));
        assert_eq!(config.icon, Some(PathBuf::from("E:/repo/demo/icon.png")));
        assert_eq!(config.tags, vec!["python".to_string(), "tts".to_string()]);
        assert_eq!(
            config.env.get("PYTHONUNBUFFERED").map(String::as_str),
            Some("1")
        );
    }

    #[test]
    fn invalid_id_reports_chinese_message() {
        let mut input = valid_input();
        input.id = "带空格 的 id".to_string();
        let err = input.into_config().expect_err("非法 id 应失败");
        assert_eq!(err.kind, "invalid_id");
        assert!(err.message.contains("非法标识符"), "文案: {}", err.message);
    }

    #[test]
    fn blank_name_and_command_are_rejected() {
        let mut input = valid_input();
        input.name = "   ".to_string();
        let err = input.into_config().expect_err("空名称应失败");
        assert_eq!(err.kind, "invalid_input");
        assert!(err.message.contains("名称"));

        let mut input = valid_input();
        input.command = "".to_string();
        let err = input.into_config().expect_err("空命令应失败");
        assert_eq!(err.kind, "invalid_input");
        assert!(err.message.contains("命令"));
    }

    #[test]
    fn whitespace_is_trimmed_and_blank_entries_dropped() {
        let mut input = valid_input();
        input.name = "  演示  ".to_string();
        input.command = " python ".to_string();
        input.args = vec![
            "  main.py ".to_string(),
            "".to_string(),
            "   ".to_string(),
            "--port".to_string(),
        ];
        input.tags = vec![
            " python ".to_string(),
            "python".to_string(),
            "".to_string(),
        ];
        let config = input.into_config().expect("应转换成功");
        assert_eq!(config.name, "演示");
        assert_eq!(config.command, "python");
        assert_eq!(config.args, vec!["main.py".to_string(), "--port".to_string()]);
        assert_eq!(config.tags, vec!["python".to_string()], "标签应去重");
    }

    #[test]
    fn empty_cwd_and_icon_become_absent() {
        let mut input = valid_input();
        input.cwd = Some("   ".to_string());
        input.icon = Some("".to_string());
        let config = input.into_config().expect("应转换成功");
        assert_eq!(config.cwd, PathBuf::new(), "空白 cwd 应为空路径");
        assert_eq!(config.icon, None, "空白 icon 应为 None");
    }

    #[test]
    fn env_key_must_be_non_empty_without_whitespace() {
        let mut input = valid_input();
        input.env = BTreeMap::from([("  ".to_string(), "1".to_string())]);
        let err = input.into_config().expect_err("空键应失败");
        assert_eq!(err.kind, "invalid_input");

        let mut input = valid_input();
        input.env = BTreeMap::from([("BAD KEY".to_string(), "1".to_string())]);
        let err = input.into_config().expect_err("含空白的键应失败");
        assert!(err.message.contains("BAD KEY"), "文案: {}", err.message);
    }

    #[test]
    fn session_snapshot_dto_mirrors_core_fields() {
        let snapshot = SessionSnapshot {
            session_id: SessionId::new_v4(),
            app_id: AppId::new("demo").expect("合法 id"),
            pid: 4321,
            state: RunState::Running,
            started_at: "2026-10-24T18:30:00Z".to_string(),
            last_exit: None,
            latest_status: Some(StatusSnapshot {
                window_title: Some("主窗口".to_string()),
                ports: vec![8000, 8001],
            }),
        };
        let dto = SessionSnapshotDto::from(snapshot.clone());
        assert_eq!(dto.session_id, snapshot.session_id.to_string());
        assert_eq!(dto.app_id, "demo");
        assert_eq!(dto.pid, 4321);
        assert_eq!(dto.state, RunState::Running);
        assert_eq!(dto.last_exit, None);
        let status = dto.latest_status.clone().expect("应有状态");
        assert_eq!(status.window_title.as_deref(), Some("主窗口"));
        assert_eq!(status.ports, vec![8000, 8001]);

        // wire 形式必须与核心枚举的既定契约一致
        let json = serde_json::to_string(&dto).expect("序列化失败");
        assert!(json.contains("\"state\":\"running\""), "{json}");
        assert!(json.contains("\"session_id\":\""), "{json}");
    }

    #[test]
    fn exit_info_dto_keeps_code_and_reason() {
        let dto = ExitInfoDto::from(ExitInfo {
            code: Some(3),
            reason: ExitReason::Crashed,
        });
        assert_eq!(dto.code, Some(3));
        assert_eq!(dto.reason, ExitReason::Crashed);
        let json = serde_json::to_string(&dto).expect("序列化失败");
        assert_eq!(json, "{\"code\":3,\"reason\":\"crashed\"}");
    }

    #[test]
    fn capture_config_dto_reports_durations_in_millis() {
        let config = CaptureConfig::default();
        let dto = CaptureConfigDto::from(config);
        // 与 launcher-core 的 CaptureConfig::default() 保持一致（3s 采集一次）
        assert_eq!(dto.status_interval_ms, Some(3000), "默认状态采集间隔应为 3s");
        assert_eq!(dto.writer.max_shards, 5);
        assert_eq!(dto.writer.flush_interval_ms, 100);
    }

    #[test]
    fn disabled_status_interval_serializes_as_null() {
        let mut config = CaptureConfig::default();
        config.status_interval = None;
        let dto = CaptureConfigDto::from(config);
        assert_eq!(dto.status_interval_ms, None);
        let json = serde_json::to_string(&dto).expect("序列化失败");
        assert!(json.contains("\"status_interval_ms\":null"), "{json}");
    }
}
