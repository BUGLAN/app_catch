/**
 * 通用展示组件：空态 / 提示条 / 骨架屏 / 分段控件 / 键值表。
 *
 * 抽出来的目的：空态原来有三份各写一套的样式与文案结构，字号/间距/对齐各不相同。
 * 这里统一成一个可组合的形态。
 */
import { Fragment, forwardRef, useId, type ReactNode } from "react";

import { IconApp, IconInfo } from "./Icons";

/* -------------------------------------------------------------------------- */
/* 空态                                                                        */
/* -------------------------------------------------------------------------- */

interface EmptyStateProps {
  title: string;
  description?: ReactNode;
  icon?: ReactNode;
  /** 主要动作（通常是一个 btn btn-primary）。 */
  action?: ReactNode;
  /** 次级说明，用更弱的色。 */
  hint?: ReactNode;
}

/** 主区级空态：居中、有图标、可带主行动。 */
export function EmptyState({ title, description, icon, action, hint }: EmptyStateProps) {
  return (
    <div className="empty-state">
      <div className="empty-state-icon">{icon ?? <IconApp size={20} />}</div>
      <h2>{title}</h2>
      {description ? <p>{description}</p> : null}
      {action}
      {hint ? <p className="small muted">{hint}</p> : null}
    </div>
  );
}

interface EmptyNoticeProps {
  children: ReactNode;
  icon?: ReactNode;
}

/** 面板内空态：一行说明，不撑开大块留白。 */
export function EmptyNotice({ children, icon }: EmptyNoticeProps) {
  return (
    <p className="empty-notice">
      {icon ?? <IconInfo size={14} />}
      <span>{children}</span>
    </p>
  );
}

/* -------------------------------------------------------------------------- */
/* 提示条                                                                      */
/* -------------------------------------------------------------------------- */

interface BannerProps {
  tone: "info" | "warn" | "error";
  children: ReactNode;
}

/** 会话级横幅：信息 / 警告 / 错误。图标 + 文字，颜色不是唯一编码。 */
export function Banner({ tone, children }: BannerProps) {
  return (
    <p className={"banner banner-" + tone} role={tone === "error" ? "alert" : "status"}>
      <IconInfo size={14} />
      <span>{children}</span>
    </p>
  );
}

/* -------------------------------------------------------------------------- */
/* 骨架屏                                                                      */
/* -------------------------------------------------------------------------- */

/** 列表骨架：应用列表加载中使用，避免"空列表"与"还没加载"混淆。 */
export function ListSkeleton({ rows = 6, collapsed = false }: { rows?: number; collapsed?: boolean }) {
  if (collapsed) {
    return (
      <div className="skeleton-list skeleton-list-rail" aria-hidden="true">
        {Array.from({ length: rows }, (_, index) => (
          <span key={index} className="skeleton skeleton-rail-icon" />
        ))}
      </div>
    );
  }
  return (
    <div className="skeleton-list" aria-hidden="true">
      {Array.from({ length: rows }, (_, index) => (
        <div className="skeleton-row" key={index}>
          <span className="skeleton skeleton-app-icon" />
          <span className="skeleton-lines">
            <span className="skeleton skeleton-line-name" />
            <span className="skeleton skeleton-line-meta" />
          </span>
        </div>
      ))}
    </div>
  );
}

/** 主区骨架：首屏加载中使用。 */
export function PanelSkeleton() {
  return (
    <div className="skeleton-panel" aria-hidden="true">
      <div className="skeleton-panel-head">
        <span className="skeleton skeleton-panel-icon" />
        <span className="skeleton-lines">
          <span className="skeleton skeleton-line-title" />
          <span className="skeleton skeleton-line-sub" />
        </span>
      </div>
      {Array.from({ length: 4 }, (_, index) => (
        <span
          key={index}
          className="skeleton skeleton-line-body"
          data-tone={index % 2 === 0 ? "wide" : "narrow"}
        />
      ))}
    </div>
  );
}

/* -------------------------------------------------------------------------- */
/* 分段控件（Tabs）                                                            */
/* -------------------------------------------------------------------------- */

export interface SegmentedOption<T extends string> {
  value: T;
  label: string;
}

interface SegmentedProps<T extends string> {
  /** 无障碍名称：读屏会播报 "实时日志/最近事件, 选项卡列表"。 */
  ariaLabel: string;
  value: T;
  options: SegmentedOption<T>[];
  onChange: (value: T) => void;
  /**
   * 面板 id 前缀。分段控件负责生成 `<prefix>-panel-<value>`，
   * 并用 aria-controls 关联；内容区用 {@link TabPanel} 渲染即可自动对上 id。
   */
  panelId: string;
}

/**
 * 与 {@link Segmented} 配套的内容面板。
 *
 * 单独抽出来是为了让 id 的生成规则只有一处（Segmented 的 `panelId` 前缀）：
 * 两边各写一遍字符串是最容易错位的地方（aria-controls 指向不存在的节点时，
 * 读屏不会报错，只是静默失效）。
 *
 * 只有当前选中的面板才渲染 —— 与原来的条件渲染行为一致，不做额外的 DOM 保留。
 * 每次切换都是新挂载，`.tab-panel` 的淡入关键帧因此自然播放（见 primitives.css）。
 */
interface TabPanelProps extends React.HTMLAttributes<HTMLDivElement> {
  panelId: string;
  value: string;
  active: boolean;
  label: string;
  children: ReactNode;
}

export const TabPanel = forwardRef<HTMLDivElement, TabPanelProps>(function TabPanel(
  { panelId, value, active, label, className, children, ...rest },
  ref
) {
  if (!active) {
    return null;
  }
  return (
    <div
      id={panelId + "-panel-" + value}
      role="tabpanel"
      aria-label={label}
      className={className ? "tab-panel " + className : "tab-panel"}
      tabIndex={0}
      ref={ref}
      {...rest}
    >
      {children}
    </div>
  );
});

/**
 * 分段控件。
 *
 * 交互语义按 WAI-ARIA Tabs 实现：左右方向键切换、Home/End 跳首尾，
 * 只有当前项 tabIndex=0（roving tabindex），避免 Tab 键在三个按钮间来回跳。
 */
export function Segmented<T extends string>({
  ariaLabel,
  value,
  options,
  onChange,
  panelId,
}: SegmentedProps<T>) {
  const baseId = useId();

  const handleKeyDown = (event: React.KeyboardEvent<HTMLDivElement>) => {
    const index = options.findIndex((option) => option.value === value);
    if (index < 0) {
      return;
    }
    let next = index;
    if (event.key === "ArrowRight" || event.key === "ArrowDown") {
      next = (index + 1) % options.length;
    } else if (event.key === "ArrowLeft" || event.key === "ArrowUp") {
      next = (index - 1 + options.length) % options.length;
    } else if (event.key === "Home") {
      next = 0;
    } else if (event.key === "End") {
      next = options.length - 1;
    } else {
      return;
    }
    event.preventDefault();
    onChange(options[next]!.value);
    const target = event.currentTarget.querySelectorAll<HTMLButtonElement>("[role=tab]")[next];
    target?.focus();
  };

  return (
    <div
      className="segmented"
      role="tablist"
      aria-label={ariaLabel}
      onKeyDown={handleKeyDown}
    >
      {options.map((option, index) => {
        const active = option.value === value;
        return (
          <button
            key={option.value}
            type="button"
            role="tab"
            id={baseId + "-tab-" + index}
            className={"seg" + (active ? " is-active" : "")}
            aria-selected={active}
            aria-controls={panelId + "-panel-" + option.value}
            tabIndex={active ? 0 : -1}
            onClick={() => onChange(option.value)}
          >
            {option.label}
          </button>
        );
      })}
    </div>
  );
}

/* -------------------------------------------------------------------------- */
/* 键值表                                                                      */
/* -------------------------------------------------------------------------- */

export interface KeyValueItem {
  /** 左侧标签。 */
  label: string;
  /** 右侧内容。 */
  value: ReactNode;
}

/** 详情用的键值表：dt/dd 语义，窄屏自动换列宽。 */
export function KeyValueList({ items }: { items: KeyValueItem[] }) {
  return (
    <dl className="kv">
      {items.map((item) => (
        <Fragment key={item.label}>
          <dt>{item.label}</dt>
          <dd>{item.value}</dd>
        </Fragment>
      ))}
    </dl>
  );
}
