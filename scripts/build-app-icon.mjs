#!/usr/bin/env node
/**
 * 生成 Windows 的 src-tauri/icons/icon.ico：每档尺寸都从矢量母版单独栅格化。
 *
 * 为什么不再用 "npx tauri icon" 生成 ico：
 *   1. 它只吃一张 1024 PNG，再把同一张图重采样到 16/24/32/48/64/256。
 *      16px 里三条横杠被压成 2px 的糊状（logo/README 里那份「两杠简化版」小母版根本没被用上）。
 *   2. 它写出的每一帧都是 PNG 压缩帧。Windows 只为 256px 承诺支持 PNG 帧，
 *      小尺寸走 PNG 在部分 shell 路径会被忽略、回退成「把 256 帧缩下来」——这就是「图标发虚」。
 *
 * 本脚本：每档尺寸从矢量母版按目标尺寸直接栅格化（没有二次重采样），
 * <=24px 用两杠小母版；ICO 里 16-128 写成 DIB(BMP) 帧，只有 256 用 PNG 帧。
 *
 * 依赖：本机 Chrome / Edge（无头截图）。不引入任何 npm 包。
 *
 * 用法：
 *   node scripts/build-app-icon.mjs
 *   node scripts/build-app-icon.mjs --out /tmp/test.ico
 */
import { execFileSync } from "node:child_process";
import { existsSync, mkdtempSync, readFileSync, rmSync, writeFileSync } from "node:fs";
import { tmpdir } from "node:os";
import { join, resolve } from "node:path";
import { inflateSync } from "node:zlib";
import { fileURLToPath } from "node:url";

const ROOT = resolve(fileURLToPath(new URL("..", import.meta.url)));
const ICON_DIR = join(ROOT, "logo", "icon");
const MASTER = join(ICON_DIR, "app-icon-master.svg");
const SMALL = join(ICON_DIR, "app-icon-small.svg");
const DEFAULT_OUT = join(ROOT, "src-tauri", "icons", "icon.ico");

/**
 * 每档尺寸的母版与编码方式。
 * DIB 帧 = 老 shell 路径也能读的 32 位 BGRA + 掩码；PNG 帧只给 256（Windows 明确支持的那一档）。
 */
export const FRAMES = [
  { size: 16, source: "small", encoding: "dib" },
  { size: 20, source: "small", encoding: "dib" },
  { size: 24, source: "master", encoding: "dib" },
  { size: 32, source: "master", encoding: "dib" },
  { size: 40, source: "master", encoding: "dib" },
  { size: 48, source: "master", encoding: "dib" },
  { size: 64, source: "master", encoding: "dib" },
  { size: 96, source: "master", encoding: "dib" },
  { size: 128, source: "master", encoding: "dib" },
  { size: 256, source: "master", encoding: "png" },
];

/** 依次尝试的浏览器：CHROME_PATH -> Chrome -> Edge。 */
export function findBrowser(env = process.env) {
  const candidates = [
    env.CHROME_PATH,
    "C:\\Program Files\\Google\\Chrome\\Application\\chrome.exe",
    "C:\\Program Files (x86)\\Google\\Chrome\\Application\\chrome.exe",
    "C:\\Program Files (x86)\\Microsoft\\Edge\\Application\\msedge.exe",
    "C:\\Program Files\\Microsoft\\Edge\\Application\\msedge.exe",
  ].filter(Boolean);
  const hit = candidates.find((p) => existsSync(p));
  if (!hit) {
    throw new Error("找不到 Chrome/Edge：请设置 CHROME_PATH 指向浏览器可执行文件");
  }
  return hit;
}

/** 从矢量母版里取出渐变与矩形（母版只有 <rect>，解析比引 SVG 依赖划算）。 */
export function parseIcon(svg) {
  const stops = [...svg.matchAll(/<stop offset="[^"]*" stop-color="([^"]+)"/g)].map((m) => m[1]);
  const rects = [...svg.matchAll(/<rect ([^/]+)\/>/g)].map((match) => {
    const attrs = Object.fromEntries(
      [...match[1].matchAll(/(\w+)="([^"]*)"/g)].map((pair) => [pair[1], pair[2]])
    );
    return {
      x: Number(attrs.x),
      y: Number(attrs.y),
      w: Number(attrs.width),
      h: Number(attrs.height),
      rx: Number(attrs.rx ?? 0),
      fill: attrs.fill,
    };
  });
  return { stops, rects };
}

/**
 * 生成某一档尺寸的 SVG：几何吸附到整数像素。
 *
 * 为什么不让浏览器把 1024 的母版缩下来：亚像素坐标会让直边落在半像素上，
 * 每条边都被抗锯齿抹成两行灰 —— 32px 的任务栏图标就是这样"发虚"的。
 * 这里把底板留白、横杠高度、间距、起点全部取整，直边就落在像素边界上；
 * 横杠按"上一条的下沿 + 间距"顺排，避免各自四舍五入后在 16px 粘成一坨。
 */
export function buildSvgAtSize(icon, size) {
  const scale = size / 1024;
  const [background, ...bars] = icon.rects;
  const margin = Math.max(1, Math.round(background.x * scale));
  const side = size - margin * 2;
  const radius = Math.min(Math.round(background.rx * scale), Math.floor(side / 2));
  const parts = [
    '<rect x="' + margin + '" y="' + margin + '" width="' + side + '" height="' + side +
      '" rx="' + radius + '" fill="' + background.fill + '"/>',
  ];

  let y = Math.round(bars[0].y * scale);
  bars.forEach((bar, index) => {
    const height = Math.max(1, Math.round(bar.h * scale));
    const x = Math.round(bar.x * scale);
    const width = Math.max(1, Math.round(bar.w * scale));
    const rx = Math.min(Math.max(1, Math.round(bar.rx * scale)), Math.floor(height / 2));
    parts.push(
      '<rect x="' + x + '" y="' + y + '" width="' + width + '" height="' + height +
        '" rx="' + rx + '" fill="' + bar.fill + '"/>'
    );
    const next = bars[index + 1];
    if (next) {
      const gap = next.y - (bar.y + bar.h);
      y += height + Math.max(1, Math.round(gap * scale));
    }
  });

  const defs =
    '<defs><linearGradient id="g" x1="0" y1="0" x2="0" y2="1">' +
    icon.stops.map((stop, i) => '<stop offset="' + (i === 0 ? 0 : 1) + '" stop-color="' + stop + '"/>').join("") +
    "</linearGradient></defs>";
  return (
    '<svg xmlns="http://www.w3.org/2000/svg" viewBox="0 0 ' + size + " " + size +
    '" width="' + size + '" height="' + size + '">' + defs + parts.join("") + "</svg>"
  );
}

/** 把已定尺的 SVG 包成一张网页（无头浏览器截图用）。 */
export function buildHtml(svg) {
  return (
    '<!doctype html><meta charset="utf-8">' +
    "<style>html,body{margin:0;padding:0;background:transparent;overflow:hidden}svg{display:block}</style>" +
    svg
  );
}

/** 用无头浏览器把已定尺的 SVG 截成 size x size 的 PNG。 */
export function renderPng(svg, size, browser, workDir) {
  const html = buildHtml(svg);
  const htmlPath = join(workDir, "icon-" + size + ".html");
  const pngPath = join(workDir, "icon-" + size + ".png");
  writeFileSync(htmlPath, html, "utf8");
  execFileSync(
    browser,
    [
      "--headless=new",
      "--disable-gpu",
      "--hide-scrollbars",
      "--force-device-scale-factor=1",
      "--user-data-dir=" + join(workDir, "chrome-profile"),
      "--window-size=" + size + "," + size,
      "--default-background-color=00000000",
      "--screenshot=" + pngPath,
      "file:///" + htmlPath.replace(/\\/g, "/"),
    ],
    { stdio: "ignore" }
  );
  return readFileSync(pngPath);
}

/**
 * 最小 PNG 解码器（8 位、RGB/RGBA/灰度+alpha，无隔行）。
 *
 * 只用 node:zlib，不引第三方包：图标是构建期产物，不值得为它加一个图像依赖。
 * 返回 { width, height, rgba }，rgba 为 top-down 顺序。
 */
export function decodePng(buf) {
  if (buf.readUInt32BE(0) !== 0x89504e47) {
    throw new Error("不是 PNG");
  }
  let pos = 8;
  let width = 0;
  let height = 0;
  let bitDepth = 0;
  let colorType = 0;
  const idat = [];
  while (pos < buf.length) {
    const len = buf.readUInt32BE(pos);
    const type = buf.toString("ascii", pos + 4, pos + 8);
    const data = buf.subarray(pos + 8, pos + 8 + len);
    if (type === "IHDR") {
      width = data.readUInt32BE(0);
      height = data.readUInt32BE(4);
      bitDepth = data[8];
      colorType = data[9];
    } else if (type === "IDAT") {
      idat.push(data);
    } else if (type === "IEND") {
      break;
    }
    pos += 12 + len;
  }
  if (bitDepth !== 8) {
    throw new Error("只支持 8 位 PNG，实际 " + bitDepth);
  }
  const channels = colorType === 6 ? 4 : colorType === 2 ? 3 : colorType === 4 ? 2 : colorType === 0 ? 1 : 0;
  if (!channels) {
    throw new Error("不支持的 PNG 颜色类型 " + colorType);
  }
  const raw = inflateSync(Buffer.concat(idat));
  const stride = width * channels;
  const rgba = Buffer.alloc(width * height * 4);
  let prev = Buffer.alloc(stride);
  for (let y = 0; y < height; y += 1) {
    const filter = raw[y * (stride + 1)];
    const line = raw.subarray(y * (stride + 1) + 1, (y + 1) * (stride + 1));
    const cur = Buffer.alloc(stride);
    for (let i = 0; i < stride; i += 1) {
      const a = i >= channels ? cur[i - channels] : 0;
      const b = prev[i];
      const c = i >= channels ? prev[i - channels] : 0;
      let value = line[i];
      if (filter === 1) {
        value += a;
      } else if (filter === 2) {
        value += b;
      } else if (filter === 3) {
        value += (a + b) >> 1;
      } else if (filter === 4) {
        const p = a + b - c;
        const pa = Math.abs(p - a);
        const pb = Math.abs(p - b);
        const pc = Math.abs(p - c);
        value += pa <= pb && pa <= pc ? a : pb <= pc ? b : c;
      }
      cur[i] = value & 0xff;
    }
    for (let x = 0; x < width; x += 1) {
      const src = x * channels;
      const dst = (y * width + x) * 4;
      if (channels >= 3) {
        rgba[dst] = cur[src];
        rgba[dst + 1] = cur[src + 1];
        rgba[dst + 2] = cur[src + 2];
        rgba[dst + 3] = channels === 4 ? cur[src + 3] : 255;
      } else if (channels === 2) {
        rgba[dst] = rgba[dst + 1] = rgba[dst + 2] = cur[src];
        rgba[dst + 3] = cur[src + 1];
      } else {
        rgba[dst] = rgba[dst + 1] = rgba[dst + 2] = cur[src];
        rgba[dst + 3] = 255;
      }
    }
    prev = cur;
  }
  return { width, height, rgba };
}

/**
 * 32 位 DIB 帧：BITMAPINFOHEADER + 自下而上的 BGRA + 1bpp AND 掩码。
 *
 * 掩码只对「完全透明」的像素置 1：有些老路径仍按掩码做二值透明度，
 * 半透明边留给 alpha 通道，圆角才不会变成锯齿。
 */
export function encodeDib(size, rgba) {
  const header = Buffer.alloc(40);
  header.writeUInt32LE(40, 0);
  header.writeInt32LE(size, 4);
  header.writeInt32LE(size * 2, 8);
  header.writeUInt16LE(1, 12);
  header.writeUInt16LE(32, 14);
  header.writeUInt32LE(0, 16);
  header.writeUInt32LE(size * size * 4, 20);

  const pixels = Buffer.alloc(size * size * 4);
  for (let y = 0; y < size; y += 1) {
    const srcRow = size - 1 - y;
    for (let x = 0; x < size; x += 1) {
      const src = (srcRow * size + x) * 4;
      const dst = (y * size + x) * 4;
      pixels[dst] = rgba[src + 2];
      pixels[dst + 1] = rgba[src + 1];
      pixels[dst + 2] = rgba[src];
      pixels[dst + 3] = rgba[src + 3];
    }
  }

  const maskStride = Math.ceil(size / 32) * 4;
  const mask = Buffer.alloc(maskStride * size);
  for (let y = 0; y < size; y += 1) {
    const srcRow = size - 1 - y;
    for (let x = 0; x < size; x += 1) {
      if (rgba[(srcRow * size + x) * 4 + 3] === 0) {
        mask[y * maskStride + (x >> 3)] |= 0x80 >> (x & 7);
      }
    }
  }
  return Buffer.concat([header, pixels, mask]);
}

/** 组装 ICONDIR + 目录项 + 各帧数据。 */
export function encodeIco(frames) {
  const dir = Buffer.alloc(6);
  dir.writeUInt16LE(0, 0);
  dir.writeUInt16LE(1, 2);
  dir.writeUInt16LE(frames.length, 4);

  const entries = [];
  let offset = 6 + frames.length * 16;
  for (const frame of frames) {
    const entry = Buffer.alloc(16);
    entry[0] = frame.size >= 256 ? 0 : frame.size;
    entry[1] = frame.size >= 256 ? 0 : frame.size;
    entry[2] = 0;
    entry[3] = 0;
    entry.writeUInt16LE(1, 4);
    entry.writeUInt16LE(32, 6);
    entry.writeUInt32LE(frame.data.length, 8);
    entry.writeUInt32LE(offset, 12);
    entries.push(entry);
    offset += frame.data.length;
  }
  return Buffer.concat([dir, ...entries, ...frames.map((f) => f.data)]);
}

function main(argv) {
  const outIndex = argv.indexOf("--out");
  const out = outIndex > -1 ? resolve(argv[outIndex + 1]) : DEFAULT_OUT;
  const browser = findBrowser();
  const workDir = mkdtempSync(join(tmpdir(), "app-catch-icon-"));
  try {
    const icons = {
      master: parseIcon(readFileSync(MASTER, "utf8")),
      small: parseIcon(readFileSync(SMALL, "utf8")),
    };
    const frames = FRAMES.map((frame) => {
      const svg = buildSvgAtSize(icons[frame.source], frame.size);
      const png = renderPng(svg, frame.size, browser, workDir);
      if (frame.encoding === "png") {
        return { size: frame.size, data: png };
      }
      const decoded = decodePng(png);
      if (decoded.width !== frame.size || decoded.height !== frame.size) {
        throw new Error("截图尺寸不符：" + decoded.width + "x" + decoded.height + " != " + frame.size);
      }
      return { size: frame.size, data: encodeDib(frame.size, decoded.rgba) };
    });
    writeFileSync(out, encodeIco(frames));
    const total = frames.reduce((sum, f) => sum + f.data.length, 0);
    console.log("icon.ico -> " + out);
    console.log("frames: " + frames.map((f) => f.size).join(", ") + " (" + total + " bytes)");
  } finally {
    rmSync(workDir, { recursive: true, force: true });
  }
}

if (process.argv[1] && resolve(process.argv[1]) === fileURLToPath(import.meta.url)) {
  main(process.argv.slice(2));
}