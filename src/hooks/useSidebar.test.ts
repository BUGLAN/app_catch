/**
 * 抽屉宽度的纯逻辑测试。
 *
 * 这些函数决定"刷新后侧栏恢复成什么样"，出错的表现很隐蔽
 * （宽度差几像素、拖到边界不回弹），几乎不可能靠肉眼在截图里发现。
 */
import { describe, expect, it } from "vitest";

import {
  clampRailWidth,
  RAIL_COLLAPSED_WIDTH,
  RAIL_DEFAULT_WIDTH,
  RAIL_MAX_WIDTH,
  RAIL_MIN_WIDTH,
  RAIL_SNAP_WIDTH,
  resolveRailDrag,
} from "./useSidebar";

describe("clampRailWidth", () => {
  it("保留合法宽度（取整）", () => {
    expect(clampRailWidth(200)).toBe(200);
    expect(clampRailWidth(200.4)).toBe(200);
    expect(clampRailWidth(200.6)).toBe(201);
  });

  it("夹到 [184, 320]", () => {
    expect(clampRailWidth(0)).toBe(RAIL_MIN_WIDTH);
    expect(clampRailWidth(-200)).toBe(RAIL_MIN_WIDTH);
    expect(clampRailWidth(128)).toBe(RAIL_MIN_WIDTH);
    expect(clampRailWidth(321)).toBe(RAIL_MAX_WIDTH);
    expect(clampRailWidth(9999)).toBe(RAIL_MAX_WIDTH);
  });

  it("边界值本身合法", () => {
    expect(clampRailWidth(RAIL_MIN_WIDTH)).toBe(RAIL_MIN_WIDTH);
    expect(clampRailWidth(RAIL_MAX_WIDTH)).toBe(RAIL_MAX_WIDTH);
  });

  it("NaN / Infinity 退回默认宽度（localStorage 被手改过的情况）", () => {
    expect(clampRailWidth(Number.NaN)).toBe(RAIL_DEFAULT_WIDTH);
    expect(clampRailWidth(Number.POSITIVE_INFINITY)).toBe(RAIL_DEFAULT_WIDTH);
    expect(clampRailWidth(Number.NEGATIVE_INFINITY)).toBe(RAIL_DEFAULT_WIDTH);
  });

  it("默认宽度落在合法区间内，且折叠宽度就是需求里的 56", () => {
    expect(RAIL_DEFAULT_WIDTH).toBeGreaterThanOrEqual(RAIL_MIN_WIDTH);
    expect(RAIL_DEFAULT_WIDTH).toBeLessThanOrEqual(RAIL_MAX_WIDTH);
    expect(RAIL_COLLAPSED_WIDTH).toBe(56);
    // 展开态下限必须明显宽于折叠态，否则会出现文字被挤成一字一行的半折叠态
    expect(RAIL_MIN_WIDTH).toBeGreaterThan(RAIL_COLLAPSED_WIDTH * 3);
  });
});

describe("resolveRailDrag", () => {
  it("拖到阈值以下吸附为折叠，并保留上次的展开宽度", () => {
    expect(resolveRailDrag(RAIL_SNAP_WIDTH - 1, 240)).toEqual({ collapsed: true, width: 240 });
    expect(resolveRailDrag(0, 240)).toEqual({ collapsed: true, width: 240 });
  });

  it("阈值与下限之间展开到下限，不允许半折叠宽度", () => {
    expect(resolveRailDrag(RAIL_SNAP_WIDTH, 240)).toEqual({ collapsed: false, width: RAIL_MIN_WIDTH });
    expect(resolveRailDrag(128, 240)).toEqual({ collapsed: false, width: RAIL_MIN_WIDTH });
  });

  it("正常区间原样取整", () => {
    expect(resolveRailDrag(250.4, 220)).toEqual({ collapsed: false, width: 250 });
    expect(resolveRailDrag(999, 220)).toEqual({ collapsed: false, width: RAIL_MAX_WIDTH });
  });
});
