/**
 * 应用图标组件：有图标用图标，没有就用内置默认图，加载中显示淡入占位。
 *
 * 设计取舍：
 * - **不做错误提示**：图标失败是常态（Python 服务类应用抓不到），
 *   静默降级到默认图即可，绝不能用 toast 打扰用户；
 * - **尺寸类里写死宽高**：避免图标加载完成后撑动列表布局（CLS）；
 * - **alt="" + aria-hidden**：应用名就在旁边，图标对读屏是冗余信息。
 */
import { useEffect, useState } from "react";

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
}

/** 已加载的内置默认图（模块级共享，只取一次）。 */
let cachedDefaultIcon: string | null = null;

export function AppIcon({ appId, name, size = "sm", refreshKey = 0 }: Props) {
  const [icon, setIcon] = useState<IconView>(undefined);
  const [fallback, setFallback] = useState<string>(cachedDefaultIcon ?? "");

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
      }
    });
    return () => {
      cancelled = true;
    };
  }, []);

  // 应用图标：appId 变化或外部要求刷新时重新取
  useEffect(() => {
    let cancelled = false;
    setIcon(undefined);
    loadIcon(appId).then((value) => {
      if (!cancelled) {
        setIcon(value);
      }
    });
    return () => {
      cancelled = true;
    };
  }, [appId, refreshKey]);

  const src = icon ?? fallback;
  const className = "app-icon app-icon-" + size;

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
      // 加载失败静默降级：把图标置空，交给默认图/首字块
      onError={() => setIcon(null)}
    />
  );
}
