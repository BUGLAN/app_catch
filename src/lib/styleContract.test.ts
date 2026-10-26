/**
 * 样式契约测试：把"设计 token 只允许在 tokens.css 里出现裸值"这条约定固化下来。
 *
 * 为什么用单测守这个而不是靠人眼 review：
 * 硬编码色值/圆角/字号是最容易在后续迭代里回流的坏味道（原 styles.css 里有 20+ 处裸色值、
 * 6 种圆角并存）。这条测试让"再写一个 #232938 进去"立刻变红。
 *
 * 样式表用 node:fs 直接读原文（vitest 下 CSS 的 ?raw 导入会被 CSS 插件吞成空串）；
 * 组件源码用 ?raw 导入，两者都不会进入打包产物。
 */
import { describe, expect, it } from "vitest";

import { readFileSync } from "node:fs";

import appSource from "../App.tsx?raw";
import appListSource from "../components/AppList.tsx?raw";
import sidebarSource from "../components/Sidebar.tsx?raw";
import titlebarSource from "../components/Titlebar.tsx?raw";
import windowControlsSource from "../components/WindowControls.tsx?raw";
import fieldSource from "../components/ui/Field.tsx?raw";
import dialogSource from "../components/ui/Dialog.tsx?raw";
import iconsSource from "../components/ui/Icons.tsx?raw";
import primitivesTsxSource from "../components/ui/Primitives.tsx?raw";
import toastSource from "../components/Toast.tsx?raw";

/** 读取某个样式表。 */
const readStyle = (name: string): string =>
  readFileSync(new URL("../styles/" + name, import.meta.url), "utf8");

const tokensSource = readStyle("tokens.css");

/** 组件样式表（tokens.css 是"允许出现裸值"的那一个，单独排除）。 */
const componentStyles: Array<[string, string]> = [
  "base.css",
  "layout.css",
  "chrome.css",
  "primitives.css",
  "apps.css",
].map((name) => [name, readStyle(name)]);

/** 全部样式表，用于跨文件的断言。 */
const allStyles: Array<[string, string]> = [["tokens.css", tokensSource], ...componentStyles];

const byName = (name: string): string => {
  const hit = allStyles.find(([file]) => file === name);
  if (!hit) {
    throw new Error("找不到样式表 " + name);
  }
  return hit[1];
};

/** 从 tokens.css 里抽出已定义的自定义属性名。 */
const definedTokens = new Set(
  Array.from(tokensSource.matchAll(/^\s*(--[\w-]+)\s*:/gm), (m) => String(m[1]))
);

/** 逐行检查，跳过注释行。 */
function checkLines(
  source: string,
  label: string,
  match: (line: string) => string | null
): string[] {
  const offenders: string[] = [];
  source.split("\n").forEach((line, index) => {
    if (/^\s*(\/\*|\*)/.test(line)) {
      return;
    }
    const hit = match(line);
    if (hit) {
      offenders.push(label + ":" + (index + 1) + " " + hit);
    }
  });
  return offenders;
}

describe("设计 token 契约", () => {
  it("tokens.css 里定义了完整的 token 分层", () => {
    for (const token of [
      "--surface-base",
      "--surface-panel",
      "--surface-inset",
      "--text-primary",
      "--text-secondary",
      "--text-muted",
      "--border-control",
      "--accent",
      "--danger",
      "--success",
      "--warning",
      "--focus-ring",
    ]) {
      expect(definedTokens, "缺少语义 token " + token).toContain(token);
    }
    for (const token of [
      "--space-1",
      "--space-2",
      "--space-3",
      "--space-4",
      "--radius-sm",
      "--radius-md",
      "--radius-lg",
      "--text-2xs",
      "--text-xs",
      "--text-sm",
      "--shadow-sm",
      "--duration-fast",
      "--ease-standard",
    ]) {
      expect(definedTokens, "缺少尺度 token " + token).toContain(token);
    }
  });

  it("除 tokens.css 外不再出现裸十六进制色值", () => {
    const offenders = componentStyles.flatMap(([path, source]) =>
      checkLines(source, path, (line) => {
        const hit = line.match(/#[0-9a-fA-F]{3,8}\b/);
        return hit ? hit[0] : null;
      })
    );
    expect(offenders, "发现裸色值：" + offenders.join(" / ")).toEqual([]);
  });

  it("字体尺寸只允许走 --text-* token 或百分比", () => {
    const offenders = componentStyles.flatMap(([path, source]) =>
      checkLines(source, path, (line) => {
        const match = line.match(/font-size:\s*([^;]+);/);
        if (!match) {
          return null;
        }
        const value = String(match[1]).trim();
        const allowed =
          value.startsWith("var(--text-") || value.endsWith("%") || value === "0" ||
          value === "inherit";
        return allowed ? null : value;
      })
    );
    expect(offenders, "字号应使用 --text-* token：" + offenders.join(" / ")).toEqual([]);
  });

  it("圆角只使用 --radius-* token", () => {
    const offenders = componentStyles.flatMap(([path, source]) =>
      checkLines(source, path, (line) => {
        const match = line.match(/border-radius:\s*([^;]+);/);
        if (!match) {
          return null;
        }
        const value = String(match[1]).trim();
        const ok = value
          .split(/\s+/)
          .every((part) => part.startsWith("var(--radius-") || part === "0" || part === "50%");
        return ok ? null : value;
      })
    );
    expect(offenders, "圆角应使用 --radius-* token：" + offenders.join(" / ")).toEqual([]);
  });

  it("不在组件样式里自造颜色变量", () => {
    const offenders = componentStyles.flatMap(([path, source]) =>
      checkLines(source, path, (line) => {
        const match = line.match(/^\s*--[\w-]+\s*:\s*#[0-9a-fA-F]{3,8}/);
        return match ? match[0].trim() : null;
      })
    );
    expect(offenders, "不要在组件样式里新造颜色变量：" + offenders.join(" / ")).toEqual([]);
  });

  it("所有 var(--x) 引用都在 tokens.css 里有定义", () => {
    const missing = new Set<string>();
    for (const [, source] of allStyles) {
      for (const match of source.matchAll(/var\((--[\w-]+)/g)) {
        const name = String(match[1]);
        if (!definedTokens.has(name)) {
          missing.add(name);
        }
      }
    }
    expect(Array.from(missing), "引用了未定义的 token").toEqual([]);
  });

  it("唯一允许的 !important 是 reduced-motion 的全局降级", () => {
    const offenders: string[] = [];
    for (const [path, source] of allStyles) {
      const lines = source.split("\n");
      const reducedMotionStart = lines.findIndex((line) =>
        line.includes("prefers-reduced-motion: reduce")
      );
      lines.forEach((line, index) => {
        if (/^\s*(\/\*|\*)/.test(line) || !line.includes("!important")) {
          return;
        }
        const inReducedMotion =
          reducedMotionStart >= 0 && index >= reducedMotionStart && index < reducedMotionStart + 12;
        if (!inReducedMotion) {
          offenders.push(path + ":" + (index + 1));
        }
      });
    }
    expect(offenders, "!important 只能出现在 reduced-motion 降级里：" + offenders.join(" / ")).toEqual(
      []
    );
  });

  it("交互控件都有 hover / active / disabled 三态样式", () => {
    const primitives = byName("primitives.css");
    for (const selector of [
      ".btn:hover:not(:disabled)",
      ".btn:active:not(:disabled)",
      ".btn:disabled",
      ".input:hover:not(:disabled):not(:focus)",
      ".input:focus",
      ".input:disabled",
      ".seg:hover:not(.is-active)",
      ".seg:active",
    ]) {
      expect(primitives, "缺少交互态样式 " + selector).toContain(selector);
    }
  });

  it("焦点环在 base.css 里统一提供", () => {
    const base = byName("base.css");
    expect(base).toContain(":focus-visible");
    expect(base).toContain("outline: var(--focus-ring-width) solid var(--focus-ring)");
  });

  it("支持 reduced-motion / forced-colors / 无 hover 设备", () => {
    const all = allStyles.map(([, source]) => source).join("\n");
    expect(all).toContain("prefers-reduced-motion: reduce");
    expect(all).toContain("forced-colors: active");
    expect(all).toContain("(hover: none)");
  });

  it("不写死 min-width 导致横向裁切（原 .shell 的 800px 就是这个问题）", () => {
    const layout = byName("layout.css");
    expect(layout).not.toMatch(/\.shell[^}]*min-width:\s*\d{3,}px/);
    // 主区只允许纵向滚动，横向溢出必须由内部容器自己承担
    expect(layout).toContain("overflow-x: hidden");
  });
});

describe("组件层的可访问性契约", () => {
  it("装饰性图标一律 aria-hidden", () => {
    expect(iconsSource).toContain('"aria-hidden": true');
  });

  it("表单控件通过 Field 关联错误与提示", () => {
    const field = fieldSource;
    expect(field).toContain("aria-invalid");
    expect(field).toContain("aria-describedby");
    expect(field).toContain("aria-required");
    expect(field).toContain("htmlFor={id}");
  });

  it("弹窗具备 aria-modal / aria-labelledby，并处理 Esc 与焦点陷阱", () => {
    const dialog = dialogSource;
    expect(dialog).toContain('aria-modal="true"');
    expect(dialog).toContain("aria-labelledby={titleId}");
    expect(dialog).toContain('event.key === "Escape"');
    expect(dialog).toContain('event.key !== "Tab"');
  });

  it("分段控件使用 tablist / tab / tabpanel 语义并支持方向键", () => {
    const primitives = primitivesTsxSource;
    expect(primitives).toContain('role="tablist"');
    expect(primitives).toContain('role="tab"');
    expect(primitives).toContain("aria-selected");
    expect(primitives).toContain("ArrowRight");
    expect(primitives).toContain('role="tabpanel"');
    expect(primitives).toContain("aria-controls");
  });

  it("列表项不再嵌套交互元素（选择按钮与启动按钮同级）", () => {
    const list = appListSource;
    const selectStart = list.indexOf('className="app-item-select"');
    const mainStart = list.indexOf('<span className="app-item-main"');
    expect(selectStart).toBeGreaterThan(-1);
    expect(mainStart).toBeGreaterThan(selectStart);
    // 覆盖层的选择按钮里只能有 sr-only 文案，不能再塞按钮
    const selectBlock = list.slice(selectStart, mainStart);
    expect(selectBlock).not.toContain("<button");
    expect(list).toContain('aria-current={selected ? "true" : undefined}');
  });

  it("toast 区分 role=status 与 role=alert，并在悬停时暂停倒计时", () => {
    const toast = toastSource;
    expect(toast).toContain('role={toast.tone === "error" ? "alert" : "status"}');
    expect(toast).toContain("onMouseEnter={pause}");
  });

  it("应用壳提供跳转链接、landmark 名称与 aria-busy", () => {
    expect(appSource).toContain('className="skip-link"');
    expect(appSource).toContain('href="#main-content"');
    expect(appSource).toContain('id="main-content"');
    expect(appSource).toContain("aria-busy={loading}");
    // 侧栏已抽成独立组件，landmark 语义在 Sidebar 里
    expect(sidebarSource).toContain('aria-label="应用列表"');
    expect(sidebarSource).toContain("<aside");
  });

  it("自绘标题栏：拖拽区、窗口按钮、macOS 安全区三件事都在", () => {
    // 拖拽交给 Tauri 的 window 插件；自己再绑 onDoubleClick 会切两次
    expect(titlebarSource).toContain('data-tauri-drag-region="deep"');
    expect(titlebarSource).not.toContain("onDoubleClick");
    // 窗口按钮只出现在 Windows/Linux；macOS 由 CSS 隐藏并让系统画红黄绿
    expect(windowControlsSource).toContain("最小化");
    expect(windowControlsSource).toContain("toggleMaximize");
    expect(windowControlsSource).toContain("windowApi.minimize");
    expect(windowControlsSource).toContain("windowApi.close");
    expect(windowControlsSource).toContain("onMaximizedChange");
    const chrome = byName("chrome.css");
    expect(chrome).toContain('[data-platform="macos"] .titlebar');
    expect(chrome).toContain("--titlebar-mac-inset");
    expect(chrome).toContain('[data-platform="macos"] .window-controls');
    expect(chrome).toContain("-webkit-app-region: drag");
    expect(chrome).toContain("no-drag");
    expect(chrome).toContain(".window-control-close:hover");
  });

  it("抽屉：折叠宽度、拖拽热区、过渡时长都在 token 与样式里", () => {
    const chrome = byName("chrome.css");
    // 折叠/展开必须有过渡，拖拽期间必须关掉
    expect(chrome).toContain("transition: width var(--duration-base)");
    expect(chrome).toContain('.sidebar[data-resizing="true"]');
    // 4px 热区 + col-resize 光标
    expect(chrome).toContain("cursor: col-resize");
    expect(chrome).toContain("--rail-handle-width");
    // 宽度约束来自 token
    for (const token of [
      "--rail-min-width",
      "--rail-max-width",
      "--rail-collapsed-width",
      "--rail-expanded-width",
    ]) {
      expect(definedTokens, "缺少尺寸 token " + token).toContain(token);
    }
    // 折叠状态与宽度持久化到 localStorage
    expect(sidebarSource).toContain("rail-handle");
  });

  it("骨架屏与空态是两套独立状态", () => {
    const primitives = primitivesTsxSource;
    expect(primitives).toContain("export function EmptyState");
    expect(primitives).toContain("export function EmptyNotice");
    expect(primitives).toContain("export function ListSkeleton");
    expect(primitives).toContain("export function PanelSkeleton");
  });
});