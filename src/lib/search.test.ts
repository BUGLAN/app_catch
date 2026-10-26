/**
 * 侧栏搜索的纯逻辑测试。
 *
 * 搜索是"看起来最简单、坏了最难发现"的那类功能：
 * 匹配不到时界面上只显示一句"没有匹配"，看不出是逻辑错还是真没数据。
 */
import { describe, expect, it } from "vitest";

import { filterApps, matchesQuery } from "./search";
import type { AppConfig } from "./types";

const make = (id: string, name: string, tags: string[] = []): AppConfig => ({
  id,
  name,
  command: "python",
  args: [],
  env: {},
  icon: null,
  tags,
});

const apps = [
  make("gpt-sovits", "GPT-SoVITS 语音克隆", ["python", "tts"]),
  make("trendradar", "TrendRadar", ["crawler"]),
  make("charmap", "字符映射表", ["gui", "系统"]),
];

describe("matchesQuery", () => {
  it("空查询恒为真", () => {
    expect(matchesQuery(apps[0]!, "")).toBe(true);
    expect(matchesQuery(apps[0]!, "   ")).toBe(true);
  });

  it("按名称匹配，忽略大小写", () => {
    expect(matchesQuery(apps[0]!, "sovits")).toBe(true);
    expect(matchesQuery(apps[0]!, "语音")).toBe(true);
    expect(matchesQuery(apps[1]!, "TREND")).toBe(true);
  });

  it("按 id 匹配", () => {
    expect(matchesQuery(apps[2]!, "charmap")).toBe(true);
  });

  it("按标签匹配（含中文标签）", () => {
    expect(matchesQuery(apps[0]!, "tts")).toBe(true);
    expect(matchesQuery(apps[2]!, "系统")).toBe(true);
  });

  it("首尾空白不影响结果", () => {
    expect(matchesQuery(apps[0]!, "  tts  ")).toBe(true);
  });

  it("不匹配时返回 false", () => {
    expect(matchesQuery(apps[0]!, "crawler")).toBe(false);
    expect(matchesQuery(apps[0]!, "不存在的应用")).toBe(false);
  });

  it("标签缺失（null）时不会抛错", () => {
    const noTags = { ...make("x", "X"), tags: undefined } as unknown as AppConfig;
    expect(matchesQuery(noTags, "anything")).toBe(false);
    expect(matchesQuery(noTags, "x")).toBe(true);
  });
});

describe("filterApps", () => {
  it("空查询返回原数组引用（不制造新数组）", () => {
    expect(filterApps(apps, "")).toBe(apps);
    expect(filterApps(apps, "   ")).toBe(apps);
  });

  it("过滤后只保留命中的项", () => {
    const result = filterApps(apps, "tts");
    expect(result).toHaveLength(1);
    expect(result[0]!.id).toBe("gpt-sovits");
  });

  it("无命中时返回空数组而不是原数组", () => {
    expect(filterApps(apps, "zzz")).toEqual([]);
  });
});