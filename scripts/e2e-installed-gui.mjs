#!/usr/bin/env node
/**
 * 安装版应用的 GUI 端到端驱动（CDP）。
 *
 * 为什么需要它：M5 的验收要点是「用打包后的安装版应用跑一遍闭环」，
 * 而安装版没有 dev 工具链可用。WebView2 自带 Chrome DevTools Protocol，
 * 只要在启动安装版时注入 WEBVIEW2_ADDITIONAL_BROWSER_ARGUMENTS=--remote-debugging-port=<port>，
 * 就能用本脚本在真实 UI 上点击「启动 / 停止」，走的是和人工操作完全一样的命令层路径。
 *
 * 用法（先启动安装版，见 docs/E2E.md）：
 *   node scripts/e2e-installed-gui.mjs --port 9223 --app charmap --timeout 30
 *
 * 参数：
 *   --port    CDP 端口（默认 9223）
 *   --app     要点击「启动」的应用名称关键字（必填，匹配列表里的应用名）
 *   --timeout 等待会话出现的秒数（默认 30）
 *   --hold    启动后保持运行的秒数，默认 8（让状态采集抓到窗口标题/端口）
 *
 * 退出码：0 全部成功；1 任一步骤失败（原因打到 stderr）。
 */

const args = process.argv.slice(2);
function arg(name, fallback) {
  const index = args.indexOf("--" + name);
  if (index === -1) return fallback;
  const value = args[index + 1];
  return value === undefined ? fallback : value;
}

const port = Number(arg("port", "9223"));
const appName = arg("app", "");
const timeoutSec = Number(arg("timeout", "30"));
const holdSec = Number(arg("hold", "8"));

if (!appName) {
  console.error("必须提供 --app <应用名称关键字>");
  process.exit(1);
}

const sleep = (ms) => new Promise((resolve) => setTimeout(resolve, ms));

/** 取页面 target 的 websocket 地址。 */
async function findPageTarget() {
  const deadline = Date.now() + timeoutSec * 1000;
  while (Date.now() < deadline) {
    try {
      const response = await fetch("http://127.0.0.1:" + port + "/json");
      const targets = await response.json();
      const page = targets.find((t) => t.type === "page");
      if (page && page.webSocketDebuggerUrl) return page;
    } catch {
      // 端口还没起来：继续等
    }
    await sleep(300);
  }
  throw new Error(timeoutSec + "s 内未连上 CDP 端口 " + port);
}

/** 极简 CDP 客户端：只需要 Runtime.evaluate。 */
class Cdp {
  #ws;
  #nextId = 1;
  #pending = new Map();

  constructor(ws) {
    this.#ws = ws;
    ws.addEventListener("message", (event) => {
      const message = JSON.parse(event.data);
      const resolver = this.#pending.get(message.id);
      if (resolver) {
        this.#pending.delete(message.id);
        resolver(message);
      }
    });
  }

  static async connect(url) {
    const ws = new WebSocket(url);
    await new Promise((resolve, reject) => {
      ws.addEventListener("open", resolve, { once: true });
      ws.addEventListener("error", () => reject(new Error("CDP websocket 连接失败")), {
        once: true,
      });
    });
    return new Cdp(ws);
  }

  send(method, params = {}) {
    const id = this.#nextId++;
    return new Promise((resolve, reject) => {
      this.#pending.set(id, (message) => {
        if (message.error) reject(new Error(method + " 失败: " + message.error.message));
        else resolve(message.result);
      });
      this.#ws.send(JSON.stringify({ id, method, params }));
    });
  }

  /** 在页面里求值；返回可序列化的 JSON。 */
  async evaluate(expression) {
    const result = await this.send("Runtime.evaluate", {
      expression,
      awaitPromise: true,
      returnByValue: true,
    });
    if (result.exceptionDetails) {
      const description =
        (result.exceptionDetails.exception && result.exceptionDetails.exception.description) ||
        "unknown";
      throw new Error("页面求值异常: " + description);
    }
    return result.result ? result.result.value : null;
  }

  close() {
    this.#ws.close();
  }
}

/** 页面取证脚本：从真实 DOM 里读应用列表与状态徽标。 */
const PAGE_STATE = `(() => {
  const items = Array.from(document.querySelectorAll(".app-item")).map((el) => {
    const name = el.querySelector(".app-item-name");
    const badge = el.querySelector(".badge");
    return {
      name: name ? name.textContent : "",
      badge: badge ? badge.textContent : "",
      selected: el.classList.contains("is-selected"),
    };
  });
  const titleEl = document.querySelector(".panel-title");
  return {
    items,
    title: titleEl ? titleEl.textContent : null,
  };
})()`;

/** 点击某个应用卡片上的「启动」按钮。 */
function clickStartExpr(name) {
  return `(() => {
    const items = Array.from(document.querySelectorAll(".app-item"));
    const needle = ${JSON.stringify(name)};
    const target = items.find((el) => {
      const n = el.querySelector(".app-item-name");
      return n && n.textContent.includes(needle);
    });
    if (!target) return { ok: false, reason: "未找到应用卡片" };
    const button = Array.from(target.querySelectorAll("button")).find((b) =>
      b.textContent.includes("启动")
    );
    if (!button) return { ok: false, reason: "未找到启动按钮" };
    button.click();
    return { ok: true };
  })()`;
}

/** 在详情面板点击「停止」（停止最近启动的运行中会话）。 */
const CLICK_STOP = `(() => {
  const buttons = Array.from(document.querySelectorAll(".panel-actions button"));
  const stop = buttons.find((b) => b.textContent.trim() === "停止");
  if (!stop) return { ok: false, reason: "未找到停止按钮" };
  if (stop.disabled) return { ok: false, reason: "停止按钮不可用（没有运行中会话）" };
  stop.click();
  return { ok: true };
})()`;

async function main() {
  const target = await findPageTarget();
  console.log("CDP 已连接: " + target.url + " （" + target.title + "）");
  const cdp = await Cdp.connect(target.webSocketDebuggerUrl);
  await cdp.send("Runtime.enable");

  const boot = await cdp.evaluate("document.title");
  console.log("页面标题: " + boot);

  // 等首屏渲染（前端先 invoke list_apps）
  const deadline = Date.now() + timeoutSec * 1000;
  let state = null;
  while (Date.now() < deadline) {
    state = await cdp.evaluate(PAGE_STATE);
    if (state && state.items && state.items.length) break;
    await sleep(300);
  }
  if (!state || !state.items || !state.items.length) {
    throw new Error("应用列表为空：安装版没读到 APP_CATCH_HOME 下的配置");
  }
  console.log("应用列表: " + JSON.stringify(state.items));

  const clicked = await cdp.evaluate(clickStartExpr(appName));
  if (!clicked || !clicked.ok) throw new Error("点击启动失败: " + (clicked && clicked.reason));
  console.log("已点击「启动」: " + appName);

  // 等状态徽标变成「运行中」
  let running = null;
  const runDeadline = Date.now() + timeoutSec * 1000;
  while (Date.now() < runDeadline) {
    const now = await cdp.evaluate(PAGE_STATE);
    const item = now.items.find((i) => i.badge.trim() === "运行中");
    if (item) {
      running = now;
      break;
    }
    await sleep(300);
  }
  if (!running) throw new Error("等待「运行中」状态超时");
  console.log("运行中状态: " + JSON.stringify(running.items));

  // 保持一段时间，让状态采集循环抓到窗口标题/端口
  await sleep(holdSec * 1000);
  const afterHold = await cdp.evaluate(PAGE_STATE);
  console.log("保持 " + holdSec + "s 后页面状态: " + JSON.stringify(afterHold.items));

  const stopped = await cdp.evaluate(CLICK_STOP);
  if (!stopped || !stopped.ok) throw new Error("点击停止失败: " + (stopped && stopped.reason));
  console.log("已点击「停止」");

  // 等徽标回落（不再是「运行中」）
  let fell = false;
  const stopDeadline = Date.now() + timeoutSec * 1000;
  while (Date.now() < stopDeadline) {
    const now = await cdp.evaluate(PAGE_STATE);
    if (now.items.every((i) => i.badge.trim() !== "运行中")) {
      console.log("停止后页面状态: " + JSON.stringify(now.items));
      fell = true;
      break;
    }
    await sleep(300);
  }
  if (!fell) throw new Error("停止后状态未回落");

  cdp.close();
  console.log("GUI 闭环驱动完成");
}

main().catch((error) => {
  console.error("E2E 失败:", error.message);
  process.exit(1);
});
