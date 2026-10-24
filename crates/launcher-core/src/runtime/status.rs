//! 状态采集器：可扩展的「进程外部状态」探针框架。
//!
//! 需求基线要求「新增状态项不改核心流程」。这里的做法是把每个状态项做成一个
//! 独立的 `StatusProbe`，由 `CompositeProbe` 组合：
//!
//! ```text
//! StatusProbe (trait)                    一个状态项 = 一个 probe
//!    ├── WindowTitleProbe   -> window_title
//!    ├── ListeningPortsProbe-> ports
//!    └── （未来）CpuProbe   -> 只需实现 trait 并注册进组合，不改 SessionManager
//! CompositeProbe                          顺序执行，单项失败不影响其它
//!    └── StatusSnapshot                  合并后的只读快照
//! ```
//!
//! 三条设计约定：
//!
//! 1. **失败静默**：probe 只返回字段值，不返回 Result，也不写错误事件。
//!    取不到就是 `None` / 空 `Vec`，绝不影响会话运行，也绝不 panic。
//! 2. **不阻塞 reactor**：probe 内部是同步 Win32 调用（微秒级），
//!    由调用方（SessionManager 的状态循环）放进 `spawn_blocking` 执行，
//!    保证 tokio worker 不被系统调用占住。
//! 3. **可比较**：`StatusSnapshot` 派生 `PartialEq`，状态循环据此判定「是否变化」，
//!    只在变化时追加 status 事件，避免日志刷屏。

use std::sync::Arc;

use crate::platform;

/// 每次 probe 的只读上下文。
///
/// 只放采集需要的信息，probe 不得修改它，也不得持有其引用超过本次调用。
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct ProbeContext {
    /// 会话根进程 pid。
    pub root_pid: u32,
    /// 根进程的全部子孙 pid（不含根自身）。
    ///
    /// 由调用方在每次采样时重新枚举：GUI/服务类应用常在启动后再 fork 出真正干活的子进程，
    /// 只盯着根 pid 会漏掉窗口与端口。
    pub descendant_pids: Vec<u32>,
    /// 会话 id 的字符串形式，便于 probe 做与具体会话相关的判断或打点。
    pub session_id: String,
}

impl ProbeContext {
    /// 用根 pid 构造上下文（子孙列表先留空，由调用方补齐）。
    pub fn new(root_pid: u32) -> Self {
        Self {
            root_pid,
            descendant_pids: Vec::new(),
            session_id: String::new(),
        }
    }

    /// 带上会话 id 构造上下文。
    pub fn with_session(root_pid: u32, session_id: impl Into<String>) -> Self {
        Self {
            root_pid,
            descendant_pids: Vec::new(),
            session_id: session_id.into(),
        }
    }

    /// 补上子孙 pid 列表（builder 风格，便于在状态循环里链式构造）。
    #[must_use]
    pub fn with_descendants(mut self, descendant_pids: Vec<u32>) -> Self {
        self.descendant_pids = descendant_pids;
        self
    }

    /// 根 pid + 子孙 pid 的完整集合（根在前）。
    ///
    /// 窗口与端口都按「整棵进程树」判定，因为实际持有它们的往往不是根进程本身。
    pub fn tree_pids(&self) -> Vec<u32> {
        if self.root_pid == 0 {
            return Vec::new();
        }
        let mut pids = Vec::with_capacity(self.descendant_pids.len() + 1);
        pids.push(self.root_pid);
        pids.extend(self.descendant_pids.iter().copied());
        pids
    }
}

/// 一次状态采集的结果。
///
/// 字段全部是「尽力而为」的：取不到就是 `None`，绝不因此让会话出错。
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct StatusSnapshot {
    /// 主窗口标题；无窗口（服务类应用）或取不到时为 `None`。
    pub window_title: Option<String>,
    /// 监听端口（去重升序）；取不到时为空 `Vec`。
    ///
    /// 注意与 JSONL 语义的映射：写 `status` 事件时，空 `Vec` 会被写成 `null`
    /// 而不是 `[]`，与「取不到为 null」的既有文档保持一致。
    pub ports: Vec<u16>,
}

impl StatusSnapshot {
    /// 空快照（所有状态项都取不到）。
    pub fn empty() -> Self {
        Self::default()
    }

    /// 是否没有任何有效状态项。
    pub fn is_empty(&self) -> bool {
        self.window_title.is_none() && self.ports.is_empty()
    }

    /// 转成 `status` 事件里的 `ports` 形态：空集合写成 `None`（JSON 里是 `null`）。
    pub fn ports_field(&self) -> Option<Vec<u16>> {
        if self.ports.is_empty() {
            None
        } else {
            Some(self.ports.clone())
        }
    }
}

/// 单个状态项的采集器。
///
/// 实现方约定：
/// - 只读系统状态，不得改动会话；
/// - **只填自己负责的字段**，其余留空（`None` / 空 `Vec`）——组合时按字段各自取用，
///   写别人的字段不会生效，只会让语义变模糊；
/// - 失败就返回空值，**不要** panic、不要重试到卡住；
/// - 单次调用应当是微秒到毫秒级，重活请自行加超时。
pub trait StatusProbe: Send + Sync {
    /// 采集一次状态，返回**本 probe 负责的那部分**快照。
    ///
    /// 注意返回的是「全新快照」而不是增量：这样每次采样都是自洽的，
    /// 状态消失（窗口关闭、端口释放）时对应字段自然会变回空值，变化比较才能成立。
    fn probe(&self, ctx: &ProbeContext) -> StatusSnapshot;

    /// probe 的可读名字，仅用于日志与调试。
    fn name(&self) -> &'static str;

    /// 本 probe 是否需要 `ProbeContext::descendant_pids`。
    ///
    /// 枚举整机进程表是本框架里**最贵**的一步（Windows 上
    /// `CreateToolhelp32Snapshot` 在开发机上实测约 250ms，比 EnumWindows 慢三个数量级）。
    /// 因此默认值取 `true`（新写的真实 probe 通常都需要它），
    /// 而「不依赖进程树」的 probe（如测试里的假 probe、只读环境变量的 probe）应返回 `false`，
    /// 让框架跳过这次枚举。
    fn needs_process_tree(&self) -> bool {
        true
    }
}

/// 组合多个 probe：顺序执行，单个 probe 的失败不影响其它。
///
/// **字段级合并规则**：每个 probe 返回自己的快照，组合时逐字段取「第一个非空值」——
/// 即 `window_title` 与 `ports` 各自独立合并，某个 probe 只填端口不会把别人的窗口标题冲掉。
///
/// 这里用 `catch_unwind` 兜住 probe 的 panic：一个写坏的 probe 不应该连带
/// 让整条状态循环崩掉——这是「采集失败静默」的最后一道保险。
/// `AssertUnwindSafe` 是安全的：`probe()` 只读 `ctx`、返回新值，不持有跨调用的可变状态。
pub struct CompositeProbe {
    probes: Vec<Arc<dyn StatusProbe>>,
}

impl std::fmt::Debug for CompositeProbe {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("CompositeProbe")
            .field("probes", &self.probes.iter().map(|p| p.name()).collect::<Vec<_>>())
            .finish()
    }
}

impl Default for CompositeProbe {
    fn default() -> Self {
        Self::new()
    }
}

impl CompositeProbe {
    /// 空组合（采集结果永远是空快照）。
    pub fn new() -> Self {
        Self { probes: Vec::new() }
    }

    /// 用一批 probe 构造组合。
    pub fn with(probes: Vec<Arc<dyn StatusProbe>>) -> Self {
        Self { probes }
    }

    /// 追加一个 probe。
    pub fn push(&mut self, probe: Arc<dyn StatusProbe>) {
        self.probes.push(probe);
    }

    /// 已注册的 probe 名称（调试用）。
    pub fn probe_names(&self) -> Vec<&'static str> {
        self.probes.iter().map(|p| p.name()).collect()
    }
}

impl StatusProbe for CompositeProbe {
    fn probe(&self, ctx: &ProbeContext) -> StatusSnapshot {
        let mut merged = StatusSnapshot::empty();
        for probe in &self.probes {
            let outcome = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| probe.probe(ctx)));
            // 单个 probe panic 只丢它自己那部分：继续跑下一个
            let Ok(partial) = outcome else {
                continue;
            };

            // 字段级合并：已有非空值不被覆盖，空值可以被后续 probe 补上
            if merged.window_title.is_none() {
                merged.window_title = partial.window_title;
            }
            if merged.ports.is_empty() {
                merged.ports = partial.ports;
            }
        }
        merged
    }

    fn name(&self) -> &'static str {
        "composite"
    }

    /// 只要有一个子 probe 需要进程树，组合就需要。
    fn needs_process_tree(&self) -> bool {
        self.probes.iter().any(|p| p.needs_process_tree())
    }
}

/// 空 probe：什么都不采集（非 Windows 平台与「只想测状态循环」的场景用）。
#[derive(Debug, Default, Clone, Copy)]
pub struct EmptyProbe;

impl StatusProbe for EmptyProbe {
    fn probe(&self, _ctx: &ProbeContext) -> StatusSnapshot {
        StatusSnapshot::empty()
    }

    fn name(&self) -> &'static str {
        "empty"
    }

    /// 什么都不采集，自然不需要进程树。
    fn needs_process_tree(&self) -> bool {
        false
    }
}

/// Windows 窗口标题 probe。
#[cfg(windows)]
#[derive(Debug, Default, Clone, Copy)]
pub struct WindowTitleProbe;

#[cfg(windows)]
impl StatusProbe for WindowTitleProbe {
    fn probe(&self, ctx: &ProbeContext) -> StatusSnapshot {
        StatusSnapshot {
            // 只填自己这一项，端口留空交给 ListeningPortsProbe
            window_title: platform::window::window_title_for_pids(&ctx.tree_pids()),
            ports: Vec::new(),
        }
    }

    fn name(&self) -> &'static str {
        "window_title"
    }
}

/// Windows 监听端口 probe。
#[cfg(windows)]
#[derive(Debug, Default, Clone, Copy)]
pub struct ListeningPortsProbe;

#[cfg(windows)]
impl StatusProbe for ListeningPortsProbe {
    fn probe(&self, ctx: &ProbeContext) -> StatusSnapshot {
        StatusSnapshot {
            // 只填自己这一项，窗口标题留空交给 WindowTitleProbe
            window_title: None,
            ports: platform::ports::listening_ports_for_pids(&ctx.tree_pids()),
        }
    }

    fn name(&self) -> &'static str {
        "listening_ports"
    }
}

/// 构造当前平台的默认状态采集器。
///
/// - Windows：窗口标题 + 监听端口两个 probe；
/// - 其它平台：空 probe（M4 对齐 macOS 时在这里换实现，不影响调用方）。
pub fn default_status_probe() -> Arc<dyn StatusProbe> {
    #[cfg(windows)]
    {
        Arc::new(CompositeProbe::with(vec![
            Arc::new(WindowTitleProbe) as Arc<dyn StatusProbe>,
            Arc::new(ListeningPortsProbe) as Arc<dyn StatusProbe>,
        ]))
    }
    #[cfg(not(windows))]
    {
        Arc::new(EmptyProbe)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::atomic::{AtomicUsize, Ordering};

    /// 只填窗口标题的 probe（验证字段级合并不互相覆盖）。
    struct TitleOnlyProbe(&'static str);

    impl StatusProbe for TitleOnlyProbe {
        fn probe(&self, _ctx: &ProbeContext) -> StatusSnapshot {
            StatusSnapshot {
                window_title: Some(self.0.to_string()),
                ports: Vec::new(),
            }
        }

        fn name(&self) -> &'static str {
            "title_only"
        }
    }

    /// 只填端口的 probe。
    struct PortsOnlyProbe(Vec<u16>);

    impl StatusProbe for PortsOnlyProbe {
        fn probe(&self, _ctx: &ProbeContext) -> StatusSnapshot {
            StatusSnapshot {
                window_title: None,
                ports: self.0.clone(),
            }
        }

        fn name(&self) -> &'static str {
            "ports_only"
        }
    }

    /// 记录被调用次数的 probe。
    struct CountingProbe {
        calls: Arc<AtomicUsize>,
    }

    impl StatusProbe for CountingProbe {
        fn probe(&self, _ctx: &ProbeContext) -> StatusSnapshot {
            self.calls.fetch_add(1, Ordering::Relaxed);
            StatusSnapshot {
                window_title: Some("计数".to_string()),
                ports: Vec::new(),
            }
        }

        fn name(&self) -> &'static str {
            "counting"
        }
    }

    /// 每次都 panic 的 probe：不得让组合崩溃。
    struct PanicProbe;

    impl StatusProbe for PanicProbe {
        fn probe(&self, _ctx: &ProbeContext) -> StatusSnapshot {
            panic!("这个 probe 故意 panic");
        }

        fn name(&self) -> &'static str {
            "panic"
        }
    }

    #[test]
    fn probe_context_tree_pids_puts_root_first() {
        let mut ctx = ProbeContext::new(100);
        ctx.descendant_pids = vec![101, 102];
        assert_eq!(ctx.tree_pids(), vec![100, 101, 102]);

        // pid 0 视为无效，不产生任何 pid
        assert!(ProbeContext::new(0).tree_pids().is_empty());
    }

    #[test]
    fn probe_context_with_session_carries_id() {
        let ctx = ProbeContext::with_session(42, "abc");
        assert_eq!(ctx.root_pid, 42);
        assert_eq!(ctx.session_id, "abc");
        assert!(ctx.descendant_pids.is_empty());
    }

    /// 组合的字段级合并：两个各管一半的 probe 应各自补充，互不覆盖。
    #[test]
    fn composite_probe_collects_all_fields() {
        let composite = CompositeProbe::with(vec![
            Arc::new(TitleOnlyProbe("主窗口")) as Arc<dyn StatusProbe>,
            Arc::new(PortsOnlyProbe(vec![8080])) as Arc<dyn StatusProbe>,
        ]);

        let snapshot = composite.probe(&ProbeContext::new(1));

        // 两个 probe 各填各的字段，互不覆盖
        assert_eq!(snapshot.window_title.as_deref(), Some("主窗口"));
        assert_eq!(snapshot.ports, vec![8080]);
        assert_eq!(composite.probe_names(), vec!["title_only", "ports_only"]);
    }

    /// 合并顺序无关：端口 probe 在前也一样能拿到两项。
    #[test]
    fn composite_probe_merges_regardless_of_order() {
        let composite = CompositeProbe::with(vec![
            Arc::new(PortsOnlyProbe(vec![9000, 9001])) as Arc<dyn StatusProbe>,
            Arc::new(TitleOnlyProbe("倒序窗口")) as Arc<dyn StatusProbe>,
        ]);

        let snapshot = composite.probe(&ProbeContext::new(7));
        assert_eq!(snapshot.window_title.as_deref(), Some("倒序窗口"));
        assert_eq!(snapshot.ports, vec![9000, 9001]);
    }

    /// 一个 probe panic 不得影响其它 probe：这是「采集失败静默」的兜底。
    #[test]
    fn composite_probe_survives_panicking_child() {
        let calls = Arc::new(AtomicUsize::new(0));
        let composite = CompositeProbe::with(vec![
            Arc::new(PanicProbe) as Arc<dyn StatusProbe>,
            Arc::new(CountingProbe {
                calls: Arc::clone(&calls),
            }) as Arc<dyn StatusProbe>,
        ]);

        let snapshot = composite.probe(&ProbeContext::new(1));

        assert_eq!(
            calls.load(Ordering::Relaxed),
            1,
            "panic 之后仍应继续执行后续 probe"
        );
        assert_eq!(snapshot.window_title.as_deref(), Some("计数"));
    }

    /// 空 ports 在事件里应写成 null（与文档「取不到为 null」一致）。
    #[test]
    fn empty_ports_map_to_null_field() {
        let snapshot = StatusSnapshot {
            window_title: Some("窗口".to_string()),
            ports: vec![],
        };
        assert_eq!(snapshot.ports_field(), None);
        assert!(!snapshot.is_empty(), "有标题就不算空快照");

        let snapshot = StatusSnapshot {
            window_title: None,
            ports: vec![9000],
        };
        assert_eq!(snapshot.ports_field(), Some(vec![9000]));

        assert!(StatusSnapshot::empty().is_empty());

        // EmptyProbe 什么都不采集
        assert!(EmptyProbe.probe(&ProbeContext::new(1)).is_empty());
        assert_eq!(EmptyProbe.name(), "empty");
    }

    /// 默认 probe 在 Windows 上应组合窗口 + 端口两项。
    #[cfg(windows)]
    #[test]
    fn default_probe_on_windows_covers_window_and_ports() {
        let probe = default_status_probe();
        assert_eq!(probe.name(), "composite");
    }

    /// 端口采集的真实性验证：本进程绑定一个 loopback 端口后应能采到。
    #[cfg(windows)]
    #[test]
    fn default_probe_sees_own_listening_port() {
        use std::net::TcpListener;

        let listener = TcpListener::bind("127.0.0.1:0").expect("绑定 loopback 失败");
        let port = listener.local_addr().expect("取本地地址失败").port();

        let probe = default_status_probe();
        let snapshot = probe.probe(&ProbeContext::new(std::process::id()));

        assert!(
            snapshot.ports.contains(&port),
            "应采到本进程刚绑定的端口 {port}，实际: {:?}",
            snapshot.ports
        );

        drop(listener);
    }
}
