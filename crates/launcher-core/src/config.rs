//! 配置模型与 TOML 存储（需求基线 F1）。
//!
//! 存储布局：每个应用一个 TOML 文件 `<home>/apps/<app_id>.toml`，字段全部 snake_case。
//! 写盘走 [`crate::fsutil::write_atomic`]（同目录临时文件 + rename），保证不会留下半截 TOML。
//!
//! 设计取舍：
//! - [`AppId`] 是受约束的新类型：只允许 `[A-Za-z0-9_-]`、非空、长度 <= [`APP_ID_MAX_LEN`]，
//!   因此可以安全地当作文件名（不含路径分隔符，无法逃逸出 `apps/` 目录）；
//! - 目录即真相：没有独立索引文件，[`AppStore::list`] 直接扫描 `apps/` 下的 `*.toml`，
//!   避免索引与文件不一致的双写问题；
//! - TOML 中可省略的字段（args/cwd/env/icon/tags）由 serde 默认值补齐，读旧配置不会失败。

use std::collections::BTreeMap;
use std::fmt;
use std::path::{Path, PathBuf};
use std::str::FromStr;

use serde::{Deserialize, Serialize};

use crate::error::{AppCatchError, Result};
use crate::fsutil::write_atomic;
use crate::paths;

/// `AppId` 允许的最大长度。
pub const APP_ID_MAX_LEN: usize = 64;

/// 应用标识符：配置文件主键，同时是文件名的一部分。
///
/// 约束（构造与反序列化时都会校验）：
/// - 非空，长度 1..=[`APP_ID_MAX_LEN`]；
/// - 只允许 ASCII 字母、数字、下划线、连字符。
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct AppId(String);

impl AppId {
    /// 校验并构造一个 `AppId`。
    pub fn new(raw: impl Into<String>) -> Result<Self> {
        let raw = raw.into();
        Self::validate(&raw)?;
        Ok(Self(raw))
    }

    /// 校验规则实现；错误信息直接给出可读原因。
    fn validate(raw: &str) -> Result<()> {
        if raw.is_empty() {
            return Err(AppCatchError::InvalidId {
                value: raw.to_string(),
                reason: "不能为空".to_string(),
            });
        }
        if raw.chars().count() > APP_ID_MAX_LEN {
            return Err(AppCatchError::InvalidId {
                value: raw.to_string(),
                reason: format!("长度不能超过 {APP_ID_MAX_LEN} 个字符"),
            });
        }
        if let Some(bad) = raw
            .chars()
            .find(|c| !(c.is_ascii_alphanumeric() || *c == '_' || *c == '-'))
        {
            return Err(AppCatchError::InvalidId {
                value: raw.to_string(),
                reason: format!("包含非法字符 {bad:?}，只允许 A-Z a-z 0-9 _ -"),
            });
        }
        Ok(())
    }

    /// 以字符串切片形式借用内部值。
    pub fn as_str(&self) -> &str {
        &self.0
    }
}

impl fmt::Display for AppId {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.0)
    }
}

impl FromStr for AppId {
    type Err = AppCatchError;

    fn from_str(s: &str) -> Result<Self> {
        Self::new(s)
    }
}

impl TryFrom<&str> for AppId {
    type Error = AppCatchError;

    fn try_from(value: &str) -> Result<Self> {
        Self::new(value)
    }
}

impl Serialize for AppId {
    fn serialize<S: serde::Serializer>(&self, serializer: S) -> std::result::Result<S::Ok, S::Error> {
        serializer.serialize_str(&self.0)
    }
}

impl<'de> Deserialize<'de> for AppId {
    fn deserialize<D: serde::Deserializer<'de>>(
        deserializer: D,
    ) -> std::result::Result<Self, D::Error> {
        let raw = String::deserialize(deserializer)?;
        Self::new(raw).map_err(serde::de::Error::custom)
    }
}

/// 单个应用的配置（需求基线 F1）。
///
/// TOML 字段均为 snake_case，含义与默认值见 `docs/CONFIG.md`。
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct AppConfig {
    /// 主键，同时决定配置文件名 `<id>.toml`。
    pub id: AppId,
    /// 展示名称。
    pub name: String,
    /// 启动命令（不含参数）。
    pub command: String,
    /// 命令行参数，保持顺序。
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub args: Vec<String>,
    /// 工作目录；缺省为空路径，表示由启动器决定（M1b：继承启动器当前目录）。
    #[serde(default, skip_serializing_if = "is_empty_path")]
    pub cwd: PathBuf,
    /// 环境变量增量（BTreeMap 保证 TOML 输出顺序稳定，便于 diff）。
    #[serde(default, skip_serializing_if = "BTreeMap::is_empty")]
    pub env: BTreeMap<String, String>,
    /// 可选的自定义图标路径；缺省 None 表示走 M3 的运行时抓取/兜底。
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub icon: Option<PathBuf>,
    /// 标签，便于 GUI 分组过滤。
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub tags: Vec<String>,
}

/// `skip_serializing_if` 辅助：空路径不写入 TOML，避免产出 `cwd = ""` 这类噪音。
fn is_empty_path(path: &PathBuf) -> bool {
    path.as_os_str().is_empty()
}

impl AppConfig {
    /// 以最小信息构造配置，其余字段取默认值。
    pub fn new(id: AppId, name: impl Into<String>, command: impl Into<String>) -> Self {
        Self {
            id,
            name: name.into(),
            command: command.into(),
            args: Vec::new(),
            cwd: PathBuf::new(),
            env: BTreeMap::new(),
            icon: None,
            tags: Vec::new(),
        }
    }
}

/// 应用配置仓库：以 `apps/` 目录为唯一真相。
///
/// 所有方法都不修改全局状态，便于测试中指向 `tempfile` 临时目录。
#[derive(Debug, Clone)]
pub struct AppStore {
    apps_dir: PathBuf,
}

impl AppStore {
    /// 用显式目录构造（测试或自定义布局用）。
    pub fn new(apps_dir: impl Into<PathBuf>) -> Self {
        Self {
            apps_dir: apps_dir.into(),
        }
    }

    /// 按 `APP_CATCH_HOME` / 用户主目录推导出默认布局的仓库。
    pub fn from_home() -> Result<Self> {
        Ok(Self::new(paths::apps_dir()?))
    }

    /// 仓库根目录（`<home>/apps`）。
    pub fn apps_dir(&self) -> &Path {
        &self.apps_dir
    }

    /// 幂等创建 `apps/` 目录。
    pub fn ensure_dir(&self) -> Result<()> {
        std::fs::create_dir_all(&self.apps_dir)
            .map_err(|e| AppCatchError::io(&self.apps_dir, e))
    }

    /// 某个应用的配置文件路径 `<apps>/<id>.toml`。
    pub fn config_path(&self, id: &AppId) -> PathBuf {
        self.apps_dir.join(format!("{id}.{}", paths::APP_CONFIG_EXT))
    }

    /// 判断配置是否存在。
    pub fn exists(&self, id: &AppId) -> bool {
        self.config_path(id).is_file()
    }

    /// 列出全部应用，按 id 升序稳定排序。
    ///
    /// `apps/` 不存在时返回空列表（首次启动语义）。
    pub fn list(&self) -> Result<Vec<AppConfig>> {
        let entries = match std::fs::read_dir(&self.apps_dir) {
            Ok(entries) => entries,
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(Vec::new()),
            Err(e) => return Err(AppCatchError::io(&self.apps_dir, e)),
        };

        let mut configs = Vec::new();
        for entry in entries {
            let entry = entry.map_err(|e| AppCatchError::io(&self.apps_dir, e))?;
            let path = entry.path();
            if !path.is_file() {
                continue;
            }
            let is_toml = path
                .extension()
                .map(|ext| ext.eq_ignore_ascii_case(paths::APP_CONFIG_EXT))
                .unwrap_or(false);
            if !is_toml {
                continue;
            }
            configs.push(Self::read_config(&path)?);
        }

        configs.sort_by(|a, b| a.id.cmp(&b.id));
        Ok(configs)
    }

    /// 读取单个应用配置；不存在返回 [`AppCatchError::NotFound`]。
    pub fn get(&self, id: &AppId) -> Result<AppConfig> {
        let path = self.config_path(id);
        if !path.is_file() {
            return Err(AppCatchError::not_found(format!("应用配置 {id}")));
        }
        Self::read_config(&path)
    }

    /// 新建应用配置；id 已存在返回 [`AppCatchError::AlreadyExists`]。
    pub fn create(&self, config: AppConfig) -> Result<AppConfig> {
        let path = self.config_path(&config.id);
        if path.exists() {
            return Err(AppCatchError::already_exists(format!(
                "应用配置 {}",
                config.id
            )));
        }
        Self::write_config(&path, &config)?;
        Ok(config)
    }

    /// 覆盖已有应用配置；不存在返回 [`AppCatchError::NotFound`]。
    pub fn update(&self, config: AppConfig) -> Result<AppConfig> {
        let path = self.config_path(&config.id);
        if !path.is_file() {
            return Err(AppCatchError::not_found(format!("应用配置 {}", config.id)));
        }
        Self::write_config(&path, &config)?;
        Ok(config)
    }

    /// 删除应用配置；不存在返回 [`AppCatchError::NotFound`]。
    pub fn delete(&self, id: &AppId) -> Result<()> {
        let path = self.config_path(id);
        if !path.is_file() {
            return Err(AppCatchError::not_found(format!("应用配置 {id}")));
        }
        std::fs::remove_file(&path).map_err(|e| AppCatchError::io(&path, e))
    }

    /// 把配置序列化为 TOML 文本（无写盘副作用，便于文档示例与测试比对）。
    pub fn to_toml(config: &AppConfig) -> Result<String> {
        toml::to_string_pretty(config).map_err(|e| AppCatchError::Serialize(e.to_string()))
    }

    /// 从 TOML 文本解析配置。
    pub fn from_toml(path: &Path, text: &str) -> Result<AppConfig> {
        toml::from_str::<AppConfig>(text).map_err(|e| AppCatchError::parse(path, e))
    }

    fn read_config(path: &Path) -> Result<AppConfig> {
        let text = std::fs::read_to_string(path).map_err(|e| AppCatchError::io(path, e))?;
        Self::from_toml(path, &text)
    }

    fn write_config(path: &Path, config: &AppConfig) -> Result<()> {
        let text = Self::to_toml(config)?;
        write_atomic(path, &text)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::paths::tests::HomeGuard;

    /// 每个测试独立的临时 home + AppStore；guard 保证退出时恢复 APP_CATCH_HOME。
    fn temp_store() -> (tempfile::TempDir, HomeGuard, AppStore) {
        let dir = tempfile::tempdir().expect("创建临时目录失败");
        let guard = HomeGuard::set(dir.path());
        let store = AppStore::from_home().expect("从 home 推导 AppStore 失败");
        (dir, guard, store)
    }

    /// 覆盖全部可选字段的样例配置。
    fn sample(id: &str) -> AppConfig {
        AppConfig {
            id: AppId::new(id).expect("测试用 id 应合法"),
            name: "GPT-SoVITS".to_string(),
            command: "python".to_string(),
            args: vec!["main.py".to_string(), "--port".to_string(), "9880".to_string()],
            cwd: PathBuf::from("E:/repo/GPT-SoVITS"),
            env: BTreeMap::from([
                ("PYTHONUNBUFFERED".to_string(), "1".to_string()),
                ("RUST_LOG".to_string(), "info".to_string()),
            ]),
            icon: Some(PathBuf::from("E:/repo/GPT-SoVITS/icon.png")),
            tags: vec!["python".to_string(), "tts".to_string()],
        }
    }

    #[test]
    fn app_id_accepts_valid_and_rejects_invalid() {
        let max_len_id = "x".repeat(APP_ID_MAX_LEN);
        for ok in ["a", "GPT-SoVITS_2", max_len_id.as_str()] {
            assert!(AppId::new(ok).is_ok(), "应接受合法 id: {ok}");
        }

        for bad in ["", "with space", "中文", "a/b", "a\\b", "..", "dot.dot"] {
            let err = AppId::new(bad).expect_err("应拒绝非法 id");
            assert!(
                matches!(err, AppCatchError::InvalidId { .. }),
                "应返回 InvalidId，实际: {err:?}"
            );
        }

        let too_long = "y".repeat(APP_ID_MAX_LEN + 1);
        assert!(matches!(
            AppId::new(too_long).expect_err("超长应报错"),
            AppCatchError::InvalidId { .. }
        ));
    }

    #[test]
    fn app_id_displays_and_parses_round_trip() {
        let id: AppId = "GPT-SoVITS".parse().expect("解析失败");
        assert_eq!(id.to_string(), "GPT-SoVITS");
        assert_eq!(id.as_str(), "GPT-SoVITS");
        assert_eq!(AppId::try_from("GPT-SoVITS").expect("try_from 失败"), id);
    }

    #[test]
    fn create_list_get_update_delete_round_trip() {
        let (_dir, _guard, store) = temp_store();
        store.ensure_dir().expect("创建 apps 目录失败");

        assert!(store.list().expect("空列表应成功").is_empty());
        assert!(!store.exists(&AppId::new("alpha").expect("id")));

        store.create(sample("alpha")).expect("创建失败");
        store.create(sample("beta")).expect("创建失败");

        let ids: Vec<String> = store
            .list()
            .expect("列表失败")
            .into_iter()
            .map(|c| c.id.to_string())
            .collect();
        assert_eq!(
            ids,
            vec!["alpha".to_string(), "beta".to_string()],
            "应按 id 排序"
        );

        let alpha = store.get(&AppId::new("alpha").expect("id")).expect("读取失败");
        assert_eq!(alpha, sample("alpha"));
        assert!(store.exists(&alpha.id));

        let mut updated = alpha.clone();
        updated.name = "已改名".to_string();
        updated.args.push("--extra".to_string());
        store.update(updated.clone()).expect("更新失败");
        assert_eq!(store.get(&alpha.id).expect("再读失败"), updated);

        store.delete(&alpha.id).expect("删除失败");
        assert!(!store.exists(&alpha.id));
        assert_eq!(store.list().expect("列表失败").len(), 1);
    }

    #[test]
    fn create_duplicate_reports_already_exists() {
        let (_dir, _guard, store) = temp_store();
        store.create(sample("dup")).expect("首次创建失败");
        let err = store.create(sample("dup")).expect_err("重复创建应失败");
        assert!(
            matches!(err, AppCatchError::AlreadyExists(_)),
            "应为 AlreadyExists，实际: {err:?}"
        );
    }

    #[test]
    fn missing_id_reports_not_found() {
        let (_dir, _guard, store) = temp_store();
        let id = AppId::new("ghost").expect("id");

        assert!(matches!(
            store.get(&id).expect_err("应失败"),
            AppCatchError::NotFound(_)
        ));
        assert!(matches!(
            store.update(sample("ghost")).expect_err("应失败"),
            AppCatchError::NotFound(_)
        ));
        assert!(matches!(
            store.delete(&id).expect_err("应失败"),
            AppCatchError::NotFound(_)
        ));
    }

    #[test]
    fn toml_round_trip_keeps_all_fields() {
        let config = sample("roundtrip");
        let text = AppStore::to_toml(&config).expect("序列化失败");
        let parsed = AppStore::from_toml(Path::new("memory.toml"), &text).expect("反序列化失败");
        assert_eq!(parsed, config);

        // 字段名必须是 snake_case，且 id/name/env/args 以预期形式出现
        assert!(text.contains("id = \"roundtrip\""), "TOML 应包含 id 字段:\n{text}");
        assert!(text.contains("name = "), "TOML 应包含 name 字段");
        assert!(text.contains("[env]"), "env 应以表形式输出:\n{text}");
        assert!(text.contains("args = ["), "args 应以数组输出:\n{text}");
    }

    #[test]
    fn minimal_config_serializes_without_empty_noise() {
        // 可选字段为空时不落盘，保持配置文件可读、diff 干净
        let config = AppConfig::new(
            AppId::new("hello-service").expect("id"),
            "Hello 服务",
            "node",
        );
        let text = AppStore::to_toml(&config).expect("序列化失败");
        assert!(!text.contains("args ="), "空 args 不应写入:\n{text}");
        assert!(!text.contains("cwd ="), "空 cwd 不应写入:\n{text}");
        assert!(!text.contains("[env]"), "空 env 不应写入:\n{text}");
        assert!(!text.contains("icon ="), "None icon 不应写入:\n{text}");
        assert!(!text.contains("tags ="), "空 tags 不应写入:\n{text}");
        assert_eq!(
            text.lines().filter(|l| !l.trim().is_empty()).count(),
            3,
            "最小配置应只有 3 行:\n{text}"
        );

        // 仍然必须能解析回等值结构
        let parsed = AppStore::from_toml(Path::new("min.toml"), &text).expect("反序列化失败");
        assert_eq!(parsed, config);
    }

    #[test]
    fn toml_round_trip_with_optional_fields_defaulted() {
        let minimal = "id = \"min\"\nname = \"最小\"\ncommand = \"python\"\n";
        let config = AppStore::from_toml(Path::new("min.toml"), minimal).expect("最小配置应可解析");
        assert!(config.args.is_empty());
        assert_eq!(config.cwd, PathBuf::new());
        assert!(config.env.is_empty());
        assert_eq!(config.icon, None);
        assert!(config.tags.is_empty());
    }

    #[test]
    fn icon_none_and_some_both_survive_round_trip() {
        let mut config = sample("icon-none");
        config.icon = None;
        let text = AppStore::to_toml(&config).expect("序列化失败");
        let parsed = AppStore::from_toml(Path::new("x.toml"), &text).expect("反序列化失败");
        assert_eq!(parsed.icon, None);

        let mut with_icon = sample("icon-some");
        with_icon.icon = Some(PathBuf::from("/tmp/app.png"));
        let text = AppStore::to_toml(&with_icon).expect("序列化失败");
        let parsed = AppStore::from_toml(Path::new("x.toml"), &text).expect("反序列化失败");
        assert_eq!(parsed.icon, Some(PathBuf::from("/tmp/app.png")));
    }

    #[test]
    fn written_file_is_reparsable_and_leaves_no_temp_file() {
        let (dir, _guard, store) = temp_store();
        store.create(sample("atomic")).expect("创建失败");

        let path = dir.path().join("apps").join("atomic.toml");
        let text = std::fs::read_to_string(&path).expect("读取写入的文件失败");
        let parsed = AppStore::from_toml(&path, &text).expect("写入的文件应可重新解析");
        assert_eq!(parsed.id.to_string(), "atomic");

        let leftovers: Vec<String> = std::fs::read_dir(dir.path().join("apps"))
            .expect("读取 apps 目录失败")
            .filter_map(|e| e.ok())
            .map(|e| e.file_name().to_string_lossy().to_string())
            .filter(|n| n.contains(".tmp-"))
            .collect();
        assert!(leftovers.is_empty(), "不应残留临时文件: {leftovers:?}");
    }

    #[test]
    fn list_skips_non_toml_files() {
        let (dir, _guard, store) = temp_store();
        store.ensure_dir().expect("创建 apps 目录失败");
        store.create(sample("real")).expect("创建失败");

        std::fs::write(dir.path().join("apps").join("notes.md"), "无关文件").expect("写失败");
        std::fs::write(dir.path().join("apps").join("real.toml.tmp-1-2"), "半截").expect("写失败");

        let list = store.list().expect("列表失败");
        assert_eq!(list.len(), 1, "只应读取 *.toml 文件");
        assert_eq!(list[0].id.to_string(), "real");
    }

    #[test]
    fn parse_error_carries_path() {
        let (_dir, _guard, store) = temp_store();
        store.ensure_dir().expect("创建 apps 目录失败");
        let path = store.config_path(&AppId::new("broken").expect("id"));
        std::fs::write(&path, "id = 1\nname =").expect("写入坏配置失败");

        let err = store.get(&AppId::new("broken").expect("id")).expect_err("应失败");
        match err {
            AppCatchError::Parse { path: p, .. } => assert!(p.ends_with("broken.toml")),
            other => panic!("应为 Parse 错误，实际: {other:?}"),
        }
    }
}
