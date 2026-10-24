//! Windows 监听端口采集：从系统 TCP 表中筛出目标进程树持有的监听端口。
//!
//! 数据源是 `GetExtendedTcpTable`（iphlpapi），表类为 `TCP_TABLE_OWNER_PID_LISTENER`，
//! 它一次性给出「本地地址 + 本地端口 + 状态 + 归属 pid」，不需要逐个进程去猜。
//!
//! 两阶段调用（Win32 的固定套路）：
//!
//! ```text
//! 1) GetExtendedTcpTable(null, &size, ...)  -> ERROR_INSUFFICIENT_BUFFER，size 得到所需字节数
//! 2) 按 size 分配缓冲区，再调用一次           -> ERROR_SUCCESS，缓冲区里是 MIB_TCPTABLE_OWNER_PID
//! ```
//!
//! 过滤条件（三条同时满足）：
//!
//! | 条件 | 取值 | 理由 |
//! | --- | --- | --- |
//! | 本地地址 | `INADDR_ANY`（0.0.0.0 / ::）或回环（127.0.0.0/8 / ::1） | 只关心「本机可访问的服务端口」 |
//! | 状态 | `MIB_TCP_STATE_LISTEN` | 监听中才算「应用在提供服务」 |
//! | 归属 pid | 属于目标会话的进程树 | 排除系统与其它应用的端口 |
//!
//! IPv4 与 IPv6 分别调用一次（`AF_INET` / `AF_INET6`），结果合并后去重排序。
//!
//! 本模块是**尽力而为**的：任何一次查询失败都只影响该地址族的贡献，
//! 两族都失败时返回空 `Vec`，绝不报错、绝不 panic。
//!
//! 资源释放：缓冲区是 `Vec<u8>`（RAII 自动释放）；本模块不打开任何需要 CloseHandle 的内核句柄。

#![allow(unsafe_code)]

use core::ffi::c_void;

use windows_sys::Win32::Foundation::{GetLastError, ERROR_INSUFFICIENT_BUFFER, ERROR_SUCCESS};
use windows_sys::Win32::NetworkManagement::IpHelper::{
    GetExtendedTcpTable, MIB_TCP6ROW_OWNER_PID, MIB_TCP6TABLE_OWNER_PID, MIB_TCPROW_OWNER_PID,
    MIB_TCPTABLE_OWNER_PID, MIB_TCP_STATE_LISTEN, TCP_TABLE_OWNER_PID_LISTENER,
};
use windows_sys::Win32::Networking::WinSock::{AF_INET, AF_INET6};

/// IPv4 的「任意地址」：`INADDR_ANY` 在小端存储下就是 0。
const INADDR_ANY: u32 = 0;

/// IPv4 回环地址段 `127.0.0.0/8` 的网络序首字节。
const IPV4_LOOPBACK_PREFIX: u32 = 127;

/// 一次 `GetExtendedTcpTable` 调用的最大重试次数。
///
/// 两次调用之间系统表可能变大（新端口被绑定），导致第二次仍返回
/// `ERROR_INSUFFICIENT_BUFFER`；重试几次即可收敛，避免无限循环。
const MAX_TABLE_ATTEMPTS: usize = 4;

/// 从 `dwLocalPort` 里解出端口号。
///
/// Win32 的端口字段是**网络序**存放在一个 u32 的低 16 位里：
/// 高位是 0，低 16 位按大端排列。因此取低 16 位后做一次字节序翻转。
fn port_from_network_order(raw: u32) -> u16 {
    let low = (raw & 0xFFFF) as u16;
    low.swap_bytes()
}

/// IPv4 地址是否属于「任意地址或回环」。
///
/// `dwLocalAddr` 是网络序（大端）存放的 u32，因此取**内存首字节**判断回环段。
fn ipv4_is_wildcard_or_loopback(addr: u32) -> bool {
    if addr == INADDR_ANY {
        return true;
    }
    // 小端机器上 addr 的最低字节就是网络序的第一个字节（即 127.0.0.1 的 127）
    (addr & 0xFF) as u32 == IPV4_LOOPBACK_PREFIX
}

/// IPv6 地址是否属于「任意地址或回环」。
///
/// `::`（全 0）是任意地址，`::1` 是回环地址。
fn ipv6_is_wildcard_or_loopback(addr: &[u8; 16]) -> bool {
    if addr.iter().all(|b| *b == 0) {
        return true;
    }
    // ::1 -> 前 15 字节为 0，最后 1 字节为 1
    addr[..15].iter().all(|b| *b == 0) && addr[15] == 1
}

/// 调用 `GetExtendedTcpTable` 拉取一次完整的监听表原始字节。
///
/// 返回 `None` 表示查询失败（权限、系统调用错误、重试耗尽），调用方静默跳过。
fn fetch_listener_table(af: u32) -> Option<Vec<u8>> {
    let mut size: u32 = 0;
    // 第一次调用：只为拿所需缓冲区大小，预期返回 ERROR_INSUFFICIENT_BUFFER
    // SAFETY: 传入空表指针 + 有效 size 指针；此调用不会写入任何表数据。
    let first = unsafe {
        GetExtendedTcpTable(
            core::ptr::null_mut::<c_void>(),
            &mut size,
            0,
            af,
            TCP_TABLE_OWNER_PID_LISTENER,
            0,
        )
    };
    if first != ERROR_INSUFFICIENT_BUFFER || size == 0 {
        return None;
    }

    for _ in 0..MAX_TABLE_ATTEMPTS {
        // 多留一点余量：两次调用之间表可能变大
        let capacity = size as usize + core::mem::size_of::<usize>();
        let mut buffer: Vec<u8> = vec![0; capacity];
        let mut available = capacity as u32;

        // SAFETY: buffer 至少 available 字节可写，available 与之一致；
        // 调用成功后 buffer 内是 MIB_TCP*TABLE_OWNER_PID 结构，下面按同一布局读取。
        let status = unsafe {
            GetExtendedTcpTable(
                buffer.as_mut_ptr() as *mut c_void,
                &mut available,
                0,
                af,
                TCP_TABLE_OWNER_PID_LISTENER,
                0,
            )
        };

        match status {
            ERROR_SUCCESS => return Some(buffer),
            // 表在两次调用之间变大了：按新尺寸重试
            ERROR_INSUFFICIENT_BUFFER => {
                size = available;
                continue;
            }
            _ => return None,
        }
    }
    // 重试耗尽仍然拿不到稳定尺寸：放弃，按「查不到端口」处理
    // SAFETY: GetLastError 无副作用，任意线程可调用；这里只用于留痕。
    let _ = unsafe { GetLastError() };
    None
}

/// 过滤一条 IPv4 监听记录；命中时返回端口号。
fn filter_v4(row: &MIB_TCPROW_OWNER_PID, pids: &[u32]) -> Option<u16> {
    if row.dwState != MIB_TCP_STATE_LISTEN as u32 {
        return None;
    }
    if !pids.contains(&row.dwOwningPid) {
        return None;
    }
    if !ipv4_is_wildcard_or_loopback(row.dwLocalAddr) {
        return None;
    }
    Some(port_from_network_order(row.dwLocalPort))
}

/// 过滤一条 IPv6 监听记录；命中时返回端口号。
fn filter_v6(row: &MIB_TCP6ROW_OWNER_PID, pids: &[u32]) -> Option<u16> {
    if row.dwState != MIB_TCP_STATE_LISTEN as u32 {
        return None;
    }
    if !pids.contains(&row.dwOwningPid) {
        return None;
    }
    if !ipv6_is_wildcard_or_loopback(&row.ucLocalAddr) {
        return None;
    }
    Some(port_from_network_order(row.dwLocalPort))
}

/// 从 `MIB_TCPTABLE_OWNER_PID` 布局的缓冲区里读出全部条目。
///
/// 缓冲区布局：开头 4 字节是 `dwNumEntries`，随后是 $N$ 个连续的 `MIB_TCPROW_OWNER_PID`。
/// 最后一个条目只保证 1 条的空间，因此必须按 `dwNumEntries` 逐个 shift 读取，
/// 不能把整块缓冲区当成 `MIB_TCPTABLE_OWNER_PID` 直接索引（会越界读）。
fn parse_v4(buffer: &[u8], pids: &[u32]) -> Vec<u16> {
    let header = core::mem::size_of::<u32>();
    if buffer.len() < header {
        return Vec::new();
    }

    // SAFETY: buffer.len() >= 4，按 u32 读取不会越界。
    let count = u32::from_ne_bytes([buffer[0], buffer[1], buffer[2], buffer[3]]) as usize;
    let row_size = core::mem::size_of::<MIB_TCPROW_OWNER_PID>();
    let mut ports = Vec::new();

    for i in 0..count {
        let offset = header + i * row_size;
        if offset + row_size > buffer.len() {
            // 缓冲区比声明条目数小：以实际可读范围为界，不越界也不 panic
            break;
        }
        // SAFETY: 已确认 [offset, offset+row_size) 落在 buffer 内且对齐要求
        // 由 MIB_TCPROW_OWNER_PID 的 repr(C) 布局满足（u32 数组本身自然对齐）。
        let row = unsafe { core::ptr::read_unaligned(buffer.as_ptr().add(offset) as *const MIB_TCPROW_OWNER_PID) };
        if let Some(port) = filter_v4(&row, pids) {
            ports.push(port);
        }
    }
    // 上一行仅用于消除未使用告警的占位（MIB_TCPTABLE_OWNER_PID 只用于文档对照）
    let _ = core::mem::size_of::<MIB_TCPTABLE_OWNER_PID>();
    ports
}

/// 从 `MIB_TCP6TABLE_OWNER_PID` 布局的缓冲区里读出全部条目。
fn parse_v6(buffer: &[u8], pids: &[u32]) -> Vec<u16> {
    let header = core::mem::size_of::<u32>();
    if buffer.len() < header {
        return Vec::new();
    }

    // SAFETY: buffer.len() >= 4，按 u32 读取不会越界。
    let count = u32::from_ne_bytes([buffer[0], buffer[1], buffer[2], buffer[3]]) as usize;
    let row_size = core::mem::size_of::<MIB_TCP6ROW_OWNER_PID>();
    let mut ports = Vec::new();

    for i in 0..count {
        let offset = header + i * row_size;
        if offset + row_size > buffer.len() {
            break;
        }
        // SAFETY: 已确认区间在 buffer 内；用 read_unaligned 避免对 u8 缓冲区做对齐假设。
        let row =
            unsafe { core::ptr::read_unaligned(buffer.as_ptr().add(offset) as *const MIB_TCP6ROW_OWNER_PID) };
        if let Some(port) = filter_v6(&row, pids) {
            ports.push(port);
        }
    }
    let _ = core::mem::size_of::<MIB_TCP6TABLE_OWNER_PID>();
    ports
}

/// 取目标进程树持有的监听端口（去重升序）；查不到返回空 `Vec`。
///
/// `pids` 应当包含根 pid 与其全部子孙 pid：Python 服务常见「python.exe 起子进程
/// 真正提供服务」，只看根 pid 会漏掉端口。
pub fn listening_ports_for_pids(pids: &[u32]) -> Vec<u16> {
    if pids.is_empty() {
        return Vec::new();
    }

    let mut ports: Vec<u16> = Vec::new();
    if let Some(buffer) = fetch_listener_table(AF_INET as u32) {
        ports.extend(parse_v4(&buffer, pids));
    }
    if let Some(buffer) = fetch_listener_table(AF_INET6 as u32) {
        ports.extend(parse_v6(&buffer, pids));
    }

    ports.sort_unstable();
    ports.dedup();
    ports
}

/// 取指定根 pid 及其子孙进程的监听端口；查不到返回空 `Vec`。
///
/// 这是给 `StatusProbe` 用的便捷入口。
pub fn listening_ports_for_pid(root_pid: u32) -> Vec<u16> {
    if root_pid == 0 {
        return Vec::new();
    }
    let mut pids = vec![root_pid];
    pids.extend(super::windows::descendant_pids(root_pid));
    listening_ports_for_pids(&pids)
}

#[cfg(test)]
mod tests {
    use super::*;

    /// 端口字段是网络序的低 16 位：8000 -> 0x1F40 -> 网络序 0x401F。
    #[test]
    fn network_order_port_is_decoded() {
        // 8000 = 0x1F40；网络序存成低 16 位 0x401F
        assert_eq!(port_from_network_order(0x401F), 8000);
        // 9880 = 0x2698；网络序低 16 位 0x9826
        assert_eq!(port_from_network_order(0x9826), 9880);
        // 80 = 0x0050；网络序低 16 位 0x5000
        assert_eq!(port_from_network_order(0x5000), 80);
    }

    /// IPv4：0.0.0.0（任意）与 127.x（回环）应通过，其它地址不通过。
    #[test]
    fn ipv4_filter_accepts_wildcard_and_loopback_only() {
        assert!(ipv4_is_wildcard_or_loopback(INADDR_ANY), "0.0.0.0 应通过");
        // 127.0.0.1 网络序大端首字节是 127，存放在 u32 的最低字节
        assert!(
            ipv4_is_wildcard_or_loopback(0x0100_007F),
            "127.0.0.1 应通过"
        );
        // 192.168.1.10 -> 首字节 192，不应通过
        assert!(
            !ipv4_is_wildcard_or_loopback(0x0A01_A8C0),
            "192.168.1.10 不应通过"
        );
    }

    /// IPv6：:: （任意）与 ::1（回环）通过，其余不通过。
    #[test]
    fn ipv6_filter_accepts_wildcard_and_loopback_only() {
        assert!(ipv6_is_wildcard_or_loopback(&[0u8; 16]), ":: 应通过");

        let mut loopback = [0u8; 16];
        loopback[15] = 1;
        assert!(ipv6_is_wildcard_or_loopback(&loopback), "::1 应通过");

        let mut other = [0u8; 16];
        other[0] = 0xfe;
        other[1] = 0x80;
        other[15] = 1;
        assert!(!ipv6_is_wildcard_or_loopback(&other), "fe80::1 不应通过");
    }

    /// 空 pid 列表直接返回空，不触发系统调用。
    #[test]
    fn empty_pid_list_yields_no_ports() {
        assert!(listening_ports_for_pids(&[]).is_empty());
        assert!(listening_ports_for_pid(0).is_empty());
    }

    /// 声明条目数超过缓冲区实际容量时不得越界读，也不得 panic。
    #[test]
    fn parser_tolerates_truncated_buffer() {
        // 声明 999 条，但缓冲区只有 4 字节头
        let mut buffer = 999u32.to_ne_bytes().to_vec();
        assert!(parse_v4(&buffer, &[1, 2, 3]).is_empty());
        assert!(parse_v6(&buffer, &[1, 2, 3]).is_empty());

        // 连 4 字节头都不够
        buffer.truncate(2);
        assert!(parse_v4(&buffer, &[1]).is_empty());
        assert!(parse_v6(&buffer, &[1]).is_empty());
    }
}
