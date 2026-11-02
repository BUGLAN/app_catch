/**
 * 屏蔽 WebView 的原生右键菜单。
 *
 * 原生菜单对桌面应用没有意义，还会暴露浏览器行为：开发构建里有「检查元素」，
 * 各平台都有「重新加载 / 后退 / 打印」这类会把界面状态弄乱的项。
 *
 * 只给可编辑的文本控件放行：输入框里的剪切 / 复制 / 粘贴 / 拼写建议是用户真会用的。
 * 日志等只读文本的复制走选中后 ⌘C / Ctrl+C，不受影响。
 */

/** 判断所需的最小元素形状：方便在没有 DOM 的测试环境里直接构造。 */
export interface ContextMenuTarget {
  tagName: string;
  type?: string;
  readOnly?: boolean;
  disabled?: boolean;
  isContentEditable?: boolean;
}

/** 这些 input 类型没有可编辑文本，原生菜单对它们只剩浏览器项。 */
const NON_TEXT_INPUT_TYPES = new Set([
  "button",
  "checkbox",
  "color",
  "file",
  "hidden",
  "image",
  "radio",
  "range",
  "reset",
  "submit",
]);

/** 该元素上是否保留原生右键菜单。 */
export function allowsNativeContextMenu(target: ContextMenuTarget | null): boolean {
  if (!target) {
    return false;
  }
  if (target.isContentEditable) {
    return true;
  }
  const tag = target.tagName.toLowerCase();
  if (tag === "textarea") {
    return !target.readOnly && !target.disabled;
  }
  if (tag === "input") {
    const type = (target.type ?? "text").toLowerCase();
    return !NON_TEXT_INPUT_TYPES.has(type) && !target.readOnly && !target.disabled;
  }
  return false;
}

/** 全局挂一次：除可编辑文本控件外一律吞掉 contextmenu 事件。 */
export function suppressNativeContextMenu(): void {
  if (typeof document === "undefined") {
    return;
  }
  document.addEventListener("contextmenu", (event) => {
    const target = event.target instanceof Element ? event.target : null;
    if (!allowsNativeContextMenu(target as unknown as ContextMenuTarget | null)) {
      event.preventDefault();
    }
  });
}
