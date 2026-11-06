import { describe, expect, it } from "vitest";

import {
  aggregateState,
  latestRunning,
  noticeTouches,
  pickDefaultSession,
  upsertSession,
} from "./sessions";
import type { RunState, SessionSnapshot } from "./types";

function session(id: string, state: RunState, appId = "demo"): SessionSnapshot {
  return {
    session_id: id,
    app_id: appId,
    pid: 1,
    state,
    started_at: "2026-11-05T00:00:00Z",
    last_exit: state === "running" ? null : { code: state === "exited" ? 0 : 1, reason: "normal" },
    latest_status: null,
  };
}

describe("pickDefaultSession", () => {
  it("优先最近启动的运行中会话", () => {
    const list = [session("a", "running"), session("b", "running"), session("c", "crashed")];
    expect(pickDefaultSession(list)).toBe("b");
  });

  it("全部退出时选最近启动的那个（崩溃后停在它的日志上）", () => {
    const list = [session("a", "exited"), session("b", "crashed")];
    expect(pickDefaultSession(list)).toBe("b");
  });

  it("没有会话返回 null", () => {
    expect(pickDefaultSession([])).toBeNull();
  });
});

describe("latestRunning", () => {
  it("返回最后一个运行中会话", () => {
    const list = [session("a", "running"), session("b", "running"), session("c", "exited")];
    expect(latestRunning(list)?.session_id).toBe("b");
    expect(latestRunning([session("x", "exited")])).toBeNull();
  });
});

describe("upsertSession", () => {
  it("不存在时追加，存在时原样返回", () => {
    const list = [session("a", "running")];
    const appended = upsertSession(list, session("b", "running"));
    expect(appended.map((s) => s.session_id)).toEqual(["a", "b"]);
    expect(upsertSession(appended, session("b", "running"))).toBe(appended);
  });
});

describe("aggregateState", () => {
  it("有运行中会话即运行中", () => {
    expect(aggregateState("demo", [session("a", "crashed"), session("b", "running")])).toBe(
      "running"
    );
  });

  it("全部退出时取最近一次的结果", () => {
    expect(aggregateState("demo", [session("a", "crashed"), session("b", "exited")])).toBe(
      "exited"
    );
    expect(aggregateState("demo", [session("a", "exited"), session("b", "crashed")])).toBe(
      "crashed"
    );
  });

  it("只看本应用的会话", () => {
    expect(aggregateState("demo", [session("a", "running", "other")])).toBe("idle");
  });
});

describe("noticeTouches", () => {
  it("命中列表里的会话，或 null 表示全部", () => {
    expect(noticeTouches({ session_ids: ["a", "b"] }, "a")).toBe(true);
    expect(noticeTouches({ session_ids: ["b"] }, "a")).toBe(false);
    expect(noticeTouches({ session_ids: null }, "a")).toBe(true);
  });
});
