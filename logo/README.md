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
# 2. 生成 Tauri 全套（ico / icns / Windows Square / android / ios）
npx tauri icon logo/icon/app-icon-1024.png
```

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
（原来是 Tauri 默认蓝 `#5b8def`，现在用 `--surface-elevated` + `--accent`）。

## 颜色

| token | 深色主题 | 浅色主题 |
| --- | --- | --- |
| `--accent`（方块底） | `#5b5ce0` | `#4b56d2` |
| `--text-on-accent`（杠） | `#ffffff` | `#ffffff` |

标题栏徽标走 token，所以深浅切换自动跟随；
应用图标本身是静态 PNG，固定用深色主题那组值（任务栏以深色为主）。
