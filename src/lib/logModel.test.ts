/**
 * logModel 纯逻辑测试。
 *
 * 覆盖需求里点名的三件事：分片顺序合并、增量游标推进、lossy 标记判断，
 * 外加自动滚底判定与参数/环境变量解析等表单逻辑。
 */
import { describe, expect, it } from "vitest";

import {
  advanceCursor,
  appendRows,
  classifyOutputLevel,
  describeExit,
  describeStatus,
  eventToRows,
  exitReasonLabel,
  formatClock,
  formatDateTime,
  initialOf,
  INITIAL_CURSOR,
  isSessionGone,
  looksLikeError,
  MAX_LOG_ROWS,
  pollInterval,
  POLL_FAST_MS,
  POLL_IDLE_MS,
  renderCommand,
  runStateLabel,
  shouldAutoScroll,
} from "./logModel";
import type { LogCursor } from "./logModel";
import type { SessionEvent, SessionEventsPage } from "./types";

/** 构造一个只含 output 事件的分页结果。 */
function outputPage(lines: string[], nextLine: number, hasMore = false): SessionEventsPage {
  const events: SessionEvent[] = lines.map((line, index) => ({
    event: "output",
    v: 1,
    ts: `2026-10-24T18:30:0${index}Z`,
    stream: "stdout",
    line,
    encoding: "utf-8",
  }));
  return { events, next_line: nextLine, has_more: hasMore, skipped_lines: 0 };
}

describe("advanceCursor 增量游标推进", () => {
  it("首屏从 0 推进到服务端返回的行号", () => {
    const { cursor, rows } = advanceCursor(INITIAL_CURSOR, outputPage(["a", "b"], 2), 0);
    expect(cursor.nextLine).toBe(2);
    expect(cursor.totalEvents).toBe(2);
    expect(rows.map((row) => row.text)).toEqual(["a", "b"]);
  });

  it("多轮轮询只消费新增内容，游标单调递增", () => {
    const first = advanceCursor(INITIAL_CURSOR, outputPage(["a0", "a1"], 2), 0);
    const second = advanceCursor(first.cursor, outputPage(["b0"], 3), 2);
    const third = advanceCursor(second.cursor, outputPage([], 3), 3);

    expect(first.cursor.nextLine).toBe(2);
    expect(second.cursor.nextLine).toBe(3);
    expect(third.cursor.nextLine).toBe(3);
    expect(second.rows.map((row) => row.text)).toEqual(["b0"]);
    expect(third.rows).toEqual([]);
    expect(third.cursor.totalEvents).toBe(3);
  });

  it("服务端行号回退时（历史被裁剪）标记 truncated 且游标跟随后退", () => {
    const prev: LogCursor = { nextLine: 500, totalEvents: 500, skippedLines: 0, truncated: false };
    const page: SessionEventsPage = {
      events: [],
      next_line: 12,
      has_more: false,
      skipped_lines: 0,
    };
    const { cursor } = advanceCursor(prev, page, 500);
    expect(cursor.nextLine).toBe(12);
    expect(cursor.truncated).toBe(true);
  });

  it("累计无法解析的行数", () => {
    const page: SessionEventsPage = {
      events: [],
      next_line: 5,
      has_more: false,
      skipped_lines: 3,
    };
    const first = advanceCursor(INITIAL_CURSOR, page, 0);
    expect(first.cursor.skippedLines).toBe(3);

    const second = advanceCursor(first.cursor, page, 5);
    expect(second.cursor.skippedLines).toBe(6);
  });

  it("空页不改变游标", () => {
    const { cursor, rows } = advanceCursor(INITIAL_CURSOR, outputPage([], 0), 0);
    expect(cursor.nextLine).toBe(0);
    expect(rows).toEqual([]);
  });
});

describe("分片顺序合并", () => {
  it("跨分片事件按全局行号顺序转换，key 不重复", () => {
    const shard1: SessionEvent[] = [
      { event: "start", v: 1, ts: "t0", session_id: "s", app_id: "a", cmd: "cmd", cwd: "C:/", argv: ["cmd"], pid: 1, ppid: 2 },
      { event: "output", v: 1, ts: "t1", stream: "stdout", line: "from-shard-1" },
    ];
    const shard2: SessionEvent[] = [
      { event: "output", v: 1, ts: "t2", stream: "stderr", line: "from-shard-2" },
      { event: "exit", v: 1, ts: "t3", code: 0, reason: "normal" },
    ];

    // 服务端已把两个分片合并成一个连续序列，这里验证行号映射不串位
    const page: SessionEventsPage = {
      events: shard1.concat(shard2),
      next_line: 4,
      has_more: false,
      skipped_lines: 0,
    };
    const { rows } = advanceCursor(INITIAL_CURSOR, page, 0);
    expect(rows.length).toBe(4);
    expect(new Set(rows.map((row) => row.key)).size).toBe(4);
    expect(rows.map((row) => row.kind)).toEqual(["divider", "output", "output", "divider"]);
    expect(rows[2]!.stream).toBe("stderr");
  });
});

describe("lossy 编码标记", () => {
  it("encoding=lossy 的行被标记为 warn 且 lossy=true", () => {
    const event: SessionEvent = {
      event: "output",
      v: 1,
      ts: "2026-10-24T18:30:00Z",
      stream: "stdout",
      line: "损坏的内容",
      encoding: "lossy",
    };
    const rows = eventToRows(event, 0);
    expect(rows).toHaveLength(1);
    expect(rows[0]!.lossy).toBe(true);
    expect(rows[0]!.level).toBe("warn");
  });

  it("gbk 与 utf-8 都不算损坏", () => {
    for (const encoding of ["utf-8", "gbk"] as const) {
      const event: SessionEvent = {
        event: "output",
        v: 1,
        ts: "t",
        stream: "stdout",
        line: "正常",
        encoding,
      };
      expect(eventToRows(event, 0)[0]!.lossy).toBe(false);
    }
  });

  it("缺少 encoding 字段的旧事件按 utf-8 处理", () => {
    const legacy: SessionEvent = {
      event: "output",
      v: 1,
      ts: "t",
      stream: "stdout",
      line: "旧日志",
    };
    expect(eventToRows(legacy, 0)[0]!.lossy).toBe(false);
  });

  it("普通 stderr 标为 warn（不再一律吃红色），但不算编码损坏", () => {
    const event: SessionEvent = {
      event: "output",
      v: 1,
      ts: "t",
      stream: "stderr",
      line: '$ concurrently -k "vite --strictPort" "wait-on http://127.0.0.1:8090"',
    };
    const row = eventToRows(event, 0)[0]!;
    expect(row.level).toBe("warn");
    expect(row.lossy).toBe(false);
  });

  it("stderr 里的真实错误标为 error", () => {
    const event: SessionEvent = {
      event: "output",
      v: 1,
      ts: "t",
      stream: "stderr",
      line: "Error: Cannot find module 'vite'",
    };
    expect(eventToRows(event, 0)[0]!.level).toBe("error");
  });

  it("stdout 里的错误也标为 error（不只看流）", () => {
    const event: SessionEvent = {
      event: "output",
      v: 1,
      ts: "t",
      stream: "stdout",
      line: "thread 'main' panicked at src/main.rs:12",
    };
    expect(eventToRows(event, 0)[0]!.level).toBe("error");
  });

  it("常规 stdout 保持 normal", () => {
    const event: SessionEvent = {
      event: "output",
      v: 1,
      ts: "t",
      stream: "stdout",
      line: "Already up to date",
    };
    expect(eventToRows(event, 0)[0]!.level).toBe("normal");
  });
});

describe("classifyOutputLevel 的边界", () => {
  it("lossy 优先于一切：即使命中错误特征也标 warn（编码损坏单独表达）", () => {
    expect(classifyOutputLevel("Error: 乱码", "stderr", true)).toBe("warn");
  });

  it("未定义 stream 时按 stdout 处理", () => {
    expect(classifyOutputLevel("hello", undefined, false)).toBe("normal");
  });

  it.each([
    ["panic", "thread panicked at ..."],
    ["uncaught", "Uncaught TypeError: x is not a function"],
    ["EADDRINUSE", "listen EADDRINUSE: address already in use :::15173"],
    ["ERR!", "ERR! code ELIFECYCLE"],
    ["cannot find", "Cannot find module 'electron'"],
    ["fatal", "fatal: not a git repository"],
  ])("命中错误特征 %s", (_label, line) => {
    expect(looksLikeError(line)).toBe(true);
  });

  it.each([
    ["脚本回显", '$ concurrently -k "vite"'],
    ["升级提示", "Already up to date"],
    ["完成提示", "Done in 443ms using pnpm v11.21.0"],
    ["弃用警告", "(node:1) [DEP0060] DeprecationWarning: The `util._extend` API is deprecated."],
    ["中文常规输出", "同步忽略规则已生成：electron/ignore-rules.cjs"],
  ])("不把常规输出 %s 误判为错误", (_label, line) => {
    expect(looksLikeError(line)).toBe(false);
  });
});


describe("overflow 与状态/退出事件的渲染形态", () => {
  it("overflow 渲染成 notice，文案带丢弃行数", () => {
    const [row] = eventToRows({ event: "overflow", v: 1, ts: "t", dropped: 128 }, 0);
    expect(row!.kind).toBe("notice");
    expect(row!.text).toContain("128");
  });

  it("status 与 exit 渲染成分隔行，不混进日志行", () => {
    const [status] = eventToRows(
      { event: "status", v: 1, ts: "t", state: "running", window_title: "主窗口", ports: [8000] },
      0
    );
    expect(status!.kind).toBe("divider");
    expect(status!.text).toContain("运行中");
    expect(status!.text).toContain("主窗口");
    expect(status!.text).toContain("8000");

    const [exit] = eventToRows({ event: "exit", v: 1, ts: "t", code: 1, reason: "crashed" }, 1);
    expect(exit!.kind).toBe("divider");
    expect(exit!.text).toContain("退出码 1");
    expect(exit!.text).toContain("异常崩溃");
  });

  it("未知事件类型不产生日志行（向前兼容）", () => {
    const unknown = { event: "future_event", v: 1, ts: "t" } as unknown as SessionEvent;
    expect(eventToRows(unknown, 0)).toEqual([]);
  });
});

describe("日志缓冲裁剪", () => {
  it("不超上限时原样追加", () => {
    const existing = eventToRows(
      { event: "output", v: 1, ts: "t", stream: "stdout", line: "a" },
      0
    );
    const incoming = eventToRows(
      { event: "output", v: 1, ts: "t", stream: "stdout", line: "b" },
      1
    );
    expect(appendRows(existing, incoming).length).toBe(2);
  });

  it("超过上限时丢弃最旧的行", () => {
    const make = (index: number) =>
      eventToRows(
        { event: "output", v: 1, ts: `t${index}`, stream: "stdout", line: `line-${index}` },
        index
      );
    let rows = [] as ReturnType<typeof eventToRows>;
    for (let i = 0; i < MAX_LOG_ROWS + 10; i += 1) {
      rows = appendRows(rows, make(i));
    }
    expect(rows.length).toBe(MAX_LOG_ROWS);
    expect(rows[0]!.text).toBe("line-10");
  });

  it("空输入返回原数组（避免无意义重渲染）", () => {
    const existing = eventToRows(
      { event: "output", v: 1, ts: "t", stream: "stdout", line: "a" },
      0
    );
    expect(appendRows(existing, [])).toBe(existing);
  });
});

describe("自动滚底判定", () => {
  it("在底部时自动跟随", () => {
    expect(shouldAutoScroll(1000, 1400, 400)).toBe(true);
  });

  it("容差范围内的轻微上滚仍算底部", () => {
    // 距底部 30px，在 32px 容差内
    expect(shouldAutoScroll(970, 1400, 400)).toBe(true);
  });

  it("超出容差就暂停自动滚动", () => {
    // 距底部 40px，超出 32px 容差
    expect(shouldAutoScroll(960, 1400, 400)).toBe(false);
  });

  it("用户上滚阅读历史时暂停自动滚动", () => {
    expect(shouldAutoScroll(200, 1400, 400)).toBe(false);
  });

  it("内容不足一屏时视为在底部", () => {
    expect(shouldAutoScroll(0, 300, 400)).toBe(true);
  });
});

describe("轮询策略", () => {
  it("有运行中会话时用快间隔", () => {
    expect(pollInterval(1)).toBe(POLL_FAST_MS);
    expect(pollInterval(3)).toBe(POLL_FAST_MS);
  });

  it("无运行中会话时降到慢间隔", () => {
    expect(pollInterval(0)).toBe(POLL_IDLE_MS);
    expect(POLL_IDLE_MS).toBeGreaterThan(POLL_FAST_MS);
  });
});

describe("格式化与文案", () => {
  it("时间戳渲染为 HH:MM:SS.mmm", () => {
    const formatted = formatClock("2026-10-24T18:30:00.123Z");
    expect(formatted).toMatch(/^\d{2}:\d{2}:\d{2}\.\d{3}$/);
  });

  it("脏时间戳原样返回而不是抛错", () => {
    expect(formatClock("not-a-time")).toBe("not-a-time");
    expect(formatDateTime("not-a-time")).toBe("not-a-time");
  });

  it("状态与退出原因有中文标签", () => {
    expect(runStateLabel("running")).toBe("运行中");
    expect(exitReasonLabel("killed")).toBe("被停止");
    expect(describeExit(null, "crashed")).toContain("无退出码");
    expect(describeStatus("exited", null, null)).toBe("状态 已退出");
  });

  it("命令行拼接对含空格的参数加引号", () => {
    expect(renderCommand("python", ["main.py", "--port", "9880"])).toBe(
      "python main.py --port 9880"
    );
    expect(renderCommand("python", ["--prompt", "hello world"])).toBe(
      'python --prompt "hello world"'
    );
    expect(renderCommand("node", undefined)).toBe("node");
  });

  it("占位头像取首字符且不切坏 emoji", () => {
    expect(initialOf("gpt-sovits")).toBe("G");
    expect(initialOf("  演示 ")).toBe("演");
    expect(initialOf("")).toBe("?");
    expect(initialOf("🎯target")).toBe("🎯");
  });

  it("not_found 判定为「会话已不在管理器」", () => {
    expect(isSessionGone({ kind: "not_found", message: "未找到会话" })).toBe(true);
    expect(isSessionGone({ kind: "io", message: "磁盘错误" })).toBe(false);
  });
});
