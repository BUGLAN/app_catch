/**
 * 把各平台构建产物的 .sig 签名合并成 tauri-plugin-updater 用的静态清单 latest.json。
 *
 * 刻意不写 `#!/usr/bin/env node`：这个文件会被 scripts/*.test.mjs 直接 import，
 * 而 Windows 检出是 CRLF —— shebang 行尾多出的 \r 会让 Vite 的 esbuild 转换
 * 直接抛 “Invalid or unexpected token”（只在 Windows CI 复现）。调用方一律是
 * `node scripts/merge-updater-manifests.mjs`，shebang 本来也没有作用。
 *
 * 为什么不用 tauri-action 自动生成：我们的 Release 是矩阵构建（macOS aarch64 /
 * x86_64 + Windows x64）后统一发的，每个平台只有自己那一份签名；清单要求
 * platforms 里同时出现所有平台的 { signature, url }，所以需要一个合并步骤。
 *
 * 输入约定（release.yml 产出）：
 *   <inputs>/updater-<rust-target>/            # download-artifact 按名字建目录
 *     ├── app_catch_0.1.0_x64-setup.exe.sig    # Windows NSIS（更新器优先用）
 *     ├── app_catch_0.1.0_x64_en-US.msi.sig    # Windows MSI（备选）
 *     └── app_catch.app.tar.gz.sig             # macOS
 *
 * 用法（CI 里）：
 *   node scripts/merge-updater-manifests.mjs \
 *     --inputs updater-inputs --repo BUGLAN/app_catch --tag v0.2.0 \
 *     --version 0.2.0 --notes-file RELEASE_BODY.md --out latest.json
 */
import { readdir, readFile, writeFile } from "node:fs/promises";
import { join } from "node:path";
import { pathToFileURL } from "node:url";

/** rust target -> 更新清单的 platform 键（OS-ARCH）。 */
export const TARGET_TO_PLATFORM = {
  "aarch64-apple-darwin": "darwin-aarch64",
  "x86_64-apple-darwin": "darwin-x86_64",
  "x86_64-pc-windows-msvc": "windows-x86_64",
  "x86_64-unknown-linux-gnu": "linux-x86_64",
};

/**
 * 同一平台出现多种安装包时的优先级（越大越优先）。
 * Windows 更新器默认走 NSIS 的 -setup.exe（官方推荐路径），MSI 只做备选；
 * macOS 用 .app.tar.gz；Linux 用 .AppImage。
 */
function artifactPreference(sigName) {
  if (sigName.endsWith("-setup.exe.sig") || sigName.endsWith(".exe.sig")) return 4;
  if (sigName.endsWith(".app.tar.gz.sig")) return 3;
  if (sigName.endsWith(".AppImage.sig")) return 3;
  if (sigName.endsWith(".msi.sig")) return 2;
  return 0;
}

/** 文件名放进 URL 时只转义真正必须转义的字符，保证与 GitHub 资产名一致。 */
function encodeAssetName(name) {
  return name.replace(/[^-A-Za-z0-9._]/g, (ch) => encodeURIComponent(ch));
}

/**
 * 合并逻辑（纯函数，便于单测）。
 *
 * @param {object} options
 * @param {string} options.inputsDir   各平台 .sig 所在目录（子目录名 updater-<target>）
 * @param {string} options.repo        owner/repo，用于拼下载 URL
 * @param {string} options.tag         形如 v0.2.0 的 tag
 * @param {string} options.version     语义化版本（不带 v）
 * @param {string} [options.notes]     更新说明（Release 正文）
 * @param {() => string} [options.now] 时间源注入（测试用）
 * @returns {Promise<object>}         latest.json 的内容
 */
export async function mergeUpdaterManifests({ inputsDir, repo, tag, version, notes = "", now = () => new Date().toISOString() }) {
  const entries = await readdir(inputsDir, { withFileTypes: true });
  const platformDirs = entries.filter((entry) => entry.isDirectory() && entry.name.startsWith("updater-"));

  if (platformDirs.length === 0) {
    throw new Error(`没有找到任何 updater-* 目录（inputs=${inputsDir}），无法生成更新清单`);
  }

  const platforms = {};
  for (const dir of platformDirs) {
    const target = dir.name.slice("updater-".length);
    const platform = TARGET_TO_PLATFORM[target];
    if (!platform) {
      throw new Error(`未知构建目标 ${target}（目录 ${dir.name}）：请在 TARGET_TO_PLATFORM 里登记`);
    }

    const files = await readdir(join(inputsDir, dir.name));
    const sigs = files.filter((name) => name.endsWith(".sig"));
    if (sigs.length === 0) {
      throw new Error(`${dir.name} 里没有 .sig 签名文件：更新产物签名缺失`);
    }

    /*
     * 先按版本筛选，再按安装包类型排序。
     *
     * 为什么要按版本筛：构建目录里可能同时留着旧版本的 .sig（本地重跑发布、CI 缓存、
     * 手工把 bundle 目录拷来拷去都会这样）。只按类型排序时 "0.1.0" 会排在 "0.2.0" 前面，
     * 于是 latest.json 指向旧安装包 —— 更新器要么下到 404，要么把旧包当新版本推给用户。
     * macOS 的更新包名里没有版本号（app_catch.app.tar.gz.sig），筛出来是空的，回退到全集。
     */
    const versionMatched = sigs.filter((name) => name.includes(version));
    const pool = versionMatched.length > 0 ? versionMatched : sigs;
    pool.sort((a, b) => artifactPreference(b) - artifactPreference(a) || a.localeCompare(b));
    const chosen = pool[0];
    const signature = (await readFile(join(inputsDir, dir.name, chosen), "utf8")).trim();
    if (signature.length === 0) {
      throw new Error(`${chosen} 的签名内容为空`);
    }

    const asset = chosen.slice(0, -".sig".length);
    platforms[platform] = {
      signature,
      url: `https://github.com/${repo}/releases/download/${encodeAssetName(tag)}/${encodeAssetName(asset)}`,
    };
  }

  return {
    version,
    notes,
    pub_date: now(),
    platforms,
  };
}

async function main() {
  const args = process.argv.slice(2);
  const get = (flag) => {
    const index = args.indexOf(flag);
    return index >= 0 ? args[index + 1] : undefined;
  };

  const inputsDir = get("--inputs");
  const repo = get("--repo");
  const tag = get("--tag");
  const version = get("--version");
  const notesFile = get("--notes-file");
  const out = get("--out");
  const missing = [
    ["--inputs", inputsDir],
    ["--repo", repo],
    ["--tag", tag],
    ["--version", version],
    ["--out", out],
  ]
    .filter(([, value]) => !value)
    .map(([flag]) => flag);
  if (missing.length > 0) {
    throw new Error(`缺少参数：${missing.join(" ")}（用法见文件头注释）`);
  }

  let notes = "";
  if (notesFile) {
    notes = (await readFile(notesFile, "utf8")).trim();
  }

  const manifest = await mergeUpdaterManifests({ inputsDir, repo, tag, version, notes });
  await writeFile(out, JSON.stringify(manifest, null, 2) + "\n", "utf8");
  console.log(`latest.json 已生成：${Object.keys(manifest.platforms).join(", ")}`);
}

if (process.argv[1] && import.meta.url === pathToFileURL(process.argv[1]).href) {
  main().catch((error) => {
    console.error(error instanceof Error ? error.message : error);
    process.exit(1);
  });
}
