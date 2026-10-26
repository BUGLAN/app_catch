/**
 * 界面偏好存储键的契约测试。
 *
 * 键名一旦改动，所有老用户存下的偏好都会"静默失效"——不会报错，
 * 只是刷新后侧栏回到默认宽度、主题回到跟随系统，非常难被发现。
 * 所以把它们钉在测试里，改名字必须先改这里（等于一次有意识的决定）。
 */
import { describe, expect, it } from "vitest";

import { STORAGE_KEYS } from "./preferences";

describe("STORAGE_KEYS", () => {
  it("键名保持稳定", () => {
    expect(STORAGE_KEYS.theme).toBe("app_catch.theme");
    expect(STORAGE_KEYS.rail).toBe("app_catch.sidebar");
  });

  it("全部带 app_catch 前缀，避免与同源的其他页面冲突", () => {
    for (const value of Object.values(STORAGE_KEYS)) {
      expect(value.startsWith("app_catch.")).toBe(true);
    }
  });

  it("键名互不重复", () => {
    const values = Object.values(STORAGE_KEYS);
    expect(new Set(values).size).toBe(values.length);
  });
});
