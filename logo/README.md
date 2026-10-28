# app_catch 应用图标

## 图形

圆角方块 + 三条递减的横杠。三条杠是"日志行"，递减是"越往下越短"的日志节奏；
方块是"容器"，把外部进程的输出收进来。

字标 `app_catch` 不与图形组合——桌面启动器的图标位置只有 16–256px，
组合标在那三处各占一半就会两边都糊。需要横排时单独排字。

## 文件对应关系

| 用途 | 文件 | 说明 |
| --- | --- | --- |
| 矢量母版 | `logo/icon/app-icon-master.svg` | 1024×1024，改图标从这里改 |
| 小尺寸母版 | `logo/icon/app-icon-small.svg` | 两杠简化版，给 16px 用 |
| favicon | `logo/icon/favicon.svg` | 32×32 视口，纯几何无文字 |
| 兜底图源 | `logo/icon/fallback.svg` | **不是品牌标**，见下 |

改完 `app-icon-master.svg` 后重新生成全套：

```bash
# 1. SVG -> 1024 PNG（无 cairosvg 时用 headless chrome）
chrome --headless=new --screenshot=logo/icon/app-icon-1024.png \
       --window-size=1024,1024 --default-background-color=00000000 \
       "file:///<abs>/logo/icon/app-icon-master.svg"
# 2. 生成 Tauri 全套（icns / Windows Square / android / ios）
npx tauri icon logo/icon/app-icon-1024.png
# 3. **重新生成 Windows 的 icon.ico**（见下，第 2 步产出的 ico 不能用）
node scripts/build-app-icon.mjs
# 4. macOS 的 icns 单独用"macOS 图标网格"版覆盖：
#    1024 画布里主体 824px、四周留白 + 投影；全幅图在程序坞里会比别的应用大一圈。
npx tauri icon <macOS 网格版 1024 PNG> -o /tmp/mac-icons && cp /tmp/mac-icons/icon.icns src-tauri/icons/icon.icns
```

顺序不能反：第 2 步会把 `icon.ico` 覆盖成"一张 1024 缩小到底"的版本，第 3 步必须在其后跑。

PNG 母版（全幅版与 macOS 网格版）可以用 PIL 直接按 SVG 的几何渲染，不依赖 Chrome。

### Windows 的 icon.ico 为什么单独生成

`npx tauri icon` 只吃一张 1024 PNG，再把它重采样到 16/24/32/48/64/256，有两个问题：

| 问题 | 后果 | `scripts/build-app-icon.mjs` 的做法 |
| --- | --- | --- |
| 16px 也走三杠母版 | 三条横杠在这个尺寸糊成一片 | 16/20 改用两杠小母版，其余用三杠母版 |
| 每帧都是 PNG 压缩帧 | Windows 只对 256px 承诺支持 PNG 帧，小尺寸在部分 shell 路径会被忽略、回退成"把 256 缩下来"，表现为**图标发虚** | 16–128 写 DIB(BMP) 帧，只有 256 用 PNG 帧 |
| 只有 16/24/32/48/64/256 六档 | 125%（40px）、150%（96px）等 DPI 下只能缩放，边缘发软 | 补齐 16/20/24/32/40/48/64/96/128/256 十档，每档按目标尺寸从矢量母版单独栅格化 |
| 直接缩放母版，直边落在半像素上 | 每条边被抗锯齿抹成两行灰，32px 的任务栏图标看着就糊 | 几何吸附到整数像素：底板留白、横杠高度/间距/起点全部取整，横杠按「上一条下沿 + 间距」顺排，16px 也不会粘成一坨 |

脚本只依赖本机 Chrome / Edge（无头截图，可用 `CHROME_PATH` 指定），不引入 npm 依赖；
`scripts/build-app-icon.test.mjs` 会守住"小尺寸必须是 DIB 帧""16/20 用两杠母版"这两条。

### 换了图标但 Windows 上还是旧图？

三件事按顺序检查：

1. **exe 里的资源有没有真的换掉**：`cargo build` 后看 `target/debug/build/app-catch-*/out/resource.lib` 的 mtime。
   `src-tauri/build.rs` 里为 `icons/*` 显式声明了 `rerun-if-changed`——tauri-build 自己只管
   `tauri.conf.json` 与 `capabilities/`，少了这几行，换 ico 后构建脚本不重跑，exe 会一直嵌着旧图标。
2. **图标缓存**：资源管理器按路径缓存图标，重编后仍可能显示旧图。`ie4uinit.exe -show` 可刷新
   （必要时再删 `%LOCALAPPDATA%\IconCache.db` 并重启资源管理器）。
3. **已安装的副本**：NSIS 安装目录里的 exe / 开始菜单快捷方式指向的是旧构建，需要重新安装。

## 三处会同时被看到的地方

用户会同时看到窗口图标、任务栏缩略图和标题栏徽标，三者必须是同一图形：

| 位置 | 实现 |
| --- | --- |
| 窗口 / 任务栏 / 安装包 | `src-tauri/icons/`（由 `npx tauri icon` 生成） |
| 标题栏 22px 徽标 | `<BrandMark />`（`src/components/ui/Icons.tsx`） |
| 浏览器标签页 | `index.html` 内联 data-URI favicon |

三者的几何都来自同一个 32 单位网格（`0 0 32 32`），所以比例一致。
改其中任何一个，另外两个要跟着改。

## 兜底图为什么不是品牌标

`crates/launcher-core/assets/default_icon.png` 出现在"这个应用抓不到图标"的行上。
如果它画 app_catch 自己的 logo，那一行会被读成"这就是 app_catch"，
而实际含义是"这里应该有个图标，但没抓到"。

所以它保留四宫格的通用占位语义，只是配色对齐 `tokens.css`
（石墨灰 `#3a3a3c` 底 + 浅灰 `#c7c7cc` 四宫格，刻意不用品牌的白杠，避免被误读成 app_catch 自己）。

## 颜色

石墨灰 + 白杠，与界面的中性灰阶一致（原靛紫 `#5b5ce0` 已弃用：与系统蓝的交互色冲突，观感也偏"AI 产品"）。

| 位置 | 底 | 杠 |
| --- | --- | --- |
| 应用图标（`app-icon-master.svg` / 全套 PNG） | 竖向渐变 `#4a4a4e → #222224` + 顶部 1px 高光 | `#f5f5f7` |
| 标题栏徽标 `<BrandMark />` | `--brand-bg`（= `--text-primary`，随主题反相） | `--brand-fg` |
| 浏览器 favicon | `#3a3a3c`（16px 下渐变不可见，用实色） | `#ffffff` |

标题栏徽标走 token，深浅切换自动反相（深色主题下是白底黑杠）；
应用图标本身是静态 PNG，固定用石墨灰那组值。
