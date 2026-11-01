# 图标：抓取策略、缓存规则与兜底语义

Windows 从运行中窗口与 exe 资源两路尽力抓取图标，macOS 走 `.app` 包与项目图标，统一规范化成 PNG 缓存到 `<home>/icons/`。

```text
抓得到就用真图标，抓不到就用内置默认图 —— 抓不到不算失败。
```

这是产品决策：Python 服务类应用（FastAPI/Flask 后端、CLI 工具）本来就没有窗口，从 exe 拿到的往往是解释器的通用图标，把它们判定为失败只会制造噪音。

## 优先级链

```text
IconTarget::capture
  ├── 0. 缓存命中（且档位不低于本次可抓档位）──> 直接返回 <home>/icons/<app_id>.png
  ├── 1. 用户自定义 icon（AppConfig.icon）
  ├── 2. 运行中窗口图标（需要会话在跑）
  │      ├── WM_GETICON + ICON_BIG / ICON_SMALL
  │      └── GetClassLongPtrW(GCLP_HICON / GCLP_HICONSM)
  ├── 3. 项目图标（不需要进程在跑；需要 AppConfig.cwd，全平台共用）
  │      ├── electron-builder 配置中的 icon（yml / json / package.json "build"）
  │      └── build/icon、resources/icon、assets/icon、src-tauri/icons/icon、public/icon-512 …
  ├── 4. exe 资源图标（脚本类扩展名不算图标来源）
  │      ├── PrivateExtractIconsW(256) → (48) → (32)
  │      ├── ExtractIconExW
  │      └── SHGetFileInfoW(SHGFI_LARGEICON)
  └── 5. 都没有 ──> None ──> 前端用内置默认图
```

档位记录在 `<app_id>.tier`，**只升不降**：`custom > window > project > exe`。低档位缓存可以被更高档位覆盖，反之不行——窗口偶尔取不到时，真图标不会被解释器图标顶掉。

每一档失败都静默落到下一档：用户填了已被删除的自定义图标路径，不应连带让 exe 图标也抓不到。

## macOS

图标属于 `.app` 包：`Contents/Info.plist` 的 `CFBundleIconFile` → `Resources/*.icns` → 取内嵌 PNG。

| 档位 | 来源 |
| --- | --- |
| custom | 配置里的 `.png` / `.icns` / `.ico`，或直接指向一个 `.app` |
| window | 运行中进程树里第一个位于 `.app` 内的进程 |
| project | 同「项目图标」 |
| exe | `node_modules` 里的 Electron / NW.js 包，或启动命令本身位于 `.app` 内 |

开发态 Electron 进程是 `node_modules/electron/dist/Electron.app`，只能拿到原子图标，正确图标靠项目图标档位。

## PNG 规范

| 项目 | 规定 |
| --- | --- |
| 格式 | PNG（RGBA8，始终保留 alpha 通道） |
| 尺寸上限 | 最长边 ≤ **256px** |
| 小图处理 | **不放大**，32×32 保持 32×32 |
| 缩放算法 | 面积平均（box filter），颜色按 alpha 预乘后再平均 |
| 全透明判定 | 整张图 alpha 全 0 视为「没抓到」，不落盘 |
| 行序 | DIB 用 `biHeight` 负值直接取自上而下 |

## 缓存与失效

```text
<home>/icons/<app_id>.png     （<home> = $APP_CATCH_HOME 或 ~/.app_catch）
<home>/icons/<app_id>.tier    （来源档位 sidecar）
```

按 `app_id` 命名而不是内容 hash：前端与命令层能直接拼出路径，用户删掉文件就等于强制重抓，行为可理解。

```text
cache_state(cache, source, tier, has_pid):
  文件不存在 / 长度为 0            -> Missing  -> 重新抓取
  source 存在且 mtime 晚于缓存      -> Stale    -> 重新抓取（应用更新换了 exe）
  有 pid 且档位低于 window         -> Upgrade  -> 重抓一次，抓到更高档位才覆盖
  其余                             -> Ready    -> 直接复用
```

- 源文件消失时保留既有缓存（图标还能用）；拿不到 mtime 按「不过期」处理；
- 手动强制重抓：删掉 `<app_id>.png` 与 `<app_id>.tier`，或让命令层带 `force`（仍禁止降级）。

## 兜底语义

```text
IconCapture::capture(&target) -> Option<PathBuf>
```

| 约定 | 说明 |
| --- | --- |
| 绝不返回错误 | 签名里没有 `Result`，抓不到只有 `None` |
| 绝不写错误日志 | 抓不到是预期内的正常情况 |
| 绝不阻塞会话启动 | 抓取与 `start_app` 解耦，会话先起来，图标随后补 |
| 不缓存失败 | 失败不写文件，下次仍会重试 |
| 前端静默降级 | `null` → 内置默认图；默认图也取不到 → 名称首字块，不弹 toast |

内置默认图是 `crates/launcher-core/assets/default_icon.png`，通过 `include_bytes!` 编译进二进制，不依赖磁盘或工作目录。

## 命令层与前端

图标以 **data URL** 返回，不引入 `asset:` 协议（那需要在 `tauri.conf.json` 配 scope、在 capabilities 加权限，漏配只在运行时暴露成「图裂」）。图标只有几十 KB，base64 开销可忽略，capabilities 保持 `core:default`。

| 缓存层 | 位置 | 作用 |
| --- | --- | --- |
| 磁盘 | `<home>/icons/<app_id>.png` | 跨进程复用 |
| 会话内 | `src/lib/iconModel.ts` 的 `resolved` Map | 列表滚动 / 切选中不重复 IPC |
| 并发合并 | 同文件的 `pending` Map | 同时挂载只发一次请求 |

启动应用后等待 1.5 秒再带 `force` 重抓一次，给窗口创建留时间；抓不到不影响任何功能。

## 已知限制

| 情况 | 表现 | 原因 |
| --- | --- | --- |
| Python 服务类 / CLI 应用 | 默认图 | 没有窗口，exe 是解释器 |
| 启动前（会话未运行） | 项目图标、exe 图标或默认图 | 窗口还不存在 |
| Store 应用（notepad/calc 的转发 stub） | 可能拿到系统通用图标 | 真实进程与窗口属于另一个包进程 |
| 无图标资源的 exe | 系统通用图标 | Shell 的默认「可执行文件」图标 |
| 全透明图标 | 默认图 | 空图没有信息量 |
| 自定义图标路径失效 | 回落到后续档位 | 刻意「失败即继续」 |
| 图标尺寸小于 256 | 保持原尺寸 | 刻意「不放大」 |

## 依赖选择：`png` 而不是 `image`

```toml
# Cargo.toml [workspace.dependencies]
png = "0.18"
```

图标路径只做两件事：RGBA 编码成 PNG、用户给的 PNG 读成 RGBA。`png` 是 `image` 的底层依赖，直接用它没有功能损失，却省掉 jpeg/gif/webp/tiff/bmp/ico 等解码器的编译与 CVE 跟随成本。

## Windows 资源释放

| 句柄 | 来源 | 释放方式 |
| --- | --- | --- |
| `HICON`（自己的） | `PrivateExtractIconsW` / `ExtractIconExW` / `SHGetFileInfoW` / `CopyImage` | `DestroyIcon` |
| `HICON`（外来的） | `WM_GETICON` / `GetClassLongPtrW` | **不销毁**：先 `CopyImage`，只销毁复制品 |
| `HBITMAP` | `GetIconInfo` | `DeleteObject` |
| `HDC` | `CreateCompatibleDC` / `GetDC` | `DeleteDC` / `ReleaseDC` |

两个实测踩出来的坑：

- **正斜杠路径会让 `SHGetFileInfoW` 返回 0**：转宽字符时统一把 `/` 换成 `\`，否则 `.lnk` 的 Shell 档位永远失败。
- **Shell 图标 API 在高并发下静默失败**：8 线程以上解析 `.lnk` 必现失败，因此 `file_icon` 用进程级互斥锁串行化。抓取次数是「应用个数」量级，代价可忽略。
