/** AppForm 的纯逻辑测试：id 校验、参数/环境变量/标签解析。 */
import { describe, expect, it } from "vitest";

import { APP_ID_PATTERN, parseArgs, parseEnv, parseTags, validateAppId } from "../components/AppForm";

describe("validateAppId", () => {
  it("接受合法 id", () => {
    for (const ok of ["a", "GPT-SoVITS", "app_1", "x".repeat(64)]) {
      expect(validateAppId(ok)).toBeNull();
    }
  });

  it("拒绝空值与超长", () => {
    expect(validateAppId("")).toContain("不能为空");
    expect(validateAppId("   ")).toContain("不能为空");
    expect(validateAppId("x".repeat(65))).toContain("64");
  });

  it("拒绝非法字符并给出与 Rust 侧一致的正则", () => {
    for (const bad of ["含中文", "a b", "a/b", "a\\b", "dot.dot", "emoji🎯"]) {
      expect(validateAppId(bad)).toContain("只允许");
    }
    expect(APP_ID_PATTERN.source).toBe("^[A-Za-z0-9_-]{1,64}$");
  });

  it("自动忽略首尾空白", () => {
    expect(validateAppId("  demo  ")).toBeNull();
  });
});

describe("parseArgs（每行一个参数）", () => {
  it("按行拆分并丢弃空行", () => {
    expect(parseArgs("main.py\n\n--port\n9880\n")).toEqual(["main.py", "--port", "9880"]);
  });

  it("参数内部的空格被保留（不按空白拆分）", () => {
    expect(parseArgs("--prompt\nhello world")).toEqual(["--prompt", "hello world"]);
  });

  it("兼容 CRLF", () => {
    expect(parseArgs("a\r\nb\r\n")).toEqual(["a", "b"]);
  });

  it("全空白返回空数组", () => {
    expect(parseArgs("   \n\t\n")).toEqual([]);
  });
});

describe("parseEnv（每行 KEY=VALUE）", () => {
  it("解析多个变量并保留值里的等号与空格", () => {
    const { env, errors } = parseEnv("A=1\nB=hello world\nC=x=y");
    expect(errors).toEqual([]);
    expect(env).toEqual({ A: "1", B: "hello world", C: "x=y" });
  });

  it("缺少等号时报出行号", () => {
    const { errors } = parseEnv("A=1\nBADLINE\nC=3");
    expect(errors).toHaveLength(1);
    expect(errors[0]).toContain("第 2 行");
  });

  it("键为空或含空白时报错", () => {
    expect(parseEnv("=1").errors[0]).toContain("键为空");
    expect(parseEnv("BAD KEY=1").errors[0]).toContain("空白");
  });

  it("空行被忽略", () => {
    const { env, errors } = parseEnv("\n\nA=1\n");
    expect(errors).toEqual([]);
    expect(env).toEqual({ A: "1" });
  });
});

describe("parseTags（逗号分隔）", () => {
  it("去空白、去重、保持顺序", () => {
    expect(parseTags("python, tts, python , ,rust")).toEqual(["python", "tts", "rust"]);
  });

  it("支持中文逗号", () => {
    expect(parseTags("语音，tts")).toEqual(["语音", "tts"]);
  });

  it("空输入返回空数组", () => {
    expect(parseTags("")).toEqual([]);
    expect(parseTags("  ,  ")).toEqual([]);
  });
});
