/**
 * 主题三态轮转的纯逻辑测试。
 *
 * 这里守的是一个具体的回归：早期实现是二元 toggle（浅 ↔ 深），
 * 结果用户从"跟随系统"点出去之后就再也回不到"跟随系统"。
 * 三态轮转必须能回到起点。
 */
import { describe, expect, it } from "vitest";

import { nextThemeMode, resolveTheme, THEME_ORDER, type ThemeMode } from "./useTheme";

describe("nextThemeMode", () => {
  it("按 system → light → dark → system 轮转", () => {
    expect(nextThemeMode("system")).toBe("light");
    expect(nextThemeMode("light")).toBe("dark");
    expect(nextThemeMode("dark")).toBe("system");
  });

  it("三次点击回到起点（能回到跟随系统）", () => {
    let mode: ThemeMode = "system";
    for (let i = 0; i < THEME_ORDER.length; i += 1) {
      mode = nextThemeMode(mode);
    }
    expect(mode).toBe("system");
  });

  it("轮转只会在三个合法模式之间跳，不会返回 undefined", () => {
    for (const mode of THEME_ORDER) {
      expect(THEME_ORDER).toContain(nextThemeMode(mode));
    }
  });

  it("顺序常量与需求一致：system 在前，便于文案统一", () => {
    expect(THEME_ORDER).toEqual(["system", "light", "dark"]);
  });
});

describe("resolveTheme", () => {
  it("跟随系统时读系统偏好", () => {
    expect(resolveTheme("system", true)).toBe("dark");
    expect(resolveTheme("system", false)).toBe("light");
  });

  it("强制模式忽略系统偏好", () => {
    expect(resolveTheme("light", true)).toBe("light");
    expect(resolveTheme("dark", false)).toBe("dark");
  });
});
