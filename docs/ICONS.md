# 图标（M3）：抓取策略、缓存规则与兜底语义

M3 把图标从「名称首字的占位块」换成真实图标：Windows 上从**运行中窗口**与 **exe 资源**
两路尽力抓取，统一规范化成 PNG 缓存到 `~/.app_catch/icons/`，抓不到就用内置默认图。

> 前置阅读：[docs/RUNTIME.md](RUNTIME.md)（会话生命周期）、[docs/GUI.md](GUI.md)（命令层 API）、[docs/CONFIG.md](CONFIG.md)（`icon` 字段）。

## 1. 一句话策略

```text
抓得到就用真图标，抓不到就用内置默认图 —— 抓不到不算失败。
```

这是**明确的产品决策**，不是妥协：Python 服务类应用（FastAPI/Flask 后端、CLI 工具、
纯推理服务）本来就没有窗口，从 exe 拿到的又往往是解释器的通用图标。
把它们判定为「失败」只会制造噪音；默认图反而是更诚实的呈现。

## 2. 优先级链

```text
IconTarget::capture
  │
  ├── 0. 缓存命中？ ── 是 ──> 直接返回 <home>/icons/<app_id>.png（不再抓取）
  │
  ├── 1. 用户自定义 icon（AppConfig.icon）
  │      └── .png 自己解码；.ico/.exe/.lnk 交给 Win32；无效则继续
  │
  ├── 2. 运行中窗口图标（需要会话在跑）
  │      ├── WM_GETICON + ICON_BIG      （应用自己 setWindowIcon 设置的真图标）
  │      ├── WM_GETICON + ICON_SMALL
  │      ├── GetClassLongPtrW(GCLP_HICON)    （窗口类注册时带的图标）
  │      └── GetClassLongPtrW(GCLP_HICONSM)
  │
  ├── 3. exe 资源图标（不需要进程在跑）
  │      ├── PrivateExtractIconsW(256) → (48) → (32)   （可直接指定像素尺寸）
  │      ├── ExtractIconExW                              （回退：某些 .ico）
  │      └── SHGetFileInfoW(SHGFI_LARGEICON)              （回退：Shell 大图标）
  │
  └── 4. 都没有 ──> None ──> 前端用内置默认图
```

### 2.1 为什么窗口图标优先于 exe 图标

需求基线原文：

> 应用启动后尽力抓取运行时图标（dev 模式下可执行文件常是默认图标，真图标在窗口上）

两种典型情况都印证这一点：

| 场景 | exe 图标 | 窗口图标 |
| --- | --- | --- |
| Python + PySide 开发态 | `python.exe` 的解释器图标（完全无关） | 应用自己设的图标 |
| Electron 开发态 | `electron.exe` 的默认图标 | 应用自己设的图标 |
| 打包后的独立应用 | 通常正确 | 通常也正确 |

因此只要**有运行中会话**，就先问窗口；窗口没有才回落到 exe。

### 2.2 三档之间是「失败即继续」

不是「失败即放弃」：用户填了一个已被删除的自定义图标路径，
不该连带让 exe 图标也抓不到。每一档失败都静默落到下一档。

## 3. PNG 规范

| 项目 | 规定 |
| --- | --- |
| 格式 | PNG（RGBA8，始终保留 alpha 通道） |
| 尺寸上限 | 最长边 ≤ **256px** |
| 小图处理 | **不放大**。32×32 的图标保持 32×32；硬拉到 256 只会糊 |
| 缩放算法 | 面积平均（box filter），颜色按 alpha 预乘后再平均，避免透明像素的黑色污染边缘 |
| 全透明判定 | **整张图 alpha 全 0 视为「没抓到」**，不落盘（避免把空图固化成缓存） |
| 行序 | DIB 用 `biHeight` 负值直接要「自顶向下」，省掉一次手工翻转 |

选 256 的理由：Windows 大图标（jumbo）与高清屏 2x 显示的常见档位；
单张 PNG 通常只有几 KB 到几十 KB，直接塞 data URL 也不会撑爆 IPC。

## 4. 缓存位置与失效规则

```text
<home>/icons/<app_id>.png        （<home> = $APP_CATCH_HOME 或 ~/.app_catch）
```

### 4.1 为什么按 app_id 命名，而不是内容 hash

| 方案 | 优点 | 缺点 |
| --- | --- | --- |
| **app_id（采用）** | 前端/命令层能直接拼出路径；用户删掉这个文件就等于强制重抓，行为可理解 | 图标变了要靠失效规则发现 |
| 内容 hash | 自动去重、天然不可变 | 前端无法预测路径；缓存目录会随图标变化堆积无法回收的旧文件 |

图标属于「每个应用一张、可随时重生」的资源，**可发现性**比去重更重要。

### 4.2 失效判定

```text
cache_state(cache, source):
  文件不存在 / 长度为 0           -> Missing  -> 重新抓取
  source 存在且 mtime 晚于缓存     -> Stale    -> 重新抓取（应用更新换了 exe）
  其余                            -> Ready    -> 直接复用，不问来源
```

判据是**两个文件的修改时间比较**，不是「缓存放了多久」：

- 应用升级（exe 被替换）→ exe 的 mtime 变新 → 缓存自动失效；
- 源文件消失（应用被卸载）→ 保留既有缓存（图标还能用）；
- 拿不到 mtime（文件系统不支持/权限不足）→ 按「不过期」处理。
  图标是可再生资源，宁可复用旧图，也不要在每次打开列表时反复跑 Shell API。

### 4.3 手动强制重抓

删掉 `~/.app_catch/icons/<app_id>.png` 即可。下次打开列表或启动应用时会重新抓取。

## 5. 兜底语义

```text
IconCapture::capture(&target) -> Option<PathBuf>
                                 ^^^^^^^^^^^^^^
                                 没有 Result，没有 Err 分支
```

| 约定 | 说明 |
| --- | --- |
| 绝不返回错误 | 接口签名里就没有 `Result`，抓不到只有 `None` 一种表达 |
| 绝不写错误日志 | 抓不到是预期内的正常情况，写日志只会制造噪音 |
| 绝不阻塞会话启动 | 图标抓取与 `start_app` 完全解耦：会话先起来，图标随后异步补 |
| 不缓存失败 | 失败时不写文件，下次调用仍会重试（窗口可能这会儿才建出来） |
| 前端静默降级 | `get_app_icon` 返回 `null` → 显示内置默认图；连默认图都取不到 → 首字块；**从不弹 toast** |

内置默认图是 `crates/launcher-core/assets/default_icon.png`（256×256，3169 字节），
通过 `include_bytes!` 编译进二进制——兜底图必须在**任何情况下**都能用，
不能依赖磁盘可读，也不能依赖当前工作目录。

## 6. 命令层与前端

### 6.1 为什么走 data URL 而不是 `asset:` 协议

| 方案 | 代价 |
| --- | --- |
| **data URL（采用）** | 图标几十 KB → base64 后约 +33%，仍远小于一次 IPC 的合理开销；不需要改 `capabilities/default.json` |
| `asset:` 协议 | 需要在 `tauri.conf.json` 配 `assetProtocol.scope`（还要开启 `assetProtocol.enable`）、在 capabilities 加 `core:asset:default`；任一处漏配**只在运行时**暴露成「图裂」 |

图标是**几十 KB 级的装饰性资源**，不是视频/大图；为它引入一套协议 + scope 配置，
换来的是「配错了才发现」的故障面。走 data URL 后：

- `capabilities/default.json` **保持只有 `core:default`**，无需新增权限；
- 前端只需要 `<img src={dataUrl}>`，没有额外加载状态机。

### 6.2 前端三层缓存

| 层 | 位置 | 作用 |
| --- | --- | --- |
| 磁盘缓存 | `<home>/icons/<app_id>.png` | 跨进程复用 |
| 会话内缓存 | `src/lib/iconModel.ts` 的 `resolved` Map | 列表滚动/切选中不重复 IPC |
| 并发合并 | 同文件的 `pending` Map | 列表项同时挂载时只发一次请求 |

### 6.3 运行时图标的抓取时机

```text
用户点「启动」
  └── start_app 返回（会话已起）
        └── 等待 1.5 秒（给窗口创建留时间）
              └── invalidateIcon(app_id) + 触发重抓
                    └── 这次请求带上 pid，走「窗口图标」分支
```

1.5 秒是折中：太早窗口还没出来（拿不到），太晚用户已经看过列表了。
抓不到也不影响任何功能——前端一直显示默认图。

## 7. 已知限制

| 情况 | 表现 | 原因 |
| --- | --- | --- |
| Python 服务类/CLI 应用 | 默认图 | 没有窗口；exe 是解释器，图标无关 |
| 启动前（会话未运行） | exe 资源图标或默认图 | 窗口还不存在，拿不到运行时图标 |
| Store 应用（notepad/calc 的转发 stub） | 可能拿到系统通用图标 | 真实进程与窗口属于另一个包进程 |
| 无图标资源的 exe | 系统通用图标 | Shell 会给一个默认的「可执行文件」图标，比空好 |
| 全透明图标 | 默认图 | 空图没有信息量，比默认图更差 |
| 自定义图标路径失效 | 回落到 exe/窗口图标 | 刻意「失败即继续」而非「失败即放弃」 |
| macOS | 默认图 | M4 才实现（`FallbackIconCapture` 恒返回 `None`） |
| 图标尺寸小于 256 | 保持原尺寸 | 刻意的「不放大」策略，避免糊图 |

## 8. 依赖选择：`png` 而不是 `image`

```toml
# Cargo.toml [workspace.dependencies]
png = "0.18"
```

| 方案 | 依赖树 | 说明 |
| --- | --- | --- |
| **`png`（采用）** | png + flate2 + miniz_oxide + crc32fast + fdeflate + bitflags | 只做 PNG 编解码，正好覆盖「RGBA → PNG」与「PNG → RGBA」两条路径 |
| `image` | 上述全部 **加上** jpeg/gif/webp/tiff/bmp/ico 等解码器 | 我们一张 JPEG 都不需要；多出来的依赖要参与编译、要跟随升级、要审计 CVE |

本 crate 的图标路径只需要两件事：**把 RGBA 编成 PNG**、**把用户给的 PNG 读成 RGBA**。
`png` 是 `image` 的底层依赖（image 自己也是用 png 做 PNG 编解码），
直接用它既没有功能损失，又把依赖面与编译时间压到最小。

> 实测：`cargo check -p launcher-core` 因引入 `png` 只新增 1 个直接依赖
> （`Cargo.lock` 增加 1 行）；`png` 及其传递依赖在增量编译下约 1~2 秒。

## 9. 资源释放（Windows）

图标抓取要接触四类系统句柄，全部由 RAII 包装，Drop 时释放：

| 句柄 | 来源 | 释放方式 |
| --- | --- | --- |
| `HICON`（自己的） | `PrivateExtractIconsW` / `ExtractIconExW` / `SHGetFileInfoW` / `CopyImage` | `DestroyIcon` |
| `HICON`（**外来**） | `WM_GETICON` / `GetClassLongPtrW` | **不销毁**：先 `CopyImage` 复制一份，只销毁复制品 |
| `HBITMAP` | `GetIconInfo` | `DeleteObject` |
| `HDC` | `CreateCompatibleDC` / `GetDC` | `DeleteDC` / `ReleaseDC` |

特别注意**外来 HICON**：它属于目标进程，销毁它会破坏别人的资源，
而在转换期间它也可能失效。统一先 `CopyImage` 是唯一安全的做法。

### 9.1 两个实测踩出来的坑

这两条都是**测出来才发现的**，写在这里避免后人重蹈：

#### 正斜杠路径会让 SHGetFileInfoW 直接失败

`SHGetFileInfoW` 对 `C:/dir/app.exe` 这种正斜杠路径**返回 0**，拿不到任何 hIcon；
换成反斜杠就正常。而本项目的文档与配置示例里到处都是正斜杠
（`cwd = "E:/repo/GPT-SoVITS"`、`icon = "E:/repo/GPT-SoVITS/icon.png"`）。

`PrivateExtractIconsW` 对正斜杠宽容，所以这个 bug 只在「exe 档位失败、
回落到 Shell 档位」时才暴露——**正好是 `.lnk` 的场景**。

修法：`to_wide()` 在转宽字符时把正斜杠换成反斜杠。
回归测试：`forward_slash_path_works_like_backslash`、`forward_slash_lnk_resolves_through_shell`。

#### Shell 图标 API 在高并发下会静默失败

`SHGetFileInfoW` 解析 `.lnk` 时会走 Shell 的链接解析链路，并发调用在
**高线程数下稳定失败**。本机实测：

| 并发线程数 | 结果 |
| --- | --- |
| 1 / 2 / 4 / 6 | 全部成功 |
| 8 / 12 / 16 | **必现一次失败** |

失败是**静默的**（返回 0 / 空 hIcon），调用方分不清「这文件真没图标」
与「这次撞上了并发缺陷」。

最阴险的地方：命令层正是把抓取放在 `spawn_blocking` 的线程池上跑的，
真实使用中并发度不受我们控制。

修法：`file_icon` 内部用进程级互斥锁（`ICON_API_LOCK`）串行化。
代价可以忽略——图标每个应用只抓一次，之后都走磁盘缓存，
需要抓取的次数是「应用个数」量级，不是请求量级。
回归测试：`shell_icon_api_is_serialized_under_concurrency`（16 线程 × 10 轮 = 160 次，去掉锁会稳定复现失败）。

> 这两条也解释了为什么「图标抓不到」必须走兜底而不是报错：
> 真实世界里的失败原因太多，把每一种都变成错误提示只会淹没用户。

## 10. 测试覆盖

| 层 | 命令 | 覆盖 |
| --- | --- | --- |
| 单元（跨平台） | `cargo test -p launcher-core --lib` | PNG 编解码往返、魔数与尺寸、不放大策略、box filter、base64 向量、缓存三态、mtime 失效、临时文件不残留 |
| 单元（Windows） | 同上 | `resolve_executable` 解析、无效路径不猜测、PNG 自解码、全透明拒绝 |
| 集成（真实资源） | `cargo test -p launcher-core --test icon_capture` | **真实 notepad.exe 图标**、**真实 WinForms 窗口图标**、端到端缓存写入、二次调用 mtime 不变、无效/空目标兜底、自定义图标优先级 |
| 前端 | `pnpm test` | 会话内缓存、并发合并、失败静默降级、失效重取、默认图只取一次 |

集成测试用「运行时探测 + 打印跳过原因」代替 `#[ignore]`：
无交互桌面的 Session 0/容器里窗口建不出来，那是环境限制不是代码缺陷。
判据是 `SESSIONNAME`（与 `tests/status_probe.rs` 一致）。

## 11. 实测样本

| 应用 | 来源 | 尺寸 | PNG 大小 |
| --- | --- | --- | --- |
| `notepad.exe`（exe 资源） | `PrivateExtractIconsW(256)` | 256×256 | 61274 字节 |
| PowerShell + WinForms 窗口 | `GetClassLongPtrW(GCLP_HICON)` | 32×32 | 约 1 KB |
| 内置默认图 | 编译进二进制 | 256×256 | 3169 字节 |
