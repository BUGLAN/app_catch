/**
 * 更新状态机的单元测试：所有状态迁移都在纯函数上验证，不碰 Tauri IPC。
 */
import { describe, expect, it } from "vitest";

import {
  INITIAL_UPDATE_STATE,
  beginCheck,
  beginDownload,
  describeUpdateState,
  downloadProgress,
  failUpdate,
  finishCheck,
  finishDownload,
  formatBytes,
  progressPercent,
  seedAvailable,
} from "./updaterModel";

describe("beginCheck / finishCheck", () => {
  it("从任意状态进入检查中，并清空上一次结果", () => {
    const stale = failUpdate(INITIAL_UPDATE_STATE, "上次的错误");
    const checking = beginCheck(stale);
    expect(checking.phase).toBe("checking");
    expect(checking.error).toBeNull();
    expect(checking.nextVersion).toBeNull();
  });

  it("检查到新版本进入 available 并保留版本号", () => {
    const state = finishCheck(beginCheck(INITIAL_UPDATE_STATE), "0.2.0");
    expect(state.phase).toBe("available");
    expect(state.nextVersion).toBe("0.2.0");
  });

  it("没有新版本进入 up-to-date，且不残留版本号", () => {
    const state = finishCheck(beginCheck(INITIAL_UPDATE_STATE), null);
    expect(state.phase).toBe("up-to-date");
    expect(state.nextVersion).toBeNull();
  });

  it("乱序回调不能覆盖后续阶段（available 不会被迟到的 finishCheck 打断）", () => {
    const available = finishCheck(beginCheck(INITIAL_UPDATE_STATE), "0.2.0");
    const downloading = beginDownload(available);
    expect(finishCheck(downloading, "9.9.9")).toBe(downloading);
  });
});

describe("downloadProgress / finishDownload", () => {
  it("累加增量字节并计算百分比", () => {
    let state = beginDownload(finishCheck(beginCheck(INITIAL_UPDATE_STATE), "0.2.0"));
    state = downloadProgress(state, 0, 200);
    state = downloadProgress(state, 80, null);
    state = downloadProgress(state, 20, null);
    expect(state.downloadedBytes).toBe(100);
    expect(state.totalBytes).toBe(200);
    expect(progressPercent(state)).toBe(50);
  });

  it("拿不到总长时百分比是 null（界面渲染不确定态进度条）", () => {
    let state = beginDownload(INITIAL_UPDATE_STATE);
    state = downloadProgress(state, 40, null);
    expect(state.totalBytes).toBeNull();
    expect(progressPercent(state)).toBeNull();
  });

  it("已下载超出服务器给的总长时按实际值修正（总长只缩不涨）", () => {
    let state = beginDownload(INITIAL_UPDATE_STATE);
    state = downloadProgress(state, 0, 100);
    state = downloadProgress(state, 150, 300);
    expect(state.totalBytes).toBe(150);
    expect(progressPercent(state)).toBe(100);
  });

  it("下载完成进入 ready；非 downloading 阶段忽略", () => {
    expect(finishDownload(INITIAL_UPDATE_STATE)).toBe(INITIAL_UPDATE_STATE);
    const state = finishDownload(beginDownload(INITIAL_UPDATE_STATE));
    expect(state.phase).toBe("ready");
  });
});

describe("failUpdate / seedAvailable", () => {
  it("失败保留中文原因；空原因给兜底文案", () => {
    expect(failUpdate(INITIAL_UPDATE_STATE, "网络请求失败").error).toBe("网络请求失败");
    expect(failUpdate(INITIAL_UPDATE_STATE, "").error).toBe("更新失败（未知原因）");
  });

  it("播种只在 idle 时生效，避免覆盖用户刚触发的检查", () => {
    const seeded = seedAvailable(INITIAL_UPDATE_STATE, "0.3.0");
    expect(seeded.phase).toBe("available");
    expect(seeded.nextVersion).toBe("0.3.0");

    const checking = beginCheck(INITIAL_UPDATE_STATE);
    expect(seedAvailable(checking, "0.3.0")).toBe(checking);
  });
});

describe("formatBytes / describeUpdateState", () => {
  it("字节数格式化贴近阅读习惯", () => {
    expect(formatBytes(0)).toBe("0 B");
    expect(formatBytes(512)).toBe("512 B");
    expect(formatBytes(2048)).toBe("2 KB");
    expect(formatBytes(5 * 1024 * 1024)).toBe("5 MB");
  });

  it("每个阶段都有一句可直接展示的中文描述", () => {
    expect(describeUpdateState(INITIAL_UPDATE_STATE)).toContain("尚未检查");
    expect(describeUpdateState(beginCheck(INITIAL_UPDATE_STATE))).toContain("正在检查");
    expect(
      describeUpdateState(finishCheck(beginCheck(INITIAL_UPDATE_STATE), null))
    ).toContain("已是最新");
    expect(
      describeUpdateState(finishCheck(beginCheck(INITIAL_UPDATE_STATE), "0.2.0"))
    ).toContain("0.2.0");
    expect(describeUpdateState(finishDownload(beginDownload(INITIAL_UPDATE_STATE)))).toContain(
      "重启"
    );
    expect(describeUpdateState(failUpdate(INITIAL_UPDATE_STATE, "签名失败"))).toContain(
      "签名失败"
    );
  });

  it("下载中的描述包含进度数字", () => {
    let state = beginDownload(INITIAL_UPDATE_STATE);
    state = downloadProgress(state, 0, 1000);
    state = downloadProgress(state, 250, null);
    const text = describeUpdateState(state);
    expect(text).toContain("25%");
    expect(text).toContain("250 B");
  });
});
