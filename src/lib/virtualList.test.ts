import { describe, expect, it } from "vitest";

import { buildOffsets, centeredScrollTop, visibleRange } from "./virtualList";

describe("buildOffsets", () => {
  it("按行高累加，最后一项是总高度", () => {
    expect(buildOffsets(3, (i) => [10, 20, 30][i]!)).toEqual([0, 10, 30, 60]);
    expect(buildOffsets(0, () => 10)).toEqual([0]);
  });
});

describe("visibleRange", () => {
  const offsets = buildOffsets(100, () => 20); // 每行 20px，总高 2000

  it("视口在顶部", () => {
    expect(visibleRange(offsets, 0, 100, 0)).toEqual({ start: 0, end: 5 });
  });

  it("视口在中间，部分可见的行也算进来", () => {
    // 视口 [210, 310)：第 10 行 [200,220) 部分可见，第 15 行 [300,320) 部分可见
    expect(visibleRange(offsets, 210, 100, 0)).toEqual({ start: 10, end: 16 });
  });

  it("overscan 向上下各扩一段，并夹在合法范围", () => {
    expect(visibleRange(offsets, 0, 100, 40)).toEqual({ start: 0, end: 7 });
    expect(visibleRange(offsets, 1900, 100, 40)).toEqual({ start: 93, end: 100 });
  });

  it("滚过头或视口高度为 0 时仍至少渲染一行", () => {
    expect(visibleRange(offsets, 5000, 100, 0)).toEqual({ start: 99, end: 100 });
    expect(visibleRange(offsets, 0, 0, 0)).toEqual({ start: 0, end: 1 });
  });

  it("没有行时为空区间", () => {
    expect(visibleRange([0], 0, 100, 50)).toEqual({ start: 0, end: 0 });
  });

  it("行高不一致时按真实偏移查找", () => {
    const mixed = buildOffsets(4, (i) => [10, 100, 10, 10][i]!); // [0,10,110,120,130]
    expect(visibleRange(mixed, 50, 20, 0)).toEqual({ start: 1, end: 2 });
    expect(visibleRange(mixed, 105, 10, 0)).toEqual({ start: 1, end: 3 });
  });
});

describe("centeredScrollTop", () => {
  const offsets = buildOffsets(100, () => 20);

  it("把目标行放到视口中间", () => {
    // 第 50 行顶部 1000，视口 200：1000 - (200 - 20) / 2 = 910
    expect(centeredScrollTop(offsets, 50, 200)).toBe(910);
  });

  it("夹在 [0, 总高 - 视口] 之间", () => {
    expect(centeredScrollTop(offsets, 0, 200)).toBe(0);
    expect(centeredScrollTop(offsets, 99, 200)).toBe(1800);
    expect(centeredScrollTop([0], 0, 200)).toBe(0);
  });
});
