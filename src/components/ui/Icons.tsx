/**
 * 内联 SVG 图标集。
 *
 * 为什么不用 emoji 或图标字体：emoji 在不同系统渲染差异极大（Windows 上是彩色字形），
 * 图标字体则要额外加载字体文件、还会被 font-size 影响比例。内联 SVG 只吃 currentColor，
 * 尺寸与描边完全可控，也不增加网络请求。
 *
 * 所有图标默认 aria-hidden：它们始终与文字标签（或 aria-label）成对出现，
 * 读屏不需要重复朗读。
 *
 * 描边统一 1.5；窗口控制按钮用 1.2，因为它们是 10px 的小字形，
 * 1.5 在 1x 屏上会糊成一团。
 */

type IconProps = {
  /** 像素尺寸，默认 16。 */
  size?: number;
  className?: string;
};

function svgProps(size: number, className?: string) {
  return {
    width: size,
    height: size,
    viewBox: "0 0 16 16",
    fill: "none",
    stroke: "currentColor",
    strokeWidth: 1.5,
    strokeLinecap: "round" as const,
    strokeLinejoin: "round" as const,
    className: className ? "icon " + className : "icon",
    "aria-hidden": true,
    focusable: "false" as const,
  };
}

/* ---------- 通用 ---------- */

export function IconPlus({ size = 16, className }: IconProps) {
  return (
    <svg {...svgProps(size, className)}>
      <path d="M8 3.5v9M3.5 8h9" />
    </svg>
  );
}

export function IconPlay({ size = 16, className }: IconProps) {
  return (
    <svg {...svgProps(size, className)} fill="currentColor" stroke="none">
      <path d="M5.5 3.6a.6.6 0 0 1 .9-.52l5.1 3.4a.6.6 0 0 1 0 1.02l-5.1 3.4a.6.6 0 0 1-.9-.51V3.6Z" />
    </svg>
  );
}

export function IconStop({ size = 16, className }: IconProps) {
  return (
    <svg {...svgProps(size, className)} fill="currentColor" stroke="none">
      <rect x="4.5" y="4.5" width="7" height="7" rx="1.2" />
    </svg>
  );
}

export function IconRestart({ size = 16, className }: IconProps) {
  return (
    <svg {...svgProps(size, className)}>
      <path d="M13 8a5 5 0 1 1-1.6-3.7" />
      <path d="M13 2.6V5.4h-2.8" />
    </svg>
  );
}

export function IconEdit({ size = 16, className }: IconProps) {
  return (
    <svg {...svgProps(size, className)}>
      <path d="M10.4 2.9a1.3 1.3 0 0 1 1.9 0l.8.8a1.3 1.3 0 0 1 0 1.9L6.2 12.5l-3 .7.7-3 6.5-7.3Z" />
    </svg>
  );
}

export function IconTrash({ size = 16, className }: IconProps) {
  return (
    <svg {...svgProps(size, className)}>
      <path d="M2.8 4.6h10.4M6.2 4.6V3.3a.8.8 0 0 1 .8-.8h2a.8.8 0 0 1 .8.8v1.3" />
      <path d="M4.2 4.6l.5 8.1a.8.8 0 0 0 .8.8h4.9a.8.8 0 0 0 .8-.8l.5-8.1" />
    </svg>
  );
}

export function IconClose({ size = 16, className }: IconProps) {
  return (
    <svg {...svgProps(size, className)}>
      <path d="M4 4l8 8M12 4l-8 8" />
    </svg>
  );
}

export function IconAlert({ size = 16, className }: IconProps) {
  return (
    <svg {...svgProps(size, className)}>
      <path d="M8 2.8 14 13H2L8 2.8Z" />
      <path d="M8 6.6v3M8 11.3v.1" />
    </svg>
  );
}

export function IconCheck({ size = 16, className }: IconProps) {
  return (
    <svg {...svgProps(size, className)}>
      <path d="M3.2 8.4l3 3 6.6-7" />
    </svg>
  );
}

export function IconInfo({ size = 16, className }: IconProps) {
  return (
    <svg {...svgProps(size, className)}>
      <circle cx="8" cy="8" r="5.6" />
      <path d="M8 7.2v4M8 4.9v.1" />
    </svg>
  );
}

export function IconTerminal({ size = 16, className }: IconProps) {
  return (
    <svg {...svgProps(size, className)}>
      <rect x="2" y="3" width="12" height="10" rx="1.6" />
      <path d="M4.8 6.4 6.8 8l-2 1.6M8.6 10h2.6" />
    </svg>
  );
}

export function IconApp({ size = 16, className }: IconProps) {
  return (
    <svg {...svgProps(size, className)}>
      <rect x="2.4" y="2.4" width="4.6" height="4.6" rx="1" />
      <rect x="9" y="2.4" width="4.6" height="4.6" rx="1" />
      <rect x="2.4" y="9" width="4.6" height="4.6" rx="1" />
      <rect x="9" y="9" width="4.6" height="4.6" rx="1" />
    </svg>
  );
}

/* ---------- 标题栏 / 抽屉 ---------- */

/** 搜索（标题栏全局搜索）。 */
export function IconSearch({ size = 16, className }: IconProps) {
  return (
    <svg {...svgProps(size, className)}>
      <circle cx="7.2" cy="7.2" r="3.7" />
      <path d="M10.1 10.1 13 13" />
    </svg>
  );
}

/** 浅色（太阳）。 */
export function IconSun({ size = 16, className }: IconProps) {
  return (
    <svg {...svgProps(size, className)}>
      <circle cx="8" cy="8" r="3" />
      <path d="M8 1.8v1.4M8 12.8v1.4M1.8 8h1.4M12.8 8h1.4M3.6 3.6l1 1M11.4 11.4l1 1M12.4 3.6l-1 1M4.6 11.4l-1 1" />
    </svg>
  );
}

/** 深色（月亮）。 */
export function IconMoon({ size = 16, className }: IconProps) {
  return (
    <svg {...svgProps(size, className)}>
      <path d="M13.2 9.6A5.6 5.6 0 0 1 6.4 2.8a5.6 5.6 0 1 0 6.8 6.8Z" />
    </svg>
  );
}

/** 跟随系统（显示器）。 */
export function IconMonitor({ size = 16, className }: IconProps) {
  return (
    <svg {...svgProps(size, className)}>
      <rect x="1.8" y="3" width="12.4" height="8" rx="1.6" />
      <path d="M5.6 13.4h4.8M8 11v2.4" />
    </svg>
  );
}

/**
 * 设置：三条带滑块的横线（sliders）。
 *
 * 刻意不用齿轮：齿轮在 15px 下会被画成"圆圈 + 八根辐条"，
 * 和主题切换的太阳图标几乎无法区分（实测折叠态截图里两个图标长得一样）。
 * 滑块图标形态完全不同，语义也更贴"偏好设置"。
 */
export function IconSettings({ size = 16, className }: IconProps) {
  return (
    <svg {...svgProps(size, className)}>
      <path d="M2.4 4.6h3.2M9 4.6h4.6M2.4 8h6.6M12.2 8h1.4M2.4 11.4h1.6M7.4 11.4h6.2" />
      <circle cx="7.4" cy="4.6" r="1.3" />
      <circle cx="10.6" cy="8" r="1.3" />
      <circle cx="5.8" cy="11.4" r="1.3" />
    </svg>
  );
}

/* ---------- 窗口控制（10px 字形，描边 1.2） ---------- */

/** 最小化：一条横线。 */
export function IconWindowMinimize({ size = 16, className }: IconProps) {
  return (
    <svg {...svgProps(size, className)} strokeWidth={1.2}>
      <path d="M4 8.5h8" />
    </svg>
  );
}

/** 最大化：一个方框。 */
export function IconWindowMaximize({ size = 16, className }: IconProps) {
  return (
    <svg {...svgProps(size, className)} strokeWidth={1.2}>
      <rect x="4.4" y="4.4" width="7.2" height="7.2" rx="1" />
    </svg>
  );
}

/** 还原：两个错位方框。 */
export function IconWindowRestore({ size = 16, className }: IconProps) {
  return (
    <svg {...svgProps(size, className)} strokeWidth={1.2}>
      <rect x="3.6" y="5.6" width="6.4" height="6.4" rx="1" />
      <path d="M6 5.6V4.4a1 1 0 0 1 1-1h4.4a1 1 0 0 1 1 1V9a1 1 0 0 1-1 1h-1.4" />
    </svg>
  );
}

/** 关闭：叉。 */
export function IconWindowClose({ size = 16, className }: IconProps) {
  return (
    <svg {...svgProps(size, className)} strokeWidth={1.2}>
      <path d="M4.6 4.6l6.8 6.8M11.4 4.6l-6.8 6.8" />
    </svg>
  );
}

/* ---------- 品牌 ---------- */

/**
 * 应用徽标：圆角方块里三条递减的横杠（日志行）。
 *
 * 与 `index.html` 的 favicon、以及 `src-tauri/icons/` 的应用图标是同一图形，
 * 三处必须保持一致——用户会同时看到窗口图标、任务栏缩略图和标题栏。
 *
 * 为什么是纯形状而不是字母 "ac"：标题栏徽标只有 22px，字母在这个尺寸
 * 依赖字体渲染且容易糊；三条横杠是几何图形，任何缩放下都稳定。
 *
 * 颜色刻意**不走 currentColor**（与上面那组 UI 图标不同）：这是品牌图形，
 * 不是功能性图标。用 --accent + --text-on-accent 而不是继承文字色，
 * 是为了让标题栏徽标在深浅两套主题里都和任务栏的应用图标完全同色，
 * 否则用户会看到"同一个 logo 两种颜色"。
 */
export function BrandMark({ size = 22, className }: IconProps) {
  return (
    <svg
      width={size}
      height={size}
      viewBox="0 0 32 32"
      className={className ? "brand-mark " + className : "brand-mark"}
      aria-hidden="true"
      focusable="false"
    >
      <rect x="1" y="1" width="30" height="30" rx="7.5" fill="var(--brand-bg)" />
      <g fill="var(--brand-fg)">
        <rect x="7.5" y="7.9" width="17" height="4.2" rx="2.1" />
        <rect x="7.5" y="13.9" width="12.2" height="4.2" rx="2.1" />
        <rect x="7.5" y="19.9" width="7.5" height="4.2" rx="2.1" />
      </g>
    </svg>
  );
}