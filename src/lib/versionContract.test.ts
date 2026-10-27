/**
 * 版本与更新端点的「跨文件契约」测试。
 *
 * 背景：版本号天然存在多份拷贝——package.json（npm 侧）、
 * src-tauri/tauri.conf.json（打包器命名安装包）、根 Cargo.toml（workspace）。
 * 任何一份单独改都会造成「关于页显示 0.1.0、安装包却叫 0.2.0」的错位。
 * 这里在 CI（pnpm test）里静态核对三方一致；更新 endpoint 同理：前端展示的
 * URL 必须与 tauri.conf.json 里真正生效的那条一致。
 *
 * 与 styleContract.test.ts 同一模式：直接读源文件做断言（仓库里仅有的
 * 两处允许碰 node:fs 的前端测试）。
 */
import { readFileSync } from "node:fs";
import { describe, expect, it } from "vitest";

/** 读仓库根下的文件（测试在 src/lib/ 下，往上两级即根）。 */
function readText(relative: string): string {
  return readFileSync(new URL(`../../${relative}`, import.meta.url), "utf8");
}

describe("版本号单一来源契约", () => {
  it("package.json / tauri.conf.json / workspace Cargo.toml 三方一致", () => {
    const pkg = JSON.parse(readText("package.json")) as { version: string };
    const tauri = JSON.parse(readText("src-tauri/tauri.conf.json")) as {
      version: string;
    };
    const cargo = readText("Cargo.toml");
    const match = cargo.match(/^version\s*=\s*"([^"]+)"/m);

    expect(match, "根 Cargo.toml 必须声明 [workspace.package] version").not.toBeNull();
    expect(pkg.version).toBe(tauri.version);
    expect(pkg.version).toBe(match?.[1]);
  });

  it("前端展示的更新清单地址与 tauri.conf.json 生效的 endpoint 一致", () => {
    const tauri = JSON.parse(readText("src-tauri/tauri.conf.json")) as {
      plugins: { updater: { endpoints: string[] } };
    };
    const source = readText("src/lib/updater.ts");
    const declared = tauri.plugins.updater.endpoints[0];

    expect(declared, "updater 至少要有一个 endpoint").toBeDefined();
    // UPDATER_ENDPOINT 常量必须原样出现（不是文件路径、不是另一个 URL）
    expect(source).toContain("export const UPDATER_ENDPOINT");
    expect(source).toContain(declared!);
  });

  it("updater 公钥已配置且不是占位符", () => {
    const tauri = JSON.parse(readText("src-tauri/tauri.conf.json")) as {
      plugins: { updater: { pubkey: string } };
    };
    expect(tauri.plugins.updater.pubkey.length).toBeGreaterThan(50);
  });
});
