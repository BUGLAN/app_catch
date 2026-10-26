/**
 * 表单域：把「标签 / 控件 / 提示 / 错误」四件事绑在一起。
 *
 * 原来每个字段都是手写 <label> + <input> + 条件渲染的 span，
 * 问题有三个：
 *   1. 错误只改边框颜色，读屏完全拿不到（没有 aria-invalid / aria-describedby）；
 *   2. 错误出现/消失会让整页高度跳动（消息行不留位）；
 *   3. 标签与控件的关联靠 label 包裹，多个控件时浏览器会把提示文字也算进标签名。
 *
 * 这里统一用 useId 生成 id，显式做 aria-describedby 与 aria-invalid。
 */
import { cloneElement, useId, type ReactElement, type ReactNode } from "react";

interface FieldProps {
  label: string;
  /** 是否必填：会渲染星号并给控件加 aria-required。 */
  required?: boolean;
  /** 常驻提示（与错误互斥显示，但占位一致，避免跳动）。 */
  hint?: ReactNode;
  /** 校验错误：非空即视为无效。 */
  error?: string | null;
  /** 单控件时把它传进来，Field 负责注入 id / aria-*。 */
  children: ReactElement<Record<string, unknown>>;
  /** 跨两列。 */
  wide?: boolean;
}

export function Field({ label, required, hint, error, children, wide }: FieldProps) {
  const id = useId();
  const messageId = id + "-message";
  const invalid = Boolean(error);
  const message = invalid ? error : hint;

  const control = cloneElement(children, {
    id,
    "aria-invalid": invalid || undefined,
    "aria-required": required || undefined,
    "aria-describedby": message ? messageId : undefined,
  });

  return (
    <div className={"field" + (wide ? " field-wide" : "")}>
      <label className="field-label" htmlFor={id}>
        {label}
        {required ? (
          <span className="required" aria-hidden="true">
            *
          </span>
        ) : null}
        {required ? <span className="sr-only">（必填）</span> : null}
      </label>
      {control}
      {/*
        消息区始终占一行：错误与提示共用同一个节点，既有内容时不跳动，
        又用 role=alert 让读屏在错误出现时立刻播报。
      */}
      <span
        id={messageId}
        className={"field-message" + (invalid ? " is-error" : "")}
        role={invalid ? "alert" : undefined}
      >
        {message}
      </span>
    </div>
  );
}
