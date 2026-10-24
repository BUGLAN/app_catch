//! 文档一致性集成测试：保证 `docs/CONFIG.md` 与 `docs/SESSION_FORMAT.md` 里的示例
//! 与实际序列化结果**逐字一致**，避免代码演进后文档悄悄过期。
//!
//! 做法：用与文档相同的输入构造配置/事件 → 真实序列化 → 断言产出的文本块出现在文档中。

use std::collections::BTreeMap;
use std::path::PathBuf;

use launcher_core::config::{AppConfig, AppId, AppStore};
use launcher_core::session::{
    ExitReason, OutputEncoding, RunState, SessionEvent, SessionId, StreamKind,
    SESSION_EVENT_VERSION,
};

/// 仓库根下的文档路径。
fn doc_path(name: &str) -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("../../docs")
        .join(name)
}

fn read_doc(name: &str) -> String {
    let path = doc_path(name);
    std::fs::read_to_string(&path)
        .unwrap_or_else(|e| panic!("读取文档失败 {}: {e}", path.display()))
}

/// 文档中的完整配置示例（对应 docs/CONFIG.md「完整示例」小节）。
fn doc_full_config() -> AppConfig {
    AppConfig {
        id: AppId::new("gpt-sovits").expect("文档示例 id 应合法"),
        name: "GPT-SoVITS".to_string(),
        command: "python".to_string(),
        args: vec![
            "main.py".to_string(),
            "--port".to_string(),
            "9880".to_string(),
        ],
        cwd: PathBuf::from("E:/repo/GPT-SoVITS"),
        env: BTreeMap::from([
            ("PYTHONUNBUFFERED".to_string(), "1".to_string()),
            ("RUST_LOG".to_string(), "info".to_string()),
        ]),
        icon: Some(PathBuf::from("E:/repo/GPT-SoVITS/icon.png")),
        tags: vec!["python".to_string(), "tts".to_string()],
    }
}

/// 文档中的最小配置示例。
fn doc_minimal_config() -> AppConfig {
    AppConfig::new(
        AppId::new("hello-service").expect("文档示例 id 应合法"),
        "Hello 服务",
        "node",
    )
}

/// 文档示例中固定使用的会话 id 与时间戳（保证可复现比对）。
fn doc_session_id() -> SessionId {
    "e6b562fe-91e5-45f3-8a34-8c42e0be9cef"
        .parse()
        .expect("文档中的 session_id 应可解析")
}

/// 文档「完整示例」小节列出的 9 行事件（顺序一致）。
fn doc_events() -> Vec<SessionEvent> {
    vec![
        SessionEvent::Start {
            v: SESSION_EVENT_VERSION,
            ts: "2026-10-24T18:30:00.123456789Z".to_string(),
            session_id: doc_session_id(),
            app_id: "gpt-sovits".to_string(),
            cmd: "python main.py --port 9880".to_string(),
            cwd: "E:/repo/GPT-SoVITS".to_string(),
            argv: vec![
                "python".to_string(),
                "main.py".to_string(),
                "--port".to_string(),
                "9880".to_string(),
            ],
            pid: 4321,
            ppid: 1234,
            start_time: 134_000_000_000_000_000,
        },
        SessionEvent::Output {
            v: SESSION_EVENT_VERSION,
            ts: "2026-10-24T18:30:01.000000000Z".to_string(),
            stream: StreamKind::Stdout,
            line: "Running on local URL: http://127.0.0.1:9880".to_string(),
            encoding: OutputEncoding::Utf8,
        },
        SessionEvent::Output {
            v: SESSION_EVENT_VERSION,
            ts: "2026-10-24T18:30:02.000000000Z".to_string(),
            stream: StreamKind::Stderr,
            line: "warning: torch not found, fallback to CPU".to_string(),
            encoding: OutputEncoding::Utf8,
        },
        SessionEvent::Status {
            v: SESSION_EVENT_VERSION,
            ts: "2026-10-24T18:30:03.000000000Z".to_string(),
            state: RunState::Running,
            window_title: Some("GPT-SoVITS".to_string()),
            ports: Some(vec![9880]),
        },
        SessionEvent::Status {
            v: SESSION_EVENT_VERSION,
            ts: "2026-10-24T18:30:04.000000000Z".to_string(),
            state: RunState::Running,
            window_title: None,
            ports: None,
        },
        SessionEvent::Exit {
            v: SESSION_EVENT_VERSION,
            ts: "2026-10-24T18:31:00.000000000Z".to_string(),
            code: Some(0),
            reason: ExitReason::Normal,
        },
        SessionEvent::Exit {
            v: SESSION_EVENT_VERSION,
            ts: "2026-10-24T18:32:00.000000000Z".to_string(),
            code: None,
            reason: ExitReason::Killed,
        },
        SessionEvent::Exit {
            v: SESSION_EVENT_VERSION,
            ts: "2026-10-24T18:33:00.000000000Z".to_string(),
            code: Some(1),
            reason: ExitReason::Crashed,
        },
        SessionEvent::Overflow {
            v: SESSION_EVENT_VERSION,
            ts: "2026-10-24T18:34:00.000000000Z".to_string(),
            dropped: 128,
        },
    ]
}

#[test]
fn config_doc_examples_match_code_output() {
    let doc = read_doc("CONFIG.md");

    let full = AppStore::to_toml(&doc_full_config()).expect("序列化完整示例失败");
    assert!(
        doc.contains(full.trim_end()),
        "docs/CONFIG.md 的完整示例与实际序列化结果不一致，实际输出为:\n{full}"
    );

    let minimal = AppStore::to_toml(&doc_minimal_config()).expect("序列化最小示例失败");
    assert!(
        doc.contains(minimal.trim_end()),
        "docs/CONFIG.md 的最小示例与实际序列化结果不一致，实际输出为:\n{minimal}"
    );
}

#[test]
fn session_doc_examples_match_code_output() {
    let doc = read_doc("SESSION_FORMAT.md");

    for event in doc_events() {
        let line = event.to_jsonl_line().expect("序列化文档示例事件失败");
        assert!(
            doc.contains(&line),
            "docs/SESSION_FORMAT.md 缺少或与代码不一致的事件行:\n{line}"
        );
    }

    // 文档里声明的版本号必须与代码常量一致
    assert!(
        doc.contains(&format!("\"v\":{SESSION_EVENT_VERSION}")),
        "docs/SESSION_FORMAT.md 应明确出现 v={SESSION_EVENT_VERSION} 的示例"
    );
}


/// docs/ICONS.md 里声明的尺寸上限与缓存文件名规则必须与代码常量一致。
///
/// 图标策略是「写进文档的产品决策」，文档与代码漂移比没有文档更糟：
/// 用户会照着文档去 ~/.app_catch/icons/ 找文件，而代码可能已经改了命名规则。
#[test]
fn icons_doc_matches_code_constants() {
    let doc = read_doc("ICONS.md");

    // 尺寸上限
    assert!(
        doc.contains(&format!("≤ **{}px**", launcher_core::platform::ICON_MAX_SIZE)),
        "docs/ICONS.md 应声明尺寸上限为 {}px",
        launcher_core::platform::ICON_MAX_SIZE
    );

    // 缓存文件名规则：<app_id>.png
    assert!(
        doc.contains("<app_id>.png"),
        "docs/ICONS.md 应写明缓存文件名是 <app_id>.png"
    );
    assert!(
        doc.contains("icons/"),
        "docs/ICONS.md 应写明缓存目录是 icons/"
    );

    // 优先级链的四档必须在文档里出现
    for stage in ["用户自定义 icon", "运行中窗口图标", "exe 资源图标"] {
        assert!(
            doc.contains(stage),
            "docs/ICONS.md 的优先级链应包含「{stage}」"
        );
    }

    // 兜底语义与依赖选择理由
    assert!(
        doc.contains("抓不到不算失败"),
        "docs/ICONS.md 必须明确写出「抓不到不算失败」这个产品决策"
    );
    assert!(
        doc.contains("data URL"),
        "docs/ICONS.md 应说明为什么用 data URL 而不是 asset 协议"
    );
    assert!(
        doc.contains("png"),
        "docs/ICONS.md 应说明为什么选 png 而不是 image"
    );
}

#[test]
fn docs_are_linked_from_readme() {
    let readme = std::fs::read_to_string(
        PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../README.md"),
    )
    .expect("读取 README.md 失败");

    for link in [
        "docs/CONFIG.md",
        "docs/SESSION_FORMAT.md",
        "docs/RUNTIME.md",
        "docs/GUI.md",
        // M3：图标文档必须从 README 可达，否则用户找不到「为什么我的应用没有图标」
        "docs/ICONS.md",
    ] {
        assert!(readme.contains(link), "README.md 应链接 {link}");
    }
}
