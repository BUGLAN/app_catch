//! macOS 隐私权限相关配置的守护测试（见 docs/MACOS_PERMISSIONS.md）。
//!
//! 为什么值得测：从 app_catch 启动的程序请求麦克风等权限时，责任进程是 app_catch，
//! 它的 Info.plist 缺少对应用途说明，子进程就会被系统直接杀掉；
//! 而 hardened runtime 一旦被打开又没配 entitlements，用户授权了也拿不到。
//! 两者出错都只在安装版上表现出来，开发态（终端是责任进程）永远发现不了。

use std::path::PathBuf;

use serde_json::Value;
use tauri::utils::config::Config;

fn read_file(name: &str) -> String {
    let path = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join(name);
    std::fs::read_to_string(&path).unwrap_or_else(|e| panic!("读取 {name} 失败: {e}"))
}

/// 取 Info.plist 里某个键紧随其后的 `<string>` 值（只处理本文件这种扁平 dict）。
fn plist_string<'a>(plist: &'a str, key: &str) -> Option<&'a str> {
    let after_key = plist.split(&format!("<key>{key}</key>")).nth(1)?;
    let value = after_key.trim_start().strip_prefix("<string>")?;
    value.split("</string>").next().map(str::trim)
}

#[test]
fn info_plist_declares_common_usage_descriptions() {
    let plist = read_file("Info.plist");
    for key in [
        "NSMicrophoneUsageDescription",
        "NSCameraUsageDescription",
        "NSSpeechRecognitionUsageDescription",
        "NSBluetoothAlwaysUsageDescription",
        "NSLocationUsageDescription",
        "NSLocationWhenInUseUsageDescription",
        "NSContactsUsageDescription",
        "NSCalendarsUsageDescription",
        "NSCalendarsFullAccessUsageDescription",
        "NSRemindersUsageDescription",
        "NSRemindersFullAccessUsageDescription",
        "NSPhotoLibraryUsageDescription",
        "NSAppleEventsUsageDescription",
        "NSLocalNetworkUsageDescription",
    ] {
        let value = plist_string(&plist, key).unwrap_or_else(|| {
            panic!("Info.plist 缺少 {key}：被启动的程序请求该权限时会被系统杀掉")
        });
        assert!(!value.is_empty(), "{key} 的用途说明不能为空");
    }
}

#[test]
fn hardened_runtime_stays_off_without_entitlements() {
    let raw: Value = serde_json::from_str(&read_file("tauri.conf.json"))
        .expect("tauri.conf.json 必须是合法 JSON");
    let config: Config = serde_json::from_value(raw).expect("tauri.conf.json 必须能反序列化");
    let macos = &config.bundle.macos;
    // 自签名证书无法公证，hardened runtime 没有收益；开启它又不配 audio-input 等 entitlements，
    // 用户授权了麦克风，app_catch 启动的程序也拿不到。
    assert!(
        !macos.hardened_runtime || macos.entitlements.is_some(),
        "开启 hardened runtime 时必须同时配置 entitlements（audio-input / camera 等）"
    );
}

#[test]
fn bundle_identifier_is_stable() {
    // TCC 按 bundle id + 签名要求记账：改 identifier 等于让所有用户的隐私授权失效一次
    let raw: Value = serde_json::from_str(&read_file("tauri.conf.json"))
        .expect("tauri.conf.json 必须是合法 JSON");
    assert_eq!(
        raw["identifier"],
        Value::String("com.appcatch.launcher".into())
    );
}
