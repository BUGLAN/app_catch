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
} from "./useSidebar";

describe("clampRailWidth", () => {
  it("保留合法宽度（取整）", () => {
    expect(clampRailWidth(180)).toBe(180);
    expect(clampRailWidth(180.4)).toBe(180);
    expect(clampRailWidth(180.6)).toBe(181);
  });

  it("夹到 [56, 320]", () => {
    expect(clampRailWidth(0)).toBe(RAIL_MIN_WIDTH);
    expect(clampRailWidth(-200)).toBe(RAIL_MIN_WIDTH);
    expect(clampRailWidth(55.9)).toBe(RAIL_MIN_WIDTH);
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
    // 折叠宽度必须等于最小宽度，否则"折叠"和"拖到最窄"会得到两个不同结果
    expect(RAIL_COLLAPSED_WIDTH).toBe(RAIL_MIN_WIDTH);
  });
});
