import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";

import { createPoller } from "./poller";

/** 让排在微任务队列里的 then 回调全部跑完。 */
async function flush(): Promise<void> {
  for (let i = 0; i < 5; i += 1) {
    await Promise.resolve();
  }
}

describe("createPoller", () => {
  beforeEach(() => {
    vi.useFakeTimers();
  });
  afterEach(() => {
    vi.useRealTimers();
  });

  it("按 initialDelay 跑第一轮，之后按 interval 兜底", async () => {
    const run = vi.fn(async () => {});
    const poller = createPoller({ run, interval: () => 5000, initialDelay: 1000 });

    await vi.advanceTimersByTimeAsync(999);
    expect(run).toHaveBeenCalledTimes(0);
    await vi.advanceTimersByTimeAsync(1);
    expect(run).toHaveBeenCalledTimes(1);
    await vi.advanceTimersByTimeAsync(5000);
    expect(run).toHaveBeenCalledTimes(2);
    poller.stop();
  });

  it("kick 立刻跑一轮并重置兜底计时", async () => {
    const run = vi.fn(async () => {});
    const poller = createPoller({ run, interval: () => 5000, initialDelay: 5000 });

    await vi.advanceTimersByTimeAsync(3000);
    poller.kick();
    await flush();
    expect(run).toHaveBeenCalledTimes(1);

    // 原定 5000ms 的那一轮不应再跑：下一轮从 kick 结束起算
    await vi.advanceTimersByTimeAsync(2000);
    expect(run).toHaveBeenCalledTimes(1);
    await vi.advanceTimersByTimeAsync(3000);
    expect(run).toHaveBeenCalledTimes(2);
    poller.stop();
  });

  it("运行中被多次踢只补跑一轮，且绝不并发", async () => {
    let active = 0;
    let maxActive = 0;
    const resolvers: Array<() => void> = [];
    const run = vi.fn(
      () =>
        new Promise<void>((resolve) => {
          active += 1;
          maxActive = Math.max(maxActive, active);
          resolvers.push(() => {
            active -= 1;
            resolve();
          });
        })
    );
    const poller = createPoller({ run, interval: () => 60_000 });

    await vi.advanceTimersByTimeAsync(0);
    expect(run).toHaveBeenCalledTimes(1);

    poller.kick();
    poller.kick();
    poller.kick();
    await flush();
    expect(run).toHaveBeenCalledTimes(1);

    resolvers.shift()?.();
    await flush();
    expect(run).toHaveBeenCalledTimes(2);
    resolvers.shift()?.();
    await flush();
    expect(run).toHaveBeenCalledTimes(2);
    expect(maxActive).toBe(1);
    poller.stop();
  });

  it("页面不可见时跳过，只重新排期", async () => {
    let visible = false;
    const run = vi.fn(async () => {});
    const poller = createPoller({ run, interval: () => 1000, isVisible: () => visible });

    await vi.advanceTimersByTimeAsync(3000);
    expect(run).toHaveBeenCalledTimes(0);

    visible = true;
    poller.kick();
    await flush();
    expect(run).toHaveBeenCalledTimes(1);
    poller.stop();
  });

  it("run 抛错不打断调度", async () => {
    const run = vi.fn(async () => {
      throw new Error("boom");
    });
    const poller = createPoller({ run, interval: () => 1000 });

    await vi.advanceTimersByTimeAsync(0);
    await vi.advanceTimersByTimeAsync(1000);
    expect(run).toHaveBeenCalledTimes(2);
    poller.stop();
  });

  it("stop 之后不再跑", async () => {
    const run = vi.fn(async () => {});
    const poller = createPoller({ run, interval: () => 1000 });
    await vi.advanceTimersByTimeAsync(0);
    poller.stop();
    poller.kick();
    await vi.advanceTimersByTimeAsync(5000);
    expect(run).toHaveBeenCalledTimes(1);
  });
});
