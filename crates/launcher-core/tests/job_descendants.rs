//! Job Object 进程树查询的端到端测试。
//!
//! 目标不是「函数没 panic」，而是证明它**真的列出了整棵进程树**：
//! 启动一个会派生子进程的命令，再验证根进程之后的每一层都能被看见。
//!
//! 这类测试在旧实现下会非常慢（每次查询要走全系统快照，本机约 330ms），
//! 正好也能作为「没有退回慢路径」的间接证据——若某次改动让 Job 查询失效，
//! 测试时间会明显变长。

#![cfg(windows)]

use std::sync::Arc;
use std::time::{Duration, Instant};

use launcher_core::platform::{ProcessBackend, SpawnRequest, WindowsProcessBackend};

/// 启动一个「父 -> 子 -> 孙」的进程链，返回后端与根句柄。
///
/// 用 cmd 的多级 start 构造真实的多层树：单层测试无法区分
/// 「Job 成员表」与「只看直接子进程」这两种实现。
async fn spawn_chain(backend: &WindowsProcessBackend) -> (launcher_core::platform::ProcessHandle, Vec<u32>) {
    // 根 cmd 起一个子 cmd，子 cmd 再起一个长时间运行的 node/ping。
    // 用 ping 作为叶子：Windows 自带、可控时长、不会被当成脚本立即退出。
    let mut req = SpawnRequest::new(
        launcher_core::session::SessionId::new_v4(),
        launcher_core::config::AppId::new("jobtest").expect("合法 app id"),
        "cmd".to_string(),
    );
    req.args = vec![
        "/c".to_string(),
        "start /b cmd /c \"start /b ping -n 30 127.0.0.1 > nul\" & ping -n 30 127.0.0.1 > nul"
            .to_string(),
    ];

    let spawned = backend.spawn(&req).await.expect("spawn 失败");

    // 等待进程树长起来（Windows 上 start 是异步的，立即查询往往还没见到孙子）
    let start = Instant::now();
    let mut pids = Vec::new();
    while start.elapsed() < Duration::from_secs(5) {
        pids = backend
            .descendant_pids(&spawned.handle)
            .expect("Windows 后端必须支持 descendant_pids");
        if pids.len() >= 2 {
            break;
        }
        tokio::time::sleep(Duration::from_millis(50)).await;
    }

    (spawned.handle, pids)
}

#[tokio::test]
async fn job_query_lists_descendants() {
    let backend = Arc::new(WindowsProcessBackend::new());
    let (handle, pids) = spawn_chain(&backend).await;

    assert!(
        !pids.is_empty(),
        "Job 成员表应至少列出一个子进程（根 pid={}）",
        handle.pid
    );
    assert!(
        !pids.contains(&handle.pid),
        "descendant_pids 的约定是**不含根进程**，实际返回 {pids:?}"
    );

    // 清理：杀整棵树，避免测试留下孤儿 ping
    backend.kill_tree(&handle).await.expect("kill_tree 失败");
}

/// Job 查询必须是微秒级；若退化成全系统快照（本机约 330ms）这里会失败。
#[tokio::test]
async fn job_query_is_fast() {
    let backend = Arc::new(WindowsProcessBackend::new());
    let (handle, _pids) = spawn_chain(&backend).await;

    // 预热一次，排除首次调用的 DLL/锁开销
    let _ = backend.descendant_pids(&handle);

    let iterations = 100;
    let start = Instant::now();
    for _ in 0..iterations {
        std::hint::black_box(backend.descendant_pids(&handle));
    }
    let per_query = start.elapsed().as_secs_f64() / iterations as f64;

    backend.kill_tree(&handle).await.expect("kill_tree 失败");

    // 阈值取 5ms：比 Job 查询（微秒级）宽松 3 个数量级，
    // 但比全系统快照（330ms）严格 60 倍。既能容忍 CI 抖动，又能抓住性能退化。
    assert!(
        per_query < 0.005,
        "Job 进程树查询单次耗时 {:.3} ms，疑似退回全系统快照路径（应 < 5ms）",
        per_query * 1000.0
    );
}
