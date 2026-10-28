/**
 * 主题：跟随系统 / 强制浅色 / 强制深色，三态可切换并持久化。
 *
 * 为什么存"模式"而不是存"结果"：
 * 用户选了「跟随系统」之后，系统在日落时切换深色，界面必须跟着变；
 * 如果只存 `light`/`dark` 两个结果值，这个选项就没法表达。
 *
 * 结果写在 `<html data-theme>` 上，CSS 只认这个属性（见 tokens.css）。
 * `data-theme-mode` 额外记录用户意图，方便设置项回显。
 */
import { useCallback, useEffect, useState } from "react";

import { withViewTransition } from "../lib/motion";
import { readString, writeString, STORAGE_KEYS } from "../lib/preferences";

/** 用户的三个选项。 */
export type ThemeMode = "system" | "light" | "dark";

/** 解析后的实际主题。 */
export type Theme = "light" | "dark";

const MEDIA_QUERY = "(prefers-color-scheme: dark)";

/** 从 localStorage 读上次的选择；非法值退回 system。 */
export function readThemeMode(): ThemeMode {
  const raw = readString(STORAGE_KEYS.theme);
  return raw === "light" || raw === "dark" || raw === "system" ? raw : "system";
}

/** 把"模式 + 系统偏好"解析成实际主题。 */
export function resolveTheme(mode: ThemeMode, systemPrefersDark: boolean): Theme {
  if (mode === "system") {
    return systemPrefersDark ? "dark" : "light";
  }
  return mode;
}

/** 系统当前的明暗偏好（非浏览器环境按深色处理，与产品的默认主题一致）。 */
export function systemPrefersDark(): boolean {
  if (typeof window === "undefined" || typeof window.matchMedia !== "function") {
    return true;
  }
  return window.matchMedia(MEDIA_QUERY).matches;
}

/** 把主题写到 `<html>` 上（幂等）。 */
export function applyTheme(theme: Theme, mode: ThemeMode): void {
  if (typeof document === "undefined") {
    return;
  }
  const root = document.documentElement;
  root.dataset.theme = theme;
  root.dataset.themeMode = mode;
}

interface UseThemeResult {
  mode: ThemeMode;
  theme: Theme;
  setMode: (mode: ThemeMode) => void;
  /**
   * 三态轮转：跟随系统 → 浅色 → 深色 → 跟随系统。
   *
   * 为什么不是二元 toggle：如果只在浅/深之间切，用户从"跟随系统"点一下出去之后
   * 就**再也回不到**"跟随系统"——需求明确要求"可跟随系统或手动切换"，
   * 三态轮转是唯一能同时表达三者的单按钮交互。
   */
  cycle: () => void;
}

/** 轮转顺序。UI 文案与顺序都从这里读，避免两个地方各写一遍。 */
export const THEME_ORDER: ThemeMode[] = ["system", "light", "dark"];

/** 下一次点击会切到哪个模式（纯函数，可单测）。 */
export function nextThemeMode(mode: ThemeMode): ThemeMode {
  const index = THEME_ORDER.indexOf(mode);
  return THEME_ORDER[(index + 1) % THEME_ORDER.length]!;
}

export function useTheme(): UseThemeResult {
  const [mode, setModeState] = useState<ThemeMode>(readThemeMode);
  const [prefDark, setPrefDark] = useState<boolean>(systemPrefersDark);

  const theme = resolveTheme(mode, prefDark);

  // 首屏与应用：每次 mode/theme 变化都同步到 <html>
  useEffect(() => {
    applyTheme(theme, mode);
  }, [theme, mode]);

  // 只有跟随系统时才监听：强制模式下系统变化不该影响界面
  useEffect(() => {
    if (mode !== "system" || typeof window.matchMedia !== "function") {
      return;
    }
    const media = window.matchMedia(MEDIA_QUERY);
    const onChange = (event: MediaQueryListEvent) => setPrefDark(event.matches);
    setPrefDark(media.matches);
    media.addEventListener("change", onChange);
    return () => media.removeEventListener("change", onChange);
  }, [mode]);

  /*
   * 用户主动切换：先在 View Transition 里同步写好 <html data-theme>，
   * 浏览器据此做整屏交叉淡化；随后 React 状态跟上，effect 里的 applyTheme 幂等。
   * 不支持 / 减弱动效时 withViewTransition 退化为直接执行。
   */
  const setMode = useCallback(
    (next: ThemeMode) => {
      withViewTransition(() => applyTheme(resolveTheme(next, prefDark), next));
      setModeState(next);
      writeString(STORAGE_KEYS.theme, next);
    },
    [prefDark]
  );

  const cycle = useCallback(() => setMode(nextThemeMode(mode)), [mode, setMode]);

  return { mode, theme, setMode, cycle };
}