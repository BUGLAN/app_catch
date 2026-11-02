import { describe, expect, it } from "vitest";

import { allowsNativeContextMenu } from "./contextMenu";

describe("原生右键菜单放行规则", () => {
  it.each([
    ["文本输入框", { tagName: "INPUT", type: "text" }],
    ["搜索框", { tagName: "INPUT", type: "search" }],
    ["未写 type 的输入框", { tagName: "INPUT" }],
    ["多行文本框", { tagName: "TEXTAREA" }],
    ["contenteditable", { tagName: "DIV", isContentEditable: true }],
  ])("%s 保留原生菜单（剪切 / 复制 / 粘贴）", (_label, target) => {
    expect(allowsNativeContextMenu(target)).toBe(true);
  });

  it.each([
    ["普通容器", { tagName: "DIV" }],
    ["日志文本", { tagName: "SPAN" }],
    ["按钮", { tagName: "BUTTON" }],
    ["复选框", { tagName: "INPUT", type: "checkbox" }],
    ["只读输入框", { tagName: "INPUT", type: "text", readOnly: true }],
    ["禁用的多行文本框", { tagName: "TEXTAREA", disabled: true }],
  ])("%s 屏蔽原生菜单", (_label, target) => {
    expect(allowsNativeContextMenu(target)).toBe(false);
  });

  it("没有目标元素时屏蔽", () => {
    expect(allowsNativeContextMenu(null)).toBe(false);
  });
});
