/**
 * 动效工具的纯逻辑测试：时长解析，以及无动画环境下的退化行为。
 *
 * vitest 跑在 node 环境里，没有 window / document：
 * 这正好覆盖"拿不到 DOM / matchMedia 时必须安全退化"的分支。
 */
import { describe, expect, it } from "vitest";

import {
  durationToken,
  exitDuration,
  parseDuration,
  playViewEnter,
  prefersReducedMotion,
  withViewTransition,
} from "./motion";

describe("parseDuration", () => {
  it("解析 ms 与 s 两种单位", () => {
    expect(parseDuration("180ms")).toBe(180);
    expect(parseDuration("0.26s")).toBeCloseTo(260);
    expect(parseDuration(" 120ms ")).toBe(120);
    expect(parseDuration(".5s")).toBe(500);
  });

  it("无法解析时返回 null，而不是 NaN", () => {
    expect(parseDuration("")).toBeNull();
    expect(parseDuration("fast")).toBeNull();
    expect(parseDuration("var(--duration-fast)")).toBeNull();
    expect(parseDuration("120")).toBeNull();
  });
});

describe("无 DOM 环境的退化", () => {
  it("视为偏好减弱动效：退场不等待", () => {
    expect(prefersReducedMotion()).toBe(true);
    expect(exitDuration()).toBe(0);
  });

  it("读不到 token 时用回退值", () => {
    expect(durationToken("--duration-base", 180)).toBe(180);
  });

  it("视图进场不报错、返回 null", () => {
    expect(playViewEnter(null)).toBeNull();
  });

  it("View Transition 不可用时仍同步执行更新", () => {
    let ran = false;
    withViewTransition(() => {
      ran = true;
    });
    expect(ran).toBe(true);
  });
});
