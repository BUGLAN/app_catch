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
  ├── 3. 项目图标（不需要进程在跑；需要 AppConfig.cwd）—— platform/project_icon.rs，全平台共用
  │      ├── electron-builder 配置的 icon（yml / json / package.json "build"，本平台段 mac/win/linux 优先）
  │      └── 常见位置：build/icon、resources/icon、assets/icon、src-tauri/icons/icon、public/icon-512 …
  │
  ├── 4. exe 资源图标（不需要进程在跑）
  │      ├── PrivateExtractIconsW(256) → (48) → (32)   （可直接指定像素尺寸）
  │      ├── ExtractIconExW                              （回退：某些 .ico）
  │      └── SHGetFileInfoW(SHGFI_LARGEICON)              （回退：Shell 大图标）
  │
  └── 5. 都没有 ──> None ──> 前端用内置默认图
```

档位（写在 `<app_id>.tier` 里，只升不降）：`custom > window > project > exe`。

### 2.0 macOS（`platform/icon_macos.rs`）

macOS 上图标属于 `.app` 包：`Contents/Info.plist` 的 `CFBundleIconFile` → `Resources/*.icns` → 取内嵌 PNG。

| 档位 | 来源 |
| --- | --- |
| custom | 配置里的 `.png` / `.icns` / `.ico`，或直接指向一个 `.app` |
| window | 运行中进程树里第一个位于 `.app` 内的进程（真正的 GUI 应用） |
| project | 同上「项目图标」 |
| exe | `node_modules` 里的 Electron / NW.js 包（开发态，排在项目图标之后），或启动命令本身位于 `.app` 内 |

开发态的 Electron 应用（如 `pnpm dev:electron`）：运行中的进程是 `node_modules/electron/dist/Electron.app`，
只能拿到 Electron 的原子图标；应用用 `app.dock.setIcon()` 设的图标其它进程读不到。
所以这类应用靠**项目图标**档位显示正确图标——不运行也能显示。

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

内置默认图是 `crates/launcher-core/assets/default_icon.png`（256×256，12034 字节），
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
| 内置默认图 | 编译进二进制 | 256×256 | 12034 字节 |

---

# 附：图标抓取失效的根因分析（2026-10-25）

## 现象

三个 Electron 应用（ATT 实时语音助手 / LunaWrite 2 / Opennote 笔记）显示**完全相同**的
「窗口+齿轮」灰白图标，而不是各自应用的真实图标。

## 实证过程

| 步骤 | 方法 | 结果 |
| --- | --- | --- |
| 1 | `md5sum ~/.app_catch/icons/*.png` | 三个文件 **md5 完全相同**（`05424636…`），均 64×64 / 1707 字节 |
| 2 | PowerShell `Icon.ExtractAssociatedIcon(pnpm.cmd)` | 得到**同一张**「窗口+齿轮」图 |
| 3 | 对运行中的 Electron 窗口发 `WM_GETICON ICON_BIG` | 取到 **512×512 金色「錄」字** —— 真图标存在且可访问 |

## 根因（两处，叠加导致）

### 原因 A：`resolve_executable` 返回的是 `.cmd` 批处理文件

```text
app.command = "pnpm.cmd"  →  resolve_executable()  →  C:\Users\...\npm\pnpm.cmd
```

`icon.rs` 的 `resolve_executable` 逻辑是「命令名 → 在 PATH 里找同名文件」，
`pnpm.cmd` 带扩展名，于是直接命中那个 **332 字节的 cmd 脚本**。
Windows 对 `.cmd`/`.bat` 没有图标资源，Shell 一律返回**通用批处理图标**——就是那个「窗口+齿轮」。

对 Electron 开发态来说，真正的图标在**窗口**上（应用设置的 BrowserWindow 图标），
而 `.cmd` 连 exe 都不是。这一档抓到的必然是错的。

### 原因 B：抓完立刻写缓存，且此后永不重抓

`resolve_and_cache()` 的顺序是「缓存命中 → 抓 → 写盘」，而 `cache_state()` 的
失效条件只有一条：**源文件 mtime 晚于缓存**。

问题在于：`pnpm.cmd` 的 mtime 是 **2026-08-13**，而缓存在 **2026-10-25 15:06** 写入。
源文件永远比缓存旧 → 永远 `Ready` → **永远是这张错图**。

时间线也印证了这一点：

| 时间 | 事件 |
| --- | --- |
| 15:06:00.95 | 三个图标被写入（此时 ATT **还没启动**，15:06:11 才起） |
| 15:06:11 | att 会话开始 |
| 15:07:17 | opennote 会话开始 |

也就是说：**第一次抓取发生在窗口出现之前**，抓到的是 `.cmd` 的通用图标；
等窗口起来之后，缓存已经命中，`WM_GETICON` 那一档再也没机会执行。

### 为什么「运行时刷新」没救回来

前端 `App.tsx` 在启动后确实做了：

```ts
invalidateIcon(app.id);                                    // 清前端内存缓存
window.setTimeout(() => setIconRefresh(v => v + 1), 1500); // 1.5s 后重取
```

但 `invalidateIcon` 只清**前端内存**缓存；命令层 `get_app_icon` 仍然先走
`resolve_and_cache`，磁盘缓存命中就直接返回——**根本不会走到窗口档位**。

于是链路变成：
前端清缓存 → 重新 invoke → 后端磁盘缓存命中 → 返回同一张 `.cmd` 图标。

## 附带因素（解释「为什么有时对、有时错」）

窗口档位依赖 `collect_tree_pids(root_pid)` 枚举子孙进程。实测这条链路是通的
（Electron 的窗口在 node → electron 子进程里，pid 树能覆盖到），
但只要 **pid 树枚举失败**（例如 vite 热重启换掉进程、或 Job Object 返回不全），
就会静默回落到 exe 档位 → `.cmd` 图标。

## 修复方向

| # | 改动点 | 说明 |
| --- | --- | --- |
| 1 | `resolve_executable` 过滤脚本类扩展名 | `.cmd/.bat/.ps1` 不是图标来源，应视为「没有 exe」→ 直接落到窗口档位或兜底图 |
| 2 | 缓存记录「来源档位」 | 记下这张图是「窗口 / exe / 脚本 / 自定义」抓的，低档位结果可被窗口档位结果替换，反之不可 |
| 3 | 窗口档位成功后允许覆盖旧缓存 | 若当前缓存来自 exe/脚本档位而本次窗口档位成功，则重写缓存 |
| 4 | 前端 `iconRefresh` 支持 force | 让「启动后刷新」真的绕过磁盘缓存一次，而不是只看 mtime |

## 注意：这属于后端改动

上面的修复全部落在 `crates/launcher-core/src/platform/icon*.rs`（+ 命令层传参），
属于 **Rust 侧逻辑**。按本次任务「不改后端逻辑」的约束，这里只给出定位与方案，未直接改动。
前端 `iconModel` 的缓存语义本身是正确的，不需要改。

---

# 附：图标抓取失效的修复（2026-10-25 落地）

上一节记录了根因；本节记录**已落地的修复**与实测结果。

## 改动清单

| # | 文件 | 改动 |
| --- | --- | --- |
| 1 | `crates/launcher-core/src/platform/icon.rs` | 新增 `is_script()` + `SCRIPT_EXTENSIONS`；`resolve_executable` 在三条出口上统一过滤脚本 |
| 2 | 同上 | 新增 `IconTier`（`Exe < Window < Custom`）与 `CapturedIcon`；`IconSource::produce` 返回档位 |
| 3 | 同上 | 缓存旁新增 sidecar `<app_id>.tier`（`tier_path` / `read_tier` / `write_tier`） |
| 4 | 同上 | `resolve_and_cache` 改为「档位 + 有无 pid」判定，**只升不降**；旧版无档位缓存可被升级 |
| 5 | 同上 | 新增 `resolve_and_cache_forced`：忽略缓存强制重抓（仍禁止降级） |
| 6 | 同上 | 无档位 + 无 exe（脚本来源）的旧缓存**自动删除**，回落到内置默认图 |
| 7 | `crates/launcher-core/src/platform/mod.rs` | `IconCapture` 新增 `recapture()`（默认退化为 `capture`） |
| 8 | `crates/launcher-core/src/platform/icon_windows.rs` | `produce` 返回档位；exe 档位跳过脚本；实现 `recapture` |
| 9 | `src-tauri/src/commands.rs` | `get_app_icon` 新增可选参数 `force`（默认 `false`，行为向后兼容） |
| 10 | `src/lib/api.ts` | `getAppIcon(appId, force = false)` |
| 11 | `src/lib/iconModel.ts` | `loadIcon(appId, force = false)`：force 同时穿透内存缓存 |
| 12 | `src/components/AppIcon.tsx` | 新增 `forceRefresh`；仅在 `refreshKey` **真的递增**那次强制穿透 |
| 13 | `src/App.tsx` · `AppList` · `AppDetail` | 会话启动后的刷新带上 `force`，其余刷新不带 |

## 关键设计：为什么是「档位 + 只升不降」

原来的缓存键是「文件在不在 + 源文件 mtime」。这个键**没有表达可信度**，于是：

```text
应用未运行 -> 抓到 .cmd 通用图标（Exe 档）-> 写缓存
应用运行了 -> 缓存命中 -> 直接返回错图（永不升级）
```

现在的键是「**档位 + 有没有窗口可查**」：

| 已有缓存档位 | 有 pid（能查窗口） | 行为 |
| --- | --- | --- |
| `Window` / `Custom` | 任意 | 直接复用，不再抓 |
| `Exe` / 未知 | 否 | 复用；若同时解析不出 exe（脚本来源）则**删除**并回落默认图 |
| `Exe` / 未知 | 是 | 重抓一次，抓到更高档位才覆盖 |

「只升不降」保证窗口偶尔取不到时，真图标不会被解释器图标顶掉。

## 实测结果（用户真实环境）

你的 `~/.app_catch/apps/` 里三个应用都是 `command = 'pnpm.cmd'`：

```text
修复前：resolve_executable("pnpm.cmd") -> C:\Users\...\npm\pnpm.cmd
        -> 三个应用都拿到同一张 64×64 「窗口+齿轮」通用批处理图标

修复后：resolve_executable("pnpm.cmd") -> None（脚本不是图标来源）
        -> 应用运行中时走窗口档位，拿到 256×256 真图标
```

| 应用 | 修复前 | 修复后 |
| --- | --- | --- |
| ATT 实时语音助手（运行中） | 64×64 齿轮 | **256×256 金色「錄」字，档位 `Window`** |
| LunaWrite 2（未运行） | 64×64 齿轮 | 错图被清理 → 内置默认图（启动后会拿到真图标） |
| Opennote 笔记（未运行） | 64×64 齿轮 | 同上 |

curl 之外的真实校验：

```text
$ ls ~/.app_catch/icons/
att.png   att.tier        # att.tier 内容: window
```

## 用户需要做什么

**什么都不用做。** 旧缓存会被自动识别并处理：

- 应用运行中 → 下一次 `get_app_icon` 直接把真图标换进去；
- 应用未运行 → 错图被删除，先显示内置默认图，等启动后自动换成真图标。

如果想立刻看到效果：重启 app_catch（让新二进制生效），然后把对应应用启动一次。

## 测试覆盖

新增 12 条 Rust 用例与 2 条前端用例，覆盖：

- 脚本扩展名识别、存在的 `.cmd` 解析为 `None`、PATH 命中脚本也返回 `None`
- 低档位缓存被窗口档位覆盖（复现原 bug 的时序）
- 窗口档位已缓存时不重复抓取（性能不回退）
- 无 pid 时 exe 档位缓存直接复用
- 只升不降：窗口图标不被 exe 档位覆盖
- 旧版无 sidecar 缓存可升级
- `forced` 强制重抓，且不会把 `Custom` 降级
- 档位字符串往返（持久化契约）
- 脚本来源的旧缓存被丢弃；有档位的正常缓存不受影响
