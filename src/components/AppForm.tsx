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
import { useEffect, useMemo, useRef, useState, type InputHTMLAttributes } from "react";

import { api, isHosted } from "../lib/api";
import type { AppConfig, AppConfigInput, CommandSuggestion, ProjectDetection } from "../lib/types";
import { IconAlert, IconFolder, IconSparkle } from "./ui/Icons";
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

/** 校验 hook 超时（秒）：留空表示默认值，否则必须是正整数。 */
export function validateHookTimeout(raw: string): string | null {
  const value = raw.trim();
  if (value.length === 0) {
    return null;
  }
  return /^[1-9][0-9]*$/.test(value) ? null : "hook 超时必须是正整数（秒）";
}

/** 项目类型的中文展示名。 */
export function kindLabel(kind: string): string {
  const [base, detail] = kind.split(":");
  const names: Record<string, string> = {
    node: "Node",
    python: "Python",
    rust: "Rust",
    go: "Go",
    "docker-compose": "Docker Compose",
    script: "启动脚本",
  };
  const name = names[base ?? ""] ?? base ?? kind;
  return detail ? `${name}（${detail}）` : name;
}

/** 从项目名推一个合法 id：非法字符换成 -，首尾 - 去掉，截到 64。空串表示推不出来。 */
export function suggestAppId(name: string): string {
  return name
    .trim()
    .replace(/[^A-Za-z0-9_-]+/g, "-")
    .replace(/^-+|-+$/g, "")
    .slice(0, 64);
}

/** 当前表单里的命令与参数是否正是这条候选。 */
export function matchesSuggestion(
  suggestion: CommandSuggestion,
  command: string,
  argsText: string
): boolean {
  const args = parseArgs(argsText);
  return (
    suggestion.command === command.trim() &&
    suggestion.args.length === args.length &&
    suggestion.args.every((arg, index) => arg === args[index])
  );
}

/**
 * 工作目录输入框 + 「选择…」按钮。
 *
 * Field 会把 id / aria-* 注入到它的直接子元素上；这里把这些属性原样转给 input，
 * 这样 label 仍然指向真正的输入框，读屏与点击标签聚焦都不受影响。
 */
function DirectoryInput({
  onPick,
  pickDisabled,
  ...inputProps
}: InputHTMLAttributes<HTMLInputElement> & { onPick: () => void; pickDisabled: boolean }) {
  return (
    <div className="input-row">
      <input {...inputProps} />
      <button
        type="button"
        className="btn"
        onClick={onPick}
        disabled={pickDisabled}
        title={pickDisabled && !isHosted() ? "需要在桌面应用中使用" : undefined}
      >
        <IconFolder size={14} />
        选择…
      </button>
    </div>
  );
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
  const [beforeText, setBeforeText] = useState((initial?.hooks?.before ?? []).join("\n"));
  const [afterText, setAfterText] = useState((initial?.hooks?.after ?? []).join("\n"));
  const [hookTimeout, setHookTimeout] = useState(
    initial?.hooks?.timeout_secs != null ? String(initial.hooks.timeout_secs) : ""
  );
  const [submitted, setSubmitted] = useState(false);

  /** 最近一次识别结果（带上它对应的目录，防止旧请求覆盖新输入）。 */
  const [detection, setDetection] = useState<{ dir: string; result: ProjectDetection } | null>(null);
  /** 通过「选择…」选中的目录：识别完成后若命令为空，自动套用首选候选。 */
  const autoApplyDir = useRef<string | null>(null);

  const applySuggestion = (suggestion: CommandSuggestion, projectName: string | null) => {
    setCommand(suggestion.command);
    setArgsText(suggestion.args.join("\n"));
    // 名称 / id 只在用户还没填时顺手补上，不覆盖已有输入
    if (projectName) {
      setName((prev) => (prev.trim().length > 0 ? prev : projectName));
      if (isNew) {
        setId((prev) => (prev.trim().length > 0 ? prev : suggestAppId(projectName)));
      }
    }
  };

  // 工作目录变化后（防抖）识别项目类型
  useEffect(() => {
    const dir = cwd.trim();
    if (dir.length === 0 || !isHosted()) {
      setDetection(null);
      return;
    }
    let cancelled = false;
    const timer = window.setTimeout(() => {
      api
        .detectProject(dir)
        .then((result) => {
          if (!cancelled) {
            setDetection({ dir, result });
          }
        })
        .catch(() => {
          if (!cancelled) {
            setDetection(null);
          }
        });
    }, 250);
    return () => {
      cancelled = true;
      window.clearTimeout(timer);
    };
  }, [cwd]);

  // 选择目录后的自动填充：只在命令还空着时套用首选候选
  useEffect(() => {
    if (!detection || autoApplyDir.current !== detection.dir) {
      return;
    }
    autoApplyDir.current = null;
    const first = detection.result.suggestions[0];
    if (first && command.trim().length === 0) {
      applySuggestion(first, detection.result.name);
    } else if (detection.result.name) {
      const projectName = detection.result.name;
      setName((prev) => (prev.trim().length > 0 ? prev : projectName));
      if (isNew) {
        setId((prev) => (prev.trim().length > 0 ? prev : suggestAppId(projectName)));
      }
    }
    // applySuggestion / command 只在识别结果到达那一刻读取，不需要作为依赖重跑
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [detection]);

  const handlePickDirectory = async () => {
    try {
      const picked = await api.pickDirectory(cwd.trim() || undefined);
      if (picked) {
        autoApplyDir.current = picked.trim();
        setCwd(picked);
      }
    } catch {
      // 对话框失败不影响手动输入，静默即可
    }
  };

  const suggestions = detection?.result.suggestions ?? [];

  const idError = useMemo(() => validateAppId(id), [id]);
  const envParsed = useMemo(() => parseEnv(envText), [envText]);

  const nameError = name.trim().length === 0 ? "名称不能为空" : null;
  const commandError = command.trim().length === 0 ? "启动命令不能为空" : null;

  const hookTimeoutError = validateHookTimeout(hookTimeout);

  const errors = [idError, nameError, commandError, hookTimeoutError, ...envParsed.errors].filter(
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
        hooks: {
          before: parseArgs(beforeText),
          after: parseArgs(afterText),
          timeout_secs: hookTimeout.trim().length > 0 ? Number(hookTimeout.trim()) : null,
        },
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
        <Field
          label="工作目录"
          wide
          hint="选择项目文件夹后会自动识别项目类型，并给出可直接使用的启动命令。留空则继承启动器当前目录。"
        >
          <DirectoryInput
            className="input mono"
            value={cwd}
            onChange={(event) => setCwd(event.target.value)}
            placeholder="例如 E:/repo/app，或点右侧「选择…」"
            disabled={busy}
            spellCheck={false}
            autoComplete="off"
            onPick={handlePickDirectory}
            pickDisabled={busy || !isHosted()}
          />
        </Field>

        {detection && suggestions.length > 0 ? (
          <div className="field field-wide detect-panel" role="group" aria-label="识别到的启动命令">
            <div className="detect-head">
              <IconSparkle size={14} />
              <span>
                识别为 {detection.result.kinds.map(kindLabel).join(" + ")}，点击套用启动命令：
              </span>
            </div>
            <div className="detect-list">
              {suggestions.map((suggestion) => {
                const active = matchesSuggestion(suggestion, command, argsText);
                return (
                  <button
                    key={suggestion.source + suggestion.label}
                    type="button"
                    className={"btn btn-sm detect-chip" + (active ? " is-active" : "")}
                    aria-pressed={active}
                    title={`${suggestion.command} ${suggestion.args.join(" ")}\n来自 ${suggestion.source}`}
                    onClick={() => applySuggestion(suggestion, detection.result.name)}
                    disabled={busy}
                  >
                    {suggestion.label}
                  </button>
                );
              })}
            </div>
          </div>
        ) : null}
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

        <Field label="启动命令" required wide error={showError(commandError)}>
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

      <div className="form-grid">
        <Field
          label="启动前 hook（每行一条命令）"
          wide
          hint="启动主进程前按顺序在工作目录执行，任一条失败（非 0 退出或超时）即中止启动。"
        >
          <textarea
            className="input mono textarea"
            value={beforeText}
            onChange={(event) => setBeforeText(event.target.value)}
            placeholder={"git pull --ff-only\npnpm install"}
            rows={3}
            disabled={busy}
            spellCheck={false}
          />
        </Field>

        <Field
          label="启动后 hook（每行一条命令）"
          wide
          hint="主进程启动成功后在后台按顺序执行；失败只写入日志，不影响主进程。"
        >
          <textarea
            className="input mono textarea"
            value={afterText}
            onChange={(event) => setAfterText(event.target.value)}
            placeholder={"open http://localhost:5173"}
            rows={3}
            disabled={busy}
            spellCheck={false}
          />
        </Field>

        <Field label="hook 超时（秒）" error={showError(hookTimeoutError)} hint="单条命令的超时，留空默认 600 秒。">
          <input
            className="input mono"
            value={hookTimeout}
            onChange={(event) => setHookTimeout(event.target.value)}
            placeholder="600"
            inputMode="numeric"
            disabled={busy}
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