/**
 * 侧栏搜索：按名称 / id / 标签过滤应用。
 *
 * 单独成模块（而不是留在 App.tsx 里）的两个理由：
 * - 它是纯函数，能直接单测"搜了没结果"到底是逻辑问题还是真的没数据；
 * - App.tsx 里 import 了整棵组件树，从那里导出函数会让测试拖进 Tauri 客户端。
 */
import type { AppConfig } from "./types";

/** 一个应用是否匹配搜索词。空查询恒为 true（= 不过滤）。 */
export function matchesQuery(app: AppConfig, rawQuery: string): boolean {
  const query = rawQuery.trim().toLowerCase();
  if (query.length === 0) {
    return true;
  }
  if (app.name.toLowerCase().includes(query) || app.id.toLowerCase().includes(query)) {
    return true;
  }
  return (app.tags ?? []).some((tag) => tag.toLowerCase().includes(query));
}

/** 过滤应用列表；查询为空时原样返回（保持引用，避免下游无谓重渲染）。 */
export function filterApps(apps: AppConfig[], rawQuery: string): AppConfig[] {
  if (rawQuery.trim().length === 0) {
    return apps;
  }
  return apps.filter((app) => matchesQuery(app, rawQuery));
}
