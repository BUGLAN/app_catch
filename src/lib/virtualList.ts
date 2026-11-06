/**
 * 虚拟滚动的纯计算：累计偏移、可见区间、定位。
 *
 * 日志行高度不固定（长行会折行、分隔行 / 提示行更高），因此按「已测量的真实高度，
 * 没测过的用估计值」累加出每行的顶部偏移，再二分查找视口覆盖的区间。
 * 只渲染这个区间（外加上下 overscan），其余用占位高度撑开滚动条。
 */

/**
 * 累计偏移：`offsets[i]` 是第 i 行的顶部位置，`offsets[count]` 是总高度。
 */
export function buildOffsets(count: number, heightOf: (index: number) => number): number[] {
  const offsets = new Array<number>(count + 1);
  offsets[0] = 0;
  for (let index = 0; index < count; index += 1) {
    offsets[index + 1] = offsets[index]! + heightOf(index);
  }
  return offsets;
}

/** 判定视口下沿时的容差（像素）。 */
const BOTTOM_EPSILON = 0.001;

/** 第一个「底边在 position 之下」的行号（二分）。 */
function rowAt(offsets: number[], position: number): number {
  const count = offsets.length - 1;
  let low = 0;
  let high = count;
  while (low < high) {
    const middle = (low + high) >>> 1;
    if (offsets[middle + 1]! <= position) {
      low = middle + 1;
    } else {
      high = middle;
    }
  }
  return low;
}

/**
 * 视口 [scrollTop, scrollTop + viewportHeight) 上下各扩 overscan 像素后覆盖的行区间。
 *
 * 返回 `[start, end)`；没有行时为 `{ start: 0, end: 0 }`。
 */
export function visibleRange(
  offsets: number[],
  scrollTop: number,
  viewportHeight: number,
  overscan: number
): { start: number; end: number } {
  const count = offsets.length - 1;
  if (count <= 0) {
    return { start: 0, end: 0 };
  }
  const top = Math.max(0, scrollTop - overscan);
  const bottom = scrollTop + Math.max(0, viewportHeight) + overscan;
  const start = Math.min(rowAt(offsets, top), count - 1);
  // 视口下沿是开区间：恰好从 bottom 开始的那一行不可见，因此取 bottom 之前一点所在的行
  const last = rowAt(offsets, bottom - BOTTOM_EPSILON);
  const end = Math.max(start + 1, Math.min(count, last + 1));
  return { start, end };
}

/**
 * 让第 index 行出现在视口中间所需的 scrollTop（已夹到合法范围）。
 */
export function centeredScrollTop(
  offsets: number[],
  index: number,
  viewportHeight: number
): number {
  const count = offsets.length - 1;
  if (count <= 0) {
    return 0;
  }
  const clamped = Math.min(Math.max(index, 0), count - 1);
  const rowTop = offsets[clamped]!;
  const rowHeight = offsets[clamped + 1]! - rowTop;
  const target = rowTop - (viewportHeight - rowHeight) / 2;
  const maxTop = Math.max(0, offsets[count]! - viewportHeight);
  return Math.min(Math.max(0, target), maxTop);
}
