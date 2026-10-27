/**
 * updater 客户端的单元测试：mock 掉 Tauri 插件与宿主判定，只验证封装逻辑
 * （支持判定的门卫、Update 句柄的存活、进度事件映射、错误归一）。
 */
import { afterEach, describe, expect, it, vi } from "vitest";

import { check } from "@tauri-apps/plugin-updater";
import {
  UPDATER_ENDPOINT,
  backgroundCheckForUpdate,
  checkForUpdate,
  describeUpdaterError,
  updaterSupported,
} from "./updater";

vi.mock("@tauri-apps/plugin-updater", () => ({
  check: vi.fn(),
}));

vi.mock("./api", () => ({
  isHosted: () => true,
}));

const checkMock = vi.mocked(check);

/** 造一个最小的插件 Update 桩：记录 downloadAndInstall 收到的进度回调。 */
function fakeUpdate(version: string) {
  return {
    version,
    body: null,
    downloadAndInstall: vi.fn(async (onEvent: (event: never) => void) => {
      onEvent({ event: "Started", data: { contentLength: 100 } } as never);
      onEvent({ event: "Progress", data: { chunkLength: 40 } } as never);
      onEvent({ event: "Finished" } as never);
    }),
  };
}

afterEach(() => {
  vi.restoreAllMocks();
  vi.unstubAllEnvs();
});

describe("updaterSupported", () => {
  it("开发模式（vite dev server）下禁用", () => {
    // vitest 默认 DEV=true，等价于 tauri dev 的前端环境
    vi.stubEnv("DEV", true);
    expect(updaterSupported()).toBe(false);
  });

  it("打包环境 + 宿主内可用", () => {
    vi.stubEnv("DEV", false);
    expect(updaterSupported()).toBe(true);
  });
});

describe("checkForUpdate", () => {
  it("没有新版本返回 null", async () => {
    checkMock.mockResolvedValue(null);
    await expect(checkForUpdate()).resolves.toBeNull();
  });

  it("拿到句柄：元信息可读，downloadAndInstall 把插件事件映射成三种回调", async () => {
    const update = fakeUpdate("0.2.0");
    checkMock.mockResolvedValue(update as never);

    const pending = await checkForUpdate();
    expect(pending?.info.version).toBe("0.2.0");

    const events: string[] = [];
    await pending?.downloadAndInstall((event) => {
      events.push(
        event.kind === "started"
          ? `started:${event.totalBytes}`
          : event.kind === "chunk"
            ? `chunk:${event.bytes}`
            : "finished"
      );
    });
    expect(events).toEqual(["started:100", "chunk:40", "finished"]);
    expect(update.downloadAndInstall).toHaveBeenCalledOnce();
  });
});

describe("backgroundCheckForUpdate", () => {
  it("开发模式下直接返回 null，不发起任何请求", async () => {
    vi.stubEnv("DEV", true);
    checkMock.mockReset();
    await expect(backgroundCheckForUpdate()).resolves.toBeNull();
    expect(checkMock).not.toHaveBeenCalled();
  });

  it("发现新版本返回版本号；失败静默返回 null", async () => {
    vi.stubEnv("DEV", false);
    checkMock.mockResolvedValue(fakeUpdate("0.3.0") as never);
    await expect(backgroundCheckForUpdate()).resolves.toBe("0.3.0");

    checkMock.mockRejectedValue(new Error("network error"));
    await expect(backgroundCheckForUpdate()).resolves.toBeNull();
  });
});

describe("describeUpdaterError", () => {
  it("常见错误归一成可执行的中文提示", () => {
    expect(describeUpdaterError(new Error("fetch failed: network"))).toContain("网络");
    expect(describeUpdaterError(new Error("signature verification failed"))).toContain(
      "签名"
    );
    expect(describeUpdaterError(new Error("磁盘空间不足"))).toContain("磁盘空间不足");
    expect(describeUpdaterError("随便给的字符串")).toContain("随便给的字符串");
    expect(describeUpdaterError({ odd: true })).toContain("未知原因");
  });
});

describe("UPDATER_ENDPOINT", () => {
  it("指向本仓库 GitHub Releases 的静态清单", () => {
    expect(UPDATER_ENDPOINT).toMatch(
      /^https:\/\/github\.com\/BUGLAN\/app_catch\/releases\/latest\/download\/latest\.json$/
    );
  });
});
