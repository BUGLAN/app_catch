/**
 * merge-updater-manifests 的单元测试：用临时目录伪造各平台的 .sig，
 * 验证平台映射、安装包优先级与 URL 构造，不依赖真实构建产物。
 */
import { mkdtemp, mkdir, writeFile, rm } from "node:fs/promises";
import { tmpdir } from "node:os";
import { join } from "node:path";
import { afterEach, describe, expect, it } from "vitest";

import { mergeUpdaterManifests } from "./merge-updater-manifests.mjs";

/** 每个用例的临时根目录（afterEach 清理）。 */
let root;

/**
 * 造输入目录：spec = { "<rust-target>": ["文件名.sig", ...] }。
 * 签名内容统一为 "SIG:<文件名>"，断言时容易对号。
 */
async function makeInputs(spec) {
  root = await mkdtemp(join(tmpdir(), "updater-manifest-"));
  for (const [target, sigs] of Object.entries(spec)) {
    const dir = join(root, `updater-${target}`);
    await mkdir(dir, { recursive: true });
    for (const sig of sigs) {
      await writeFile(join(dir, sig), `SIG:${sig}\n`, "utf8");
    }
  }
  return root;
}

afterEach(async () => {
  if (root) {
    await rm(root, { recursive: true, force: true });
    root = undefined;
  }
});

const BASE = {
  repo: "BUGLAN/app_catch",
  tag: "v0.2.0",
  version: "0.2.0",
  notes: "修复了一些问题",
  now: () => "2026-10-26T00:00:00.000Z",
};

describe("mergeUpdaterManifests", () => {
  it("三个平台的目录名映射成 OS-ARCH 键，URL 指向本仓库 Release 资产", async () => {
    const inputs = await makeInputs({
      "aarch64-apple-darwin": ["app_catch.app.tar.gz.sig"],
      "x86_64-apple-darwin": ["app_catch.app.tar.gz.sig"],
      "x86_64-pc-windows-msvc": [
        "app_catch_0.2.0_x64-setup.exe.sig",
        "app_catch_0.2.0_x64_en-US.msi.sig",
      ],
    });

    const manifest = await mergeUpdaterManifests({ ...BASE, inputsDir: inputs });

    expect(Object.keys(manifest.platforms).sort()).toEqual([
      "darwin-aarch64",
      "darwin-x86_64",
      "windows-x86_64",
    ]);
    // Windows 优先用 NSIS 的 -setup.exe（.msi 只是备选）
    expect(manifest.platforms["windows-x86_64"]).toEqual({
      signature: "SIG:app_catch_0.2.0_x64-setup.exe.sig",
      url: "https://github.com/BUGLAN/app_catch/releases/download/v0.2.0/app_catch_0.2.0_x64-setup.exe",
    });
    expect(manifest.platforms["darwin-aarch64"].url).toBe(
      "https://github.com/BUGLAN/app_catch/releases/download/v0.2.0/app_catch.app.tar.gz"
    );
    expect(manifest.version).toBe("0.2.0");
    expect(manifest.notes).toBe("修复了一些问题");
    expect(manifest.pub_date).toBe("2026-10-26T00:00:00.000Z");
  });

  it("只有 .msi 时也能生成清单（退而求其次）", async () => {
    const inputs = await makeInputs({
      "x86_64-pc-windows-msvc": ["app_catch_0.2.0_x64_en-US.msi.sig"],
    });
    const manifest = await mergeUpdaterManifests({ ...BASE, inputsDir: inputs });
    expect(manifest.platforms["windows-x86_64"].url).toMatch(/\.msi$/);
  });

  it("签名内容去掉首尾空白", async () => {
    root = await mkdtemp(join(tmpdir(), "updater-manifest-"));
    const dir = join(root, "updater-x86_64-pc-windows-msvc");
    await mkdir(dir, { recursive: true });
    await writeFile(join(dir, "app-setup.exe.sig"), "  \n SIG:abc \n\n", "utf8");
    const manifest = await mergeUpdaterManifests({ ...BASE, inputsDir: root });
    expect(manifest.platforms["windows-x86_64"].signature).toBe("SIG:abc");
  });

  it("资产名里的空格会被转义，其余字符保持原样", async () => {
    const inputs = await makeInputs({
      "x86_64-apple-darwin": ["app catch.app.tar.gz.sig"],
    });
    const manifest = await mergeUpdaterManifests({ ...BASE, inputsDir: inputs });
    expect(manifest.platforms["darwin-x86_64"].url).toBe(
      "https://github.com/BUGLAN/app_catch/releases/download/v0.2.0/app%20catch.app.tar.gz"
    );
  });

  it("没有 updater-* 目录时给出可读错误", async () => {
    root = await mkdtemp(join(tmpdir(), "updater-manifest-"));
    await expect(mergeUpdaterManifests({ ...BASE, inputsDir: root })).rejects.toThrow(
      /没有找到任何 updater-/
    );
  });

  it("目录里的 .sig 为空文件时给出可读错误", async () => {
    root = await mkdtemp(join(tmpdir(), "updater-manifest-"));
    const dir = join(root, "updater-x86_64-apple-darwin");
    await mkdir(dir, { recursive: true });
    await writeFile(join(dir, "app_catch.app.tar.gz.sig"), "", "utf8");
    await expect(mergeUpdaterManifests({ ...BASE, inputsDir: root })).rejects.toThrow(
      /签名为空|签名内容为空/
    );
  });

  it("未知构建目标直接失败（防止 target 改名后清单静默缺平台）", async () => {
    const inputs = await makeInputs({
      "riscv64-unknown-none": ["app.sig"],
    });
    await expect(mergeUpdaterManifests({ ...BASE, inputsDir: inputs })).rejects.toThrow(
      /未知构建目标/
    );
  });
});
