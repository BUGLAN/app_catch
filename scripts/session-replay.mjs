#!/usr/bin/env node
/**
 * 基线验收复原器：只读会话目录里的文件，回答三个问题。
 *
 * 需求基线：`<harness 契约>` 要求「只凭会话目录里的文件，agent 能完整还原
 * 跑了什么 / 何时 / 输出什么 / 怎么结束的」。
 *
 * 本脚本是这条标准的**自证工具**：它只接受一个会话目录路径，
 * 除了该目录内的 events*.jsonl 不再读取任何东西——
 * 不读 `apps/<id>.toml`、不读进程表、不连任何服务。
 * 因此它打印出来的内容，就是「只凭会话文件能知道的一切」。
 *
 * 用法：
 *   node scripts/session-replay.mjs <会话目录>
 *   node scripts/session-replay.mjs <home>/sessions/<session_id> --json
 *
 * 退出码：0 复原成功；1 目录不存在或没有任何可解析事件。
 */
import { readdirSync, readFileSync, statSync } from "node:fs";
import { join } from "node:path";

const args = process.argv.slice(2);
const jsonOnly = args.includes("--json");
const dir = args.find((a) => !a.startsWith("--"));

if (!dir) {
  console.error("用法: node scripts/session-replay.mjs <会话目录> [--json]");
  process.exit(1);
}

try {
  if (!statSync(dir).isDirectory()) throw new Error("不是目录");
} catch (error) {
  console.error("会话目录不可用:", dir, String(error));
  process.exit(1);
}

/**
 * 按「分片 seq 升序 -> 活跃片最后」收集日志文件。
 *
 * 这条顺序是会话格式的一部分（见 docs/SESSION_FORMAT.md）：
 * events.<seq>.jsonl 是轮转出的历史片，events.jsonl 是当前活跃片。
 * 复原器必须照此排序，否则会把时间顺序读反。
 */
function orderedLogFiles(sessionDir) {
  const shards = [];
  let active = null;
  for (const name of readdirSync(sessionDir)) {
    if (name === "events.jsonl") {
      active = join(sessionDir, name);
      continue;
    }
    const match = /^events\.(\d+)\.jsonl$/.exec(name);
    if (match) shards.push([Number(match[1]), join(sessionDir, name)]);
  }
  shards.sort((a, b) => a[0] - b[0]);
  const files = shards.map(([, path]) => path);
  if (active) files.push(active);
  return files;
}

const files = orderedLogFiles(dir);
const events = [];
let skipped = 0;
for (const file of files) {
  const text = readFileSync(file, "utf8");
  for (const line of text.split(/\r?\n/)) {
    if (!line.trim()) continue;
    try {
      const event = JSON.parse(line);
      // start_time 是 Windows FILETIME（u64），超过 JS 安全整数范围（2^53-1），
      // 直接 JSON.parse 会丢精度（实测 …441 会变成 …440）。
      // 复原器必须给出**精确**身份值，因此这里从原始行里把数字抠出来覆盖。
      if (event && event.event === "start") {
        const match = /"start_time"\s*:\s*(\d+)/.exec(line);
        if (match) event.start_time = match[1];
      }
      events.push(event);
    } catch {
      // 启动器被强杀可能留下半行：如实计数，不静默吞掉
      skipped += 1;
    }
  }
}

if (events.length === 0) {
  console.error("会话目录里没有可解析事件:", dir);
  process.exit(1);
}

const start = events.find((e) => e.event === "start");
const exit = events.find((e) => e.event === "exit");
const outputs = events.filter((e) => e.event === "output");
const statuses = events.filter((e) => e.event === "status");
const overflows = events.filter((e) => e.event === "overflow");

/** 从 status 事件里汇总端口与窗口标题（去重，保留出现顺序）。 */
function collect(field) {
  const seen = new Set();
  const values = [];
  for (const status of statuses) {
    const raw = status[field];
    if (raw === null || raw === undefined) continue;
    const list = Array.isArray(raw) ? raw : [raw];
    for (const value of list) {
      if (seen.has(value)) continue;
      seen.add(value);
      values.push(value);
    }
  }
  return values;
}

/** RFC3339 -> 可读 UTC 时间（带毫秒），保留原始字符串便于核对。 */
function readable(ts) {
  if (!ts) return "(无)";
  const date = new Date(ts);
  return Number.isNaN(date.getTime()) ? ts : date.toISOString();
}

const stderrCount = outputs.filter((o) => o.stream === "stderr").length;
const encodings = [...new Set(outputs.map((o) => o.encoding ?? "utf-8"))];

const report = {
  会话目录: dir,
  日志文件: files.map((f) => f.split(/[\\/]/).pop()),
  事件统计: {
    总数: events.length,
    不可解析行: skipped,
    start: start ? 1 : 0,
    output: outputs.length,
    stderr: stderrCount,
    status: statuses.length,
    exit: exit ? 1 : 0,
    overflow: overflows.length,
    丢弃行数: overflows.reduce((sum, o) => sum + (o.dropped ?? 0), 0),
    出现过的编码: encodings,
  },
  跑了什么: start
    ? {
        app_id: start.app_id,
        cmd: start.cmd,
        argv: start.argv,
        cwd: start.cwd,
        pid: start.pid,
        ppid: start.ppid,
        start_time_filetime: start.start_time,
      }
    : null,
  何时: {
    启动时间_utc: start ? readable(start.ts) : null,
    启动时间原始: start ? start.ts : null,
    结束时间_utc: exit ? readable(exit.ts) : null,
    结束时间原始: exit ? exit.ts : null,
    运行时长秒:
      start && exit
        ? Number(((new Date(exit.ts) - new Date(start.ts)) / 1000).toFixed(3))
        : null,
  },
  输出什么: {
    行数: outputs.length,
    首行: outputs[0]?.line ?? null,
    末行: outputs[outputs.length - 1]?.line ?? null,
    采样: outputs.slice(0, 5).map((o) => ({ ts: o.ts, stream: o.stream, line: o.line })),
  },
  状态: {
    监听端口: collect("ports"),
    窗口标题: collect("window_title"),
    状态序列: statuses.map((s) => s.state),
  },
  怎么结束的: exit
    ? { 退出码: exit.code ?? null, 原因: exit.reason, 结束时间_utc: readable(exit.ts) }
    : { 说明: "会话仍记录为运行中（无 exit 事件）" },
};

if (jsonOnly) {
  console.log(JSON.stringify(report, null, 2));
  process.exit(0);
}

console.log("===== 只读会话目录复原结果 =====");
console.log("会话目录   :", report.会话目录);
console.log("日志文件   :", report.日志文件.join(", "));
console.log("事件统计   :", JSON.stringify(report.事件统计));
console.log();
console.log("【1】跑了什么");
if (report.跑了什么) {
  const s = report.跑了什么;
  console.log("    app_id :", s.app_id);
  console.log("    命令   :", s.cmd);
  console.log("    argv   :", JSON.stringify(s.argv));
  console.log("    工作目录:", s.cwd);
  console.log("    pid/ppid:", s.pid + " / " + s.ppid, "(start_time = " + s.start_time_filetime + ")");
} else {
  console.log("    （缺少 start 事件）");
}
console.log();
console.log("【2】何时");
console.log("    启动:", report.何时.启动时间_utc, "(" + report.何时.启动时间原始 + ")");
console.log("    结束:", report.何时.结束时间_utc, "(" + report.何时.结束时间原始 + ")");
console.log("    时长:", report.何时.运行时长秒, "秒");
console.log();
console.log("【3】输出什么");
console.log("    行数:", report.输出什么.行数);
console.log("    首行:", report.输出什么.首行);
console.log("    末行:", report.输出什么.末行);
for (const sample of report.输出什么.采样) {
  console.log("    -", sample.ts, "[" + sample.stream + "]", sample.line);
}
console.log();
console.log("【4】运行期状态");
console.log("    监听端口:", JSON.stringify(report.状态.监听端口));
console.log("    窗口标题:", JSON.stringify(report.状态.窗口标题));
console.log("    状态序列:", report.状态.状态序列.join(" -> ") || "(无)");
console.log();
console.log("【5】怎么结束的");
console.log("   ", JSON.stringify(report.怎么结束的));
