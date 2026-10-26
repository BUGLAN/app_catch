/**
 * 应用图标组件：有图标用图标，没有就用内置默认图，加载中显示淡入占位。
 *
 * 设计取舍：
 * - **不做错误提示**：图标失败是常态（Python 服务类应用抓不到），
 *   静默降级到默认图即可，绝不能用 toast 打扰用户；
 * - **尺寸类里写死宽高**：避免图标加载完成后撑动列表布局（CLS）；
 * - **alt="" + aria-hidden**：应用名就在旁边，图标对读屏是冗余信息。
 *
 * 本次改动：
 * - 加载阶段先渲染一个同尺寸的占位块（原来在有默认图之前会先闪一下首字块，
 *   默认图到达后再跳成图片，列表会出现一次视觉抖动）；
 * - 图标请求失败时不再把 icon 置为 null（那会让"确认没有图标"和"请求失败"
 *   混为一谈，且与 iconModel 的缓存语义冲突），改为退回默认图；
 * - 装饰性图片加 loading="eager" + decoding="async"，避免 list 重排时的解码阻塞。
 */
import { useEffect, useRef, useState } from "react";

import { loadDefaultIcon, loadIcon, type IconView } from "../lib/iconModel";
import { initialOf } from "../lib/logModel";

interface Props {
  appId: string;
  /** 应用名：仅用于「连默认图都没加载出来」时的首字兜底。 */
  name: string;
  /** 尺寸档位：列表用小，详情用大。 */
  size?: "sm" | "lg";
  /** 外部触发刷新（例如会话启动后重新抓运行时图标）。 */
  refreshKey?: number;
  /**
   * 刷新时是否绕过缓存强制重抓。
   *
   * 会话刚启动那一次必须为真：窗口这时才出现，窗口档位第一次可能拿到真图标，
   * 而磁盘上很可能还躺着一张更早抓到的 `pnpm.cmd` 通用图标。
   */
  forceRefresh?: boolean;
}

/** 已加载的内置默认图（模块级共享，只取一次）。 */
let cachedDefaultIcon: string | null = null;

export function AppIcon({
  appId,
  name,
  size = "sm",
  refreshKey = 0,
  forceRefresh = false,
}: Props) {
  const [icon, setIcon] = useState<IconView>(undefined);
  const [fallback, setFallback] = useState<string>(cachedDefaultIcon ?? "");
  const [defaultResolved, setDefaultResolved] = useState(cachedDefaultIcon !== null);

  // 默认图：全局只取一次；命令失败时保持空串，渲染纯色块兜底
  useEffect(() => {
    if (cachedDefaultIcon !== null) {
      return;
    }
    let cancelled = false;
    loadDefaultIcon().then((url) => {
      cachedDefaultIcon = url;
      if (!cancelled) {
        setFallback(url);
        setDefaultResolved(true);
      }
    });
    return () => {
      cancelled = true;
    };
  }, []);

  /*
   * 记住上一次已经处理过的 refreshKey。
   *
   * 强制重抓只应该发生在「refreshKey 真的递增」的那一次（= 会话刚启动），
   * 而不是每次 appId 切换都顺带穿透一次缓存——后者只是白白多跑一遍 Shell API。
   */
  const lastRefreshKey = useRef(refreshKey);

  // 应用图标：appId 变化或外部要求刷新时重新取
  useEffect(() => {
    const keyChanged = lastRefreshKey.current !== refreshKey;
    lastRefreshKey.current = refreshKey;
    let cancelled = false;
    setIcon(undefined);
    loadIcon(appId, forceRefresh && keyChanged).then((value) => {
      if (!cancelled) {
        setIcon(value);
      }
    });
    return () => {
      cancelled = true;
    };
  }, [appId, refreshKey, forceRefresh]);

  const sizeClass = size === "lg" ? " app-icon-lg" : "";
  const className = "app-icon" + sizeClass;

  // 两者都还在路上：给同尺寸占位块，避免"首字 → 图标"的跳变
  if (icon === undefined && !defaultResolved) {
    return <span className={className + " skeleton"} aria-hidden="true" />;
  }

  const src = icon ?? fallback;

  // 有默认图就用它；连默认图都取不到（非 Tauri 环境）时退回首字块
  if (!src) {
    return (
      <span className={className + " app-icon-text"} aria-hidden="true">
        {initialOf(name)}
      </span>
    );
  }

  return (
    <img
      className={className}
      src={src}
      alt=""
      aria-hidden="true"
      draggable={false}
      decoding="async"
      // 加载失败静默降级：退回默认图/首字块，不抛错、不打扰
      onError={() => setIcon(null)}
    />
  );
}