//! 回归：子进程一口气打印 3 万行时，捕获管线不应大量丢行。
//!
//! 背景：写盘任务曾经逐行 `write_all` 并每 64 行 `sync_all` 一次，只有每秒约 1.6 万条，
//! 读取端的有界通道（8192）很快积满，3 万行里丢掉约 65%（`examples/flood_bench.rs` 实测）。
//! 改成用户态缓冲 + 只在空闲时同步后，本机实测丢弃为 0。
//!
//! 阈值刻意放宽到 10%：CI 机器慢、负载不稳定，这里只防「回到逐行写盘」那种量级的退化。
//! 仅 Unix：Windows 的 `cmd` 循环本身每秒只打印几千行，构不成洪水。
#![cfg(unix)]

use std::process::Stdio;

use launcher_core::runtime::{CaptureConfig, CapturePipeline, LogWriter};
use tokio::io::AsyncRead;

const LINES: u64 = 30_000;

#[tokio::test(flavor = "multi_thread")]
async fn flood_of_short_lines_is_captured_almost_entirely() {
    let dir = tempfile::tempdir().expect("创建临时目录失败");
    let mut child = tokio::process::Command::new("/bin/sh")
        .args([
            "-c",
            &format!(
                "i=0; while [ $i -lt {LINES} ]; do echo \"flood line $i lorem ipsum dolor sit amet\"; i=$((i+1)); done"
            ),
        ])
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .expect("启动洪水进程失败");
    let stdout = child
        .stdout
        .take()
        .map(|s| Box::new(s) as Box<dyn AsyncRead + Send + Unpin>);
    let stderr = child
        .stderr
        .take()
        .map(|s| Box::new(s) as Box<dyn AsyncRead + Send + Unpin>);

    let config = CaptureConfig::default();
    let writer = LogWriter::open(dir.path(), config.writer.clone())
        .await
        .expect("打开日志失败");
    let pipeline = CapturePipeline::start(stdout, stderr, writer, config);
    let _ = child.wait().await;
    let summary = pipeline.finish().await.expect("收尾失败");

    assert_eq!(summary.stdout_lines, LINES, "读取端应读到全部 {LINES} 行");
    assert!(
        summary.dropped_lines * 10 < LINES,
        "丢弃 {} 行（{:.1}%），超过 10%：写盘吞吐退化了",
        summary.dropped_lines,
        summary.dropped_lines as f64 * 100.0 / LINES as f64
    );

    // 落盘的 output 行数 = 读到的 - 丢弃的，且每行都完整可解析
    let text = std::fs::read_to_string(dir.path().join("events.jsonl")).expect("读日志失败");
    let mut outputs = 0u64;
    for line in text.lines() {
        let value: serde_json::Value =
            serde_json::from_str(line).unwrap_or_else(|e| panic!("存在不可解析的行: {e}\n{line}"));
        if value["event"] == "output" {
            outputs += 1;
        }
    }
    assert_eq!(outputs, LINES - summary.dropped_lines);
}
