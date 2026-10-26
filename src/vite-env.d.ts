/// <reference types="vite/client" />

/*
 * 样式契约测试（src/lib/styleContract.test.ts）需要直接读源文件做静态检查。
 *
 * 全仓库只有这一个测试用到 Node 内置模块。这里只声明真正用到的两个 API，
 * 不引入 @types/node：那样会让 process / Buffer 之类在浏览器侧代码里也"有类型"，
 * 从而失去"前端不能碰 Node API"的保护。
 */
declare module "node:fs" {
  export function readFileSync(path: URL | string, encoding: "utf8"): string;
  export function readdirSync(path: string): string[];
  export function statSync(path: string): { isDirectory(): boolean };
}

declare module "node:url" {
  export function fileURLToPath(url: URL | string): string;
}
