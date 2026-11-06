/**
 * 可「踢一下」的兜底轮询器。
 *
 * 会话数据的主通道是后端推送（`sessions:changed` / `session:log-appended`）：
 * 收到事件就调用 {@link Poller.kick} 立刻刷新。轮询只是兜底——万一漏了事件
 * （或在浏览器预览里根本没有宿主），也会在 `interval()` 之后自己刷新一次。
 *
 * 三条规则：
 * - **同一时刻最多一轮**：上一轮还没结束时被踢，只记一笔，结束后立刻补跑一轮，
 *   绝不并发（并发会让增量游标被两轮同时推进）；
 * - **递归 setTimeout**：每轮结束后才排下一轮，慢请求不会堆积；
 * - **页面不可见时不跑**：只重新排期；页面重新可见时由调用方 `kick()` 补一轮。
 */

export interface PollerOptions {
  /** 一轮刷新；自己处理错误（这里吞掉异常，只保证调度不中断）。 */
  run: () => Promise<void>;
  /** 兜底间隔（毫秒），每轮结束后重新读取，可随状态变化。 */
  interval: () => number;
  /** 页面是否可见；不可见时跳过本轮。默认总是可见。 */
  isVisible?: () => boolean;
  /** 第一轮延迟（毫秒）；默认 0 = 立刻跑。 */
  initialDelay?: number;
}

export interface Poller {
  /** 立刻刷新一轮（正在跑时合并成「结束后再跑一轮」）。 */
  kick: () => void;
  /** 停止：清掉定时器，进行中的那一轮结束后不再排期。 */
  stop: () => void;
}

export function createPoller(options: PollerOptions): Poller {
  const { run, interval, isVisible = () => true, initialDelay = 0 } = options;

  let timer: ReturnType<typeof setTimeout> | undefined;
  let running = false;
  let kickedWhileRunning = false;
  let stopped = false;

  const clear = () => {
    if (timer !== undefined) {
      clearTimeout(timer);
      timer = undefined;
    }
  };

  const schedule = (delay: number) => {
    clear();
    timer = setTimeout(tick, delay);
  };

  async function tick(): Promise<void> {
    if (stopped) {
      return;
    }
    if (running) {
      kickedWhileRunning = true;
      return;
    }
    clear();
    if (!isVisible()) {
      schedule(interval());
      return;
    }

    running = true;
    try {
      await run();
    } catch {
      // run 自己负责报错；这里只保证调度链不断
    } finally {
      running = false;
    }

    if (stopped) {
      return;
    }
    if (kickedWhileRunning) {
      kickedWhileRunning = false;
      void tick();
      return;
    }
    schedule(interval());
  }

  schedule(initialDelay);

  return {
    kick: () => {
      void tick();
    },
    stop: () => {
      stopped = true;
      clear();
    },
  };
}
