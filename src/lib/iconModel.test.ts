/**
 * 图标模型的单测：会话内缓存、并发合并、失败静默降级、失效重取。
 *
 * 这些用例刻意不连真实 Tauri 宿主：\`api\` 被整体 mock 掉，
 * 因为要验证的是「请求策略」而不是 Win32 抓取（后者在 Rust 侧测）。
 */
import { beforeEach, describe, expect, it, vi } from "vitest";

const getAppIcon = vi.fn<(appId: string, force?: boolean) => Promise<string | null>>();
const defaultIcon = vi.fn<() => Promise<string>>();

vi.mock("./api", () => ({
  api: {
    getAppIcon: (appId: string, force?: boolean) => getAppIcon(appId, force),
    defaultIcon: () => defaultIcon(),
  },
}));

// mock 必须在 import 之后才生效，因此这里用动态导入
const {
  loadIcon,
  loadDefaultIcon,
  invalidateIcon,
  resetIconCache,
  seedIcon,
} = await import("./iconModel");

const PNG_URL = "data:image/png;base64,iVBORw0KGgoAAAANSUhEUg==";

describe("loadIcon", () => {
  beforeEach(() => {
    resetIconCache();
    getAppIcon.mockReset();
    defaultIcon.mockReset();
  });

  it("把 data URL 原样返回", async () => {
    getAppIcon.mockResolvedValue(PNG_URL);
    await expect(loadIcon("demo")).resolves.toBe(PNG_URL);
    expect(getAppIcon).toHaveBeenCalledWith("demo", false);
  });

  it("同一 app_id 二次调用命中会话内缓存，不再请求", async () => {
    getAppIcon.mockResolvedValue(PNG_URL);
    await loadIcon("demo");
    await loadIcon("demo");
    await loadIcon("demo");
    expect(getAppIcon).toHaveBeenCalledTimes(1);
  });

  it("并发请求同一个 app_id 只发一次 IPC", async () => {
    getAppIcon.mockResolvedValue(PNG_URL);
    const [a, b, c] = await Promise.all([
      loadIcon("demo"),
      loadIcon("demo"),
      loadIcon("demo"),
    ]);
    expect(a).toBe(PNG_URL);
    expect(b).toBe(PNG_URL);
    expect(c).toBe(PNG_URL);
    expect(getAppIcon).toHaveBeenCalledTimes(1);
  });

  it("后端返回 null 时归一成 null，并缓存「确实没有图标」", async () => {
    getAppIcon.mockResolvedValue(null);
    await expect(loadIcon("empty")).resolves.toBeNull();
    await expect(loadIcon("empty")).resolves.toBeNull();
    expect(getAppIcon).toHaveBeenCalledTimes(1);
  });

  it("后端返回空串也视为没有图标", async () => {
    getAppIcon.mockResolvedValue("");
    await expect(loadIcon("blank")).resolves.toBeNull();
  });

  it("请求失败静默降级成 null，不抛错（图标失败绝不能打扰用户）", async () => {
    getAppIcon.mockRejectedValue(new Error("IPC 挂了"));
    await expect(loadIcon("broken")).resolves.toBeNull();
  });

  it("失败不写缓存：下次仍可重试", async () => {
    getAppIcon.mockRejectedValueOnce(new Error("临时失败"));
    await expect(loadIcon("flaky")).resolves.toBeNull();

    getAppIcon.mockResolvedValueOnce(PNG_URL);
    await expect(loadIcon("flaky")).resolves.toBe(PNG_URL);
    expect(getAppIcon).toHaveBeenCalledTimes(2);
  });

  it("invalidateIcon 后下次会重新抓（用于会话启动后拿运行时图标）", async () => {
    getAppIcon.mockResolvedValue(null);
    await loadIcon("demo");
    expect(getAppIcon).toHaveBeenCalledTimes(1);

    invalidateIcon("demo");
    getAppIcon.mockResolvedValue(PNG_URL);
    await expect(loadIcon("demo")).resolves.toBe(PNG_URL);
    expect(getAppIcon).toHaveBeenCalledTimes(2);
  });

  it("force=true 绕过内存缓存，真的重新 invoke", async () => {
    getAppIcon.mockResolvedValue(PNG_URL);
    await loadIcon("forced");
    expect(getAppIcon).toHaveBeenCalledTimes(1);

    // 普通调用：命中内存缓存
    await loadIcon("forced");
    expect(getAppIcon).toHaveBeenCalledTimes(1);

    // force：绕过内存缓存，并把 force 透传给命令层
    const other = "data:image/png;base64,AAAA";
    getAppIcon.mockResolvedValue(other);
    await expect(loadIcon("forced", true)).resolves.toBe(other);
    expect(getAppIcon).toHaveBeenCalledTimes(2);
    expect(getAppIcon).toHaveBeenLastCalledWith("forced", true);
  });

  it("force=true 的结果会覆盖缓存，后续普通调用拿到新值", async () => {
    getAppIcon.mockResolvedValue("data:image/png;base64,OLD");
    await loadIcon("upgrade");
    getAppIcon.mockResolvedValue("data:image/png;base64,NEW");
    await loadIcon("upgrade", true);
    // 再普通调用一次：应直接命中已更新的内存缓存，不再 IPC
    expect(getAppIcon).toHaveBeenCalledTimes(2);
    await expect(loadIcon("upgrade")).resolves.toBe("data:image/png;base64,NEW");
    expect(getAppIcon).toHaveBeenCalledTimes(2);
  });

  it("seedIcon 可以预置结果，跳过 IPC", async () => {
    seedIcon("seeded", PNG_URL);
    await expect(loadIcon("seeded")).resolves.toBe(PNG_URL);
    expect(getAppIcon).not.toHaveBeenCalled();
  });
});

describe("loadDefaultIcon", () => {
  beforeEach(() => {
    resetIconCache();
    getAppIcon.mockReset();
    defaultIcon.mockReset();
  });

  it("默认图全局只取一次", async () => {
    defaultIcon.mockResolvedValue(PNG_URL);
    await expect(loadDefaultIcon()).resolves.toBe(PNG_URL);
    await expect(loadDefaultIcon()).resolves.toBe(PNG_URL);
    expect(defaultIcon).toHaveBeenCalledTimes(1);
  });

  it("默认图也失败时返回空串（界面退回首字块，不崩）", async () => {
    defaultIcon.mockRejectedValue(new Error("非 Tauri 环境"));
    await expect(loadDefaultIcon()).resolves.toBe("");
  });
});