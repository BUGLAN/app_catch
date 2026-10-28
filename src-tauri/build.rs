fn main() {
    /*
     * Windows 的 exe 图标是在构建脚本里编进资源段的（tauri-build 生成 resource.rc → resource.lib），
     * 但 tauri-build 只为 tauri.conf.json 与 capabilities/ 声明了 rerun-if-changed。
     * 只换 src-tauri/icons 下文件时 Cargo 不会重跑构建脚本，resource.lib 保持旧内容，
     * 于是 exe 里一直嵌着换图前的图标 —— 表现就是「macOS 的图标已经换了，Windows 还是旧的紫色」。
     * 这里显式声明图标依赖，保证换图后必然重新编码资源并重链接。
     */
    for icon in [
        "icons/icon.ico",
        "icons/icon.icns",
        "icons/icon.png",
        "icons/32x32.png",
        "icons/64x64.png",
        "icons/128x128.png",
        "icons/128x128@2x.png",
        "icons/Square44x44Logo.png",
        "icons/Square150x150Logo.png",
        "icons/StoreLogo.png",
    ] {
        if std::path::Path::new(icon).exists() {
            println!("cargo:rerun-if-changed={icon}");
        }
    }

    // Tauri 2 构建脚本：负责生成上下文、编译前端产物前的资源等
    tauri_build::build()
}
