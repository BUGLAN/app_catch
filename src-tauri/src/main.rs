// Windows release 构建时隐藏额外的控制台窗口，勿删。
#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]

fn main() {
    app_catch_lib::run()
}
