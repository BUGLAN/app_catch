//! 窗口配置的平台差异测试。
//!
//! 为什么值得单独测：`WindowConfig` 是 `deny_unknown_fields` 的，
//! 平台配置文件里把 `titleBarStyle` 写成 `titlebarStyle`、或把
//! `trafficLightPosition` 写成 `traffic_light_position`，后果是**整个应用启动失败**，
//! 而且只有在对应平台上才会暴露（在 Windows 上写错 macOS 配置，本机永远发现不了）。
//!
//! 平台配置是 **JSON Merge Patch**（RFC 7396）：`app.windows` 是数组，
//! 数组整体替换而不是逐项合并。所以这里先按 RFC 7396 合并，再反序列化成真实的
//! `Config` —— 与 Tauri 启动时做的事一致，测的是"合并后的最终结果"。

use std::path::PathBuf;

use serde_json::Value;
use tauri::utils::config::{Config, WindowConfig};

/// 读取与 `Cargo.toml` 同目录（= `src-tauri/`）的配置文件原文。
fn read_config(name: &str) -> String {
    let path = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join(name);
    std::fs::read_to_string(&path).unwrap_or_else(|e| panic!("读取 {name} 失败: {e}"))
}

/// RFC 7396 JSON Merge Patch：对象递归合并，数组与标量整体替换，`null` 表示删除。
fn merge_patch(target: &mut Value, patch: &Value) {
    match (target, patch) {
        (Value::Object(target_map), Value::Object(patch_map)) => {
            for (key, patch_value) in patch_map {
                if patch_value.is_null() {
                    target_map.remove(key);
                    continue;
                }
                merge_patch(
                    target_map.entry(key.clone()).or_insert(Value::Null),
                    patch_value,
                );
            }
        }
        (target, patch) => *target = patch.clone(),
    }
}

/// 合并主配置与平台配置，反序列化成真实的 `Config`。
fn merged_windows(platform: Option<&str>) -> Vec<WindowConfig> {
    let mut base: Value = serde_json::from_str(&read_config("tauri.conf.json"))
        .expect("tauri.conf.json 必须是合法 JSON");
    if let Some(name) = platform {
        let patch: Value =
            serde_json::from_str(&read_config(name)).unwrap_or_else(|e| panic!("{name} 必须是合法 JSON: {e}"));
        merge_patch(&mut base, &patch);
    }
    let config: Config = serde_json::from_value(base)
        .unwrap_or_else(|e| panic!("合并后的配置无法解析为 Tauri Config: {e}"));
    config.app.windows
}

#[test]
fn base_config_disables_native_decorations_for_custom_titlebar() {
    let windows = merged_windows(None);
    let main = windows.first().expect("必须定义至少一个窗口");
    assert_eq!(
        main.label, "main",
        "窗口 label 必须与 src-tauri/src/window.rs 的 MAIN_WINDOW 一致"
    );
    assert!(
        !main.decorations,
        "Windows/Linux 走无边框自绘标题栏，decorations 必须为 false"
    );
    assert!(main.resizable, "无边框窗口仍必须可缩放");
    assert!(
        main.min_width.is_some() && main.min_height.is_some(),
        "必须保留最小尺寸，否则布局断点会被绕过"
    );
}

#[test]
fn macos_patch_keeps_traffic_lights_with_overlay_style() {
    let windows = merged_windows(Some("tauri.macos.conf.json"));
    let main = windows.first().expect("必须覆盖主窗口配置");

    assert!(
        main.decorations,
        "macOS 必须保留系统装饰，否则红黄绿按钮会消失"
    );
    assert_eq!(
        main.title_bar_style,
        tauri::utils::TitleBarStyle::Overlay,
        "需要 hiddenInset 语义：透明标题栏 + 内容延伸到标题栏下"
    );
    assert!(main.hidden_title, "隐藏窗口标题文字，只保留红黄绿按钮");

    let position = main
        .traffic_light_position
        .as_ref()
        .expect("必须给红黄绿按钮定坐标，否则会压在自绘品牌区上");
    assert!(
        position.x > 0.0 && position.y > 0.0,
        "红黄绿按钮坐标应为正数，实际 ({}, {})",
        position.x,
        position.y
    );
}

#[test]
fn macos_patch_preserves_window_identity_and_size() {
    let base = merged_windows(None);
    let mac = merged_windows(Some("tauri.macos.conf.json"));
    let base_main = base.first().expect("主配置必须有窗口");
    let mac_main = mac.first().expect("macOS 配置必须有窗口");

    assert_eq!(
        mac_main.label, base_main.label,
        "平台配置会整体替换 windows 数组，label 必须与主配置一致"
    );
    assert_eq!(
        (mac_main.width, mac_main.height),
        (base_main.width, base_main.height),
        "默认窗口尺寸不应被平台配置意外改动"
    );
    assert_eq!(
        (mac_main.min_width, mac_main.min_height),
        (base_main.min_width, base_main.min_height),
        "最小尺寸不应被平台配置意外改动"
    );
    assert_eq!(
        mac.len(),
        1,
        "平台配置只有一个窗口；多出来一个会在 macOS 上多开空窗口"
    );
}

#[test]
fn capabilities_grant_drag_region_permission() {
    // 拖拽区依赖 plugin:window|start_dragging，它**不在** core:window 的
    // 默认权限集里（默认只含 internal-toggle-maximize 等只读项）；
    // 漏了这条，按下标题栏不会有任何反应，而且**不会报错**。
    let path = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("capabilities/default.json");
    let raw = std::fs::read_to_string(&path).expect("读取 capabilities/default.json 失败");
    let json: Value = serde_json::from_str(&raw).expect("capabilities 必须是合法 JSON");
    let permissions = json["permissions"]
        .as_array()
        .expect("permissions 必须是数组")
        .iter()
        .filter_map(|v| v.as_str())
        .collect::<Vec<_>>();

    assert!(permissions.contains(&"core:default"), "基础能力必须保留");
    assert!(
        permissions.contains(&"core:window:allow-start-dragging"),
        "缺少拖拽权限，自定义标题栏将无法拖动窗口"
    );
    let windows = json["windows"].as_array().expect("windows 必须是数组");
    assert!(
        windows.iter().any(|w| w.as_str() == Some("main")),
        "权限必须作用于 main 窗口"
    );
}
