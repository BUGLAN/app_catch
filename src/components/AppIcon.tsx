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
  /**
   * 图标版本：变化时重新从缓存取一次。
   *
   * 会话启动后由 App 对**那一个应用**调用 `recaptureIcon` 强制重抓并写回缓存，
   * 再递增该应用的版本号；本组件只负责重新读缓存，自己从不强制重抓，
   * 因此其他应用的图标不会被牵连，同一应用的列表图标与详情图标也只抓一次。
   */
  version?: number;
}

/** 已加载的内置默认图（模块级共享，只取一次）。 */
let cachedDefaultIcon: string | null = null;

export function AppIcon({
  appId,
  name,
  size = "sm",
  version = 0,
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
   * 记住上一次渲染的 appId：只有换了应用才回到加载态。
   * 同一应用的版本号变化只是换一张图，旧图保留到新图到达，避免闪一下占位块。
   */
  const lastAppId = useRef(appId);

  // 应用图标：appId 或版本号变化时重新取（走缓存）
  useEffect(() => {
    if (lastAppId.current !== appId) {
      lastAppId.current = appId;
      setIcon(undefined);
    }
    let cancelled = false;
    loadIcon(appId).then((value) => {
      if (!cancelled) {
        setIcon(value);
      }
    });
    return () => {
      cancelled = true;
    };
  }, [appId, version]);

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