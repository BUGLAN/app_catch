/**
 * 新建 / 编辑应用表单。
 *
 * 参数输入采用「每行一个参数」（多行 textarea）：参数里可能自带空格
 * （例如 `--prompt "hello world"`），按空白拆分会把参数切坏。
 * 每行一个参数可以无损表达，空行自动忽略。
 *
 * 环境变量采用「每行一条 KEY=VALUE」，同样能容纳值里的空格与等号。
 *
 * 表现层改动：
 *   - 每个字段用共享 Field 组件，错误/提示共用一行消息位（不再整页跳动），
 *     并注入 aria-invalid / aria-describedby / aria-required（原来 aria-invalid 数量是 0）；
 *   - id 的实时校验只在用户动过之后才显示红字（未填写时不先骂用户），
 *     但「保存」按钮可用性判定保持原逻辑不变；
 *   - 底部错误汇总改成带图标的提示块。
 */
import { useMemo, useState } from "react";

import type { AppConfig, AppConfigInput } from "../lib/types";
import { IconAlert } from "./ui/Icons";
import { Field } from "./ui/Field";

interface Props {
  /** null 表示新建，AppConfig 表示编辑。 */
  initial: AppConfig | null;
  busy: boolean;
  onSubmit: (input: AppConfigInput, isNew: boolean) => void;
  onCancel: () => void;
}

/** id 校验规则，与 Rust 侧 AppId 保持一致。 */
export const APP_ID_PATTERN = /^[A-Za-z0-9_-]{1,64}$/;

/** 实时校验应用 id，返回中文错误或 null。 */
export function validateAppId(raw: string): string | null {
  const value = raw.trim();
  if (value.length === 0) {
    return "id 不能为空";
  }
  if (value.length > 64) {
    return "id 长度不能超过 64 个字符";
  }
  if (!APP_ID_PATTERN.test(value)) {
    return "id 只允许 A-Z a-z 0-9 _ -";
  }
  return null;
}

/** 解析「每行一个参数」的文本。 */
export function parseArgs(text: string): string[] {
  return text
    .split(/\r?\n/)
    .map((line) => line.trim())
    .filter((line) => line.length > 0);
}

/** 解析「每行一条 KEY=VALUE」的环境变量文本，返回错误列表。 */
export function parseEnv(text: string): { env: Record<string, string>; errors: string[] } {
  const env: Record<string, string> = {};
  const errors: string[] = [];
  text.split(/\r?\n/).forEach((rawLine, index) => {
    const line = rawLine.trim();
    if (line.length === 0) {
      return;
    }
    const eq = line.indexOf("=");
    if (eq < 0) {
      errors.push(`第 ${index + 1} 行不是 KEY=VALUE 形式`);
      return;
    }
    if (eq === 0) {
      errors.push(`第 ${index + 1} 行的键为空`);
      return;
    }
    const key = line.slice(0, eq).trim();
    const value = line.slice(eq + 1).trim();
    if (key.length === 0) {
      errors.push(`第 ${index + 1} 行的键为空`);
      return;
    }
    if (/\s/.test(key)) {
      errors.push(`第 ${index + 1} 行的键不能含空白字符`);
      return;
    }
    env[key] = value;
  });
  return { env, errors };
}

/** 解析逗号分隔的标签。 */
export function parseTags(text: string): string[] {
  const seen = new Set<string>();
  const tags: string[] = [];
  text.split(/[,，]/).forEach((raw) => {
    const tag = raw.trim();
    if (tag.length > 0 && !seen.has(tag)) {
      seen.add(tag);
      tags.push(tag);
    }
  });
  return tags;
}

/** 把标签数组还原成逗号分隔文本。 */
function joinTags(tags: string[] | undefined): string {
  return (tags ?? []).join(", ");
}

/** 把环境变量还原成每行一条的文本。 */
function joinEnv(env: Record<string, string> | undefined): string {
  return Object.entries(env ?? {})
    .map(([key, value]) => `${key}=${value}`)
    .join("\n");
}

export function AppForm({ initial, busy, onSubmit, onCancel }: Props) {
  const isNew = initial === null;

  const [id, setId] = useState(initial?.id ?? "");
  const [name, setName] = useState(initial?.name ?? "");
  const [command, setCommand] = useState(initial?.command ?? "");
  const [argsText, setArgsText] = useState((initial?.args ?? []).join("\n"));
  const [cwd, setCwd] = useState(initial?.cwd ?? "");
  const [envText, setEnvText] = useState(joinEnv(initial?.env));
  const [tagsText, setTagsText] = useState(joinTags(initial?.tags));
  const [icon, setIcon] = useState(initial?.icon ?? "");
  const [submitted, setSubmitted] = useState(false);

  const idError = useMemo(() => validateAppId(id), [id]);
  const envParsed = useMemo(() => parseEnv(envText), [envText]);

  const nameError = name.trim().length === 0 ? "名称不能为空" : null;
  const commandError = command.trim().length === 0 ? "启动命令不能为空" : null;

  const errors = [idError, nameError, commandError, ...envParsed.errors].filter(
    (value): value is string => value !== null
  );
  const canSubmit = errors.length === 0 && !busy;

  /**
   * 只在提交过一次之后才展示红字。
   * 空表单刚打开就满屏报错是很差的第一印象；提交按钮的可用性判定保持原逻辑。
   */
  const showError = (error: string | null): string | null => (submitted ? error : null);

  const handleSubmit = (event: React.FormEvent) => {
    event.preventDefault();
    setSubmitted(true);
    if (!canSubmit) {
      return;
    }
    onSubmit(
      {
        id: id.trim(),
        name: name.trim(),
        command: command.trim(),
        args: parseArgs(argsText),
        cwd: cwd.trim().length > 0 ? cwd.trim() : null,
        env: envParsed.env,
        icon: icon.trim().length > 0 ? icon.trim() : null,
        tags: parseTags(tagsText),
      },
      isNew
    );
  };

  return (
    <form className="panel form-panel" onSubmit={handleSubmit} noValidate>
      <div className="panel-head">
        <div className="panel-heading">
          <h2 className="panel-title">{isNew ? "新建应用" : `编辑应用 · ${initial.name}`}</h2>
          <p className="panel-sub">
            {isNew ? "填写启动命令与可选参数，保存后即可在左侧列表中启动。" : "id 是配置文件名，创建后不可修改。"}
          </p>
        </div>
        <div className="form-actions">
          <button type="button" className="btn btn-ghost" onClick={onCancel} disabled={busy}>
            取消
          </button>
          <button type="submit" className="btn btn-primary" disabled={!canSubmit}>
            {busy ? "保存中…" : "保存"}
          </button>
        </div>
      </div>

      <div className="form-grid">
        <Field label="id" required error={showError(idError)} hint={isNew ? "只允许 A-Z a-z 0-9 _ -，创建后不可修改" : "id 创建后不可修改"}>
          <input
            className="input mono"
            value={id}
            onChange={(event) => setId(event.target.value)}
            placeholder="例如 gpt-sovits"
            disabled={!isNew || busy}
            spellCheck={false}
            autoComplete="off"
          />
        </Field>

        <Field label="名称" required error={showError(nameError)}>
          <input
            className="input"
            value={name}
            onChange={(event) => setName(event.target.value)}
            placeholder="例如 GPT-SoVITS"
            disabled={busy}
            autoComplete="off"
          />
        </Field>

        <Field label="启动命令" required error={showError(commandError)}>
          <input
            className="input mono"
            value={command}
            onChange={(event) => setCommand(event.target.value)}
            placeholder="例如 python 或 E:/repo/app/start.bat"
            disabled={busy}
            spellCheck={false}
            autoComplete="off"
          />
        </Field>

        <Field label="工作目录">
          <input
            className="input mono"
            value={cwd}
            onChange={(event) => setCwd(event.target.value)}
            placeholder="留空则继承启动器当前目录"
            disabled={busy}
            spellCheck={false}
            autoComplete="off"
          />
        </Field>

        <Field
          label="参数（每行一个）"
          wide
          hint="每行一个参数，不做空白拆分——参数里带空格时也能原样传入；空行忽略。"
        >
          <textarea
            className="input mono textarea"
            value={argsText}
            onChange={(event) => setArgsText(event.target.value)}
            placeholder={"main.py\n--port\n9880"}
            rows={4}
            disabled={busy}
            spellCheck={false}
          />
        </Field>

        <Field
          label="环境变量（每行一条 KEY=VALUE）"
          wide
          error={envParsed.errors.length > 0 ? envParsed.errors.join("；") : null}
          hint="在启动器自身环境基础上追加，不会覆盖进程原有变量。"
        >
          <textarea
            className="input mono textarea"
            value={envText}
            onChange={(event) => setEnvText(event.target.value)}
            placeholder={"PYTHONUNBUFFERED=1\nRUST_LOG=info"}
            rows={4}
            disabled={busy}
            spellCheck={false}
          />
        </Field>

        <Field label="标签（逗号分隔）" hint="支持中文逗号，重复的标签会自动去掉。">
          <input
            className="input"
            value={tagsText}
            onChange={(event) => setTagsText(event.target.value)}
            placeholder="python, tts"
            disabled={busy}
            autoComplete="off"
          />
        </Field>

        <Field label="自定义图标路径（可选）">
          <input
            className="input mono"
            value={icon}
            onChange={(event) => setIcon(event.target.value)}
            placeholder="例如 E:/repo/app/icon.png"
            disabled={busy}
            spellCheck={false}
            autoComplete="off"
          />
        </Field>
      </div>

      {submitted && errors.length > 0 ? (
        <p className="form-summary" role="alert">
          <IconAlert size={14} />
          <span>请先修正上面 {errors.length} 处问题。</span>
        </p>
      ) : null}
    </form>
  );
}