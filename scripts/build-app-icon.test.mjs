/**
 * build-app-icon 的单元测试。
 *
 * 守两件事：
 *   1. ICO 里小尺寸必须是 DIB 帧（PNG 帧只有 256 这一档是 Windows 保证支持的，
 *      小尺寸写 PNG 会被部分 shell 路径忽略 → 回退成放大的 256，看起来发虚）；
 *   2. 16/20 用两杠简化母版、其余用三杠母版，别某天又变回「一张 1024 缩小到底」。
 *
 * 不驱动浏览器：渲染那一段依赖本机 Chrome，交给 "node scripts/build-app-icon.mjs" 手工跑；
 * 这里只验证纯函数与仓库里已提交的那份 icon.ico。
 */
import { readFileSync } from "node:fs";
import { join } from "node:path";
import { crc32, deflateSync } from "node:zlib";
import { describe, expect, it } from "vitest";

import { FRAMES, buildSvgAtSize, decodePng, encodeDib, encodeIco, parseIcon } from "./build-app-icon.mjs";

const REPO = join(new URL("..", import.meta.url).pathname.replace(/^\/([A-Za-z]:)/, "$1"));

/** 最小 PNG 编码器（8 位 RGBA、无隔行、filter 0）：只为测试 decodePng。 */
function encodePng(width, height, rgba) {
  const chunk = (type, data) => {
    const head = Buffer.alloc(8);
    head.writeUInt32BE(data.length, 0);
    head.write(type, 4, "ascii");
    const body = Buffer.concat([head.subarray(4), data]);
    const tail = Buffer.alloc(4);
    tail.writeUInt32BE(crc32(body), 0);
    return Buffer.concat([head, data, tail]);
  };
  const ihdr = Buffer.alloc(13);
  ihdr.writeUInt32BE(width, 0);
  ihdr.writeUInt32BE(height, 4);
  ihdr[8] = 8;
  ihdr[9] = 6;
  const raw = Buffer.alloc((width * 4 + 1) * height);
  for (let y = 0; y < height; y += 1) {
    raw[y * (width * 4 + 1)] = 0;
    rgba.copy(raw, y * (width * 4 + 1) + 1, y * width * 4, (y + 1) * width * 4);
  }
  return Buffer.concat([
    Buffer.from([0x89, 0x50, 0x4e, 0x47, 0x0d, 0x0a, 0x1a, 0x0a]),
    chunk("IHDR", ihdr),
    chunk("IDAT", deflateSync(raw)),
    chunk("IEND", Buffer.alloc(0)),
  ]);
}

/** 解析 ICO 目录项（只读目录，不展开帧数据）。 */
function readIcoDirectory(buf) {
  expect(buf.readUInt16LE(0), "ICONDIR.reserved").toBe(0);
  expect(buf.readUInt16LE(2), "ICONDIR.type").toBe(1);
  const count = buf.readUInt16LE(4);
  const entries = [];
  for (let i = 0; i < count; i += 1) {
    const base = 6 + i * 16;
    entries.push({
      width: buf[base] === 0 ? 256 : buf[base],
      height: buf[base + 1] === 0 ? 256 : buf[base + 1],
      bpp: buf.readUInt16LE(base + 6),
      bytes: buf.readUInt32LE(base + 8),
      offset: buf.readUInt32LE(base + 12),
    });
  }
  return entries;
}

describe("图标尺寸表", () => {
  it("覆盖 Windows 实际会请求的全部档位", () => {
    const sizes = FRAMES.map((f) => f.size);
    // 16/20/24/32/40/48 对应 100%-150% DPI 的列表与任务栏，64/96/128 对应 200%+，256 是大图标视图
    for (const size of [16, 20, 24, 32, 40, 48, 64, 96, 128, 256]) {
      expect(sizes, "缺少 " + size + "px 一档").toContain(size);
    }
    expect(sizes).toEqual([...sizes].sort((a, b) => a - b));
  });

  it("最小的两档用两杠简化母版（三条横杠在 16px 会糊成一片）", () => {
    const small = FRAMES.filter((f) => f.source === "small").map((f) => f.size);
    expect(small).toEqual([16, 20]);
  });

  it("只有 256 用 PNG 帧，其余用 DIB 帧", () => {
    for (const frame of FRAMES) {
      expect(frame.encoding, frame.size + "px 的编码").toBe(frame.size === 256 ? "png" : "dib");
    }
  });
});

describe("按尺寸栅格化的几何", () => {
  const master = parseIcon(
    readFileSync(join(REPO, "logo", "icon", "app-icon-master.svg"), "utf8")
  );
  const small = parseIcon(
    readFileSync(join(REPO, "logo", "icon", "app-icon-small.svg"), "utf8")
  );

  /** 从生成的 SVG 里读出所有 rect 的数值属性。 */
  const rectsOf = (svg) =>
    [...svg.matchAll(/<rect x="([\d.]+)" y="([\d.]+)" width="([\d.]+)" height="([\d.]+)" rx="([\d.]+)"/g)].map(
      (m) => ({ x: Number(m[1]), y: Number(m[2]), w: Number(m[3]), h: Number(m[4]), rx: Number(m[5]) })
    );

  it("所有坐标都是整数像素（亚像素直边正是发虚的来源）", () => {
    for (const size of [16, 20, 24, 32, 40, 48, 64, 96, 128, 256]) {
      const svg = buildSvgAtSize(master, size);
      for (const rect of rectsOf(svg)) {
        expect(Number.isInteger(rect.x) && Number.isInteger(rect.y), size + "px 的 x/y 非整数").toBe(true);
        expect(Number.isInteger(rect.w) && Number.isInteger(rect.h), size + "px 的宽高非整数").toBe(true);
      }
    }
  });

  it("横杠不会互相压住，也不会越出画布", () => {
    for (const [size, icon] of [[16, small], [20, small], [24, master], [32, master], [48, master]]) {
      const [, ...bars] = rectsOf(buildSvgAtSize(icon, size));
      expect(bars.length, size + "px 应有横杠").toBeGreaterThan(0);
      for (let i = 0; i < bars.length; i += 1) {
        expect(bars[i].y + bars[i].h, size + "px 第 " + (i + 1) + " 条横杠越界").toBeLessThanOrEqual(size);
        if (i > 0) {
          expect(bars[i].y, size + "px 第 " + (i + 1) + " 条横杠与上一条重叠").toBeGreaterThanOrEqual(
            bars[i - 1].y + bars[i - 1].h + 1
          );
        }
      }
    }
  });

  it("底板左右留白对称，圆角不超过边长一半", () => {
    for (const size of [16, 32, 64]) {
      const [bg] = rectsOf(buildSvgAtSize(master, size));
      expect(bg.x).toBe(size - bg.x - bg.w);
      expect(bg.rx).toBeLessThanOrEqual(Math.floor(bg.w / 2));
    }
  });
});

describe("PNG 解码", () => {
  it("往返一个 2x2 的 RGBA 图", () => {
    const rgba = Buffer.from([
      255, 0, 0, 255, 0, 255, 0, 128,
      0, 0, 255, 0, 9, 8, 7, 255,
    ]);
    const decoded = decodePng(encodePng(2, 2, rgba));
    expect(decoded.width).toBe(2);
    expect(decoded.height).toBe(2);
    expect(Buffer.compare(decoded.rgba, rgba)).toBe(0);
  });
});

describe("DIB 帧与 ICO 容器", () => {
  it("DIB 头高是像素高的两倍（XOR + AND 掩码），像素自下而上", () => {
    const size = 2;
    const rgba = Buffer.from([
      1, 2, 3, 255, 4, 5, 6, 255,
      7, 8, 9, 0, 10, 11, 12, 255,
    ]);
    const dib = encodeDib(size, rgba);
    expect(dib.readUInt32LE(0)).toBe(40);
    expect(dib.readInt32LE(4)).toBe(size);
    expect(dib.readInt32LE(8)).toBe(size * 2);
    expect(dib.readUInt16LE(14)).toBe(32);
    // 第一行数据对应源图最后一行：RGBA(7,8,9,0) -> BGRA
    expect([...dib.subarray(40, 44)]).toEqual([9, 8, 7, 0]);
    // 完全透明的像素在 AND 掩码里置 1（掩码从 40 + size*size*4 开始）
    const maskStart = 40 + size * size * 4;
    expect(dib[maskStart] & 0b1000_0000).toBe(0b1000_0000);
  });

  it("目录项偏移连续、尺寸 256 记作 0", () => {
    const frames = [
      { size: 16, data: Buffer.alloc(10, 1) },
      { size: 256, data: Buffer.alloc(20, 2) },
    ];
    const ico = encodeIco(frames);
    const entries = readIcoDirectory(ico);
    expect(entries.map((e) => e.width)).toEqual([16, 256]);
    expect(entries[0].offset).toBe(6 + 2 * 16);
    expect(entries[1].offset).toBe(6 + 2 * 16 + 10);
  });
});

describe("仓库里的 icon.ico", () => {
  const ico = readFileSync(join(REPO, "src-tauri", "icons", "icon.ico"));
  const entries = readIcoDirectory(ico);

  it("包含 10 档尺寸且都是 32 位", () => {
    expect(entries.map((e) => e.width)).toEqual([16, 20, 24, 32, 40, 48, 64, 96, 128, 256]);
    for (const entry of entries) {
      expect(entry.bpp, entry.width + "px 的位深").toBe(32);
    }
  });

  it("小尺寸是 DIB 帧（不是 PNG 帧），只有 256 是 PNG", () => {
    for (const entry of entries) {
      const magic = ico.subarray(entry.offset, entry.offset + 8).toString("hex");
      if (entry.width === 256) {
        expect(magic, "256px 应为 PNG 帧").toBe("89504e470d0a1a0a");
      } else {
        // DIB 帧以 BITMAPINFOHEADER 的 40 字节长度开头
        expect(ico.readUInt32LE(entry.offset), entry.width + "px 应为 DIB 帧").toBe(40);
      }
    }
  });
});