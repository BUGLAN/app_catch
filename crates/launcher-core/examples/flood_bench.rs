//! 输出洪水压测：子进程一口气打印大量短行，统计捕获管线丢了多少行、写盘有多快。
//!
//! 用来回答「写盘任务跟不跟得上」：读取端用 `try_send`（通道满就丢并记账，绝不阻塞子进程），
//! 因此写盘吞吐不够时，丢弃数会直接暴露出来。
//!
//! 用法：
//!
//! ```text
//! cargo run -p launcher-core --release --example flood_bench            # 默认 30000 行，跑 3 轮
//! cargo run -p launcher-core --release --example flood_bench -- 100000 5
//! ```
//!
//! 输出每轮的 丢弃行数 / 写入事件数 / 耗时，以及写盘吞吐（事件/秒）。
//! 设置环境变量 `BENCH_NO_SYNC=1` 可关掉按行数写出与定时磁盘同步，只剩缓冲满 / 队列空闲时写出，
//! 用来对比这两项开销。
//!
//! 参考数据（Apple Silicon，release）：逐行写盘 + 每 64 行 `sync_all` 的旧实现 3 万行丢约 65%；
//! 用户态缓冲 + 空闲时才同步后 3 万行、20 万行均为 0 丢弃，约 16 万事件/秒（瓶颈变成了 shell 本身）。

use std::process::Stdio;
use std::time::Instant;

use launcher_core::runtime::{CaptureConfig, CapturePipeline, LogWriter};
use tokio::io::AsyncRead;

/// 一段尽可能快地打印 `lines` 行的 shell 命令。
fn flood_command(lines: u64) -> tokio::process::Command {
    #[cfg(unix)]
    {
        let mut command = tokio::process::Command::new("/bin/sh");
        command.args([
            "-c",
            &format!(
                "i=0; while [ $i -lt {lines} ]; do echo \"flood line $i lorem ipsum dolor sit amet\"; i=$((i+1)); done"
            ),
        ]);
        command
    }
    #[cfg(windows)]
    {
        let mut command = tokio::process::Command::new("cmd");
        command.args([
            "/D",
            "/C",
            &format!(
                "for /L %i in (1,1,{lines}) do @echo flood line %i lorem ipsum dolor sit amet"
            ),
        ]);
        command
    }
}

struct Round {
    dropped: u64,
    written: u64,
    elapsed_ms: u128,
}

async fn run_round(lines: u64) -> Round {
    let dir = tempfile::tempdir().expect("创建临时目录失败");
    let mut child = flood_command(lines)
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

    let mut config = CaptureConfig::default();
    if std::env::var_os("BENCH_NO_SYNC").is_some() {
        config.writer.flush_every = usize::MAX / 2;
        config.writer.flush_interval = std::time::Duration::from_secs(3600);
    }
    let writer = LogWriter::open(dir.path(), config.writer.clone())
        .await
        .expect("打开日志失败");
    let started = Instant::now();
    // 与真实会话一致：带上 append 通知（这里只是空回调）
    let pipeline = CapturePipeline::start_with_notify(
        stdout,
        stderr,
        writer,
        config,
        Some(std::sync::Arc::new(|| {})),
    );
    let _ = child.wait().await;
    let summary = pipeline.finish().await.expect("收尾失败");
    Round {
        dropped: summary.dropped_lines,
        written: summary.written_events,
        elapsed_ms: started.elapsed().as_millis(),
    }
}

#[tokio::main]
async fn main() {
    let mut args = std::env::args().skip(1);
    let lines: u64 = args.next().and_then(|v| v.parse().ok()).unwrap_or(30_000);
    let rounds: u32 = args.next().and_then(|v| v.parse().ok()).unwrap_or(3);

    println!("flood_bench: {lines} 行 × {rounds} 轮");
    for round in 1..=rounds {
        let result = run_round(lines).await;
        let rate = (u128::from(result.written) * 1000)
            .checked_div(result.elapsed_ms)
            .unwrap_or(0);
        println!(
            "第 {round} 轮：丢弃 {} 行（{:.1}%），写入 {} 条事件，耗时 {} ms，约 {rate} 事件/秒",
            result.dropped,
            result.dropped as f64 * 100.0 / lines as f64,
            result.written,
            result.elapsed_ms,
        );
    }
}
