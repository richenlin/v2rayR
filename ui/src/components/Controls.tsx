import { useId, type ReactNode } from "react";
import { Icon } from "./Icon";
import { useT } from "../store";

export function Toggle({
  checked,
  onChange,
  label,
  disabled,
}: {
  checked: boolean;
  onChange: (checked: boolean) => void;
  label: string;
  disabled?: boolean;
}) {
  return (
    <button
      type="button"
      role="switch"
      aria-checked={checked}
      aria-label={label}
      className="switch"
      disabled={disabled}
      onClick={() => onChange(!checked)}
    />
  );
}

export function Field({
  label,
  hint,
  children,
  disabled,
  inline,
  block,
  htmlFor,
}: {
  label: string;
  hint?: ReactNode;
  children: ReactNode;
  disabled?: boolean;
  inline?: boolean;
  block?: boolean;
  htmlFor?: string;
}) {
  return (
    <div
      className={`field${disabled ? " is-disabled" : ""}${block ? " field-block" : ""}`}
      data-field-label={label}
    >
      <div>
        <label className="field-label" htmlFor={htmlFor}>
          {label}
        </label>
        {hint ? <p className="field-hint">{hint}</p> : null}
      </div>
      <div className={`field-control${inline ? " is-inline" : ""}`}>{children}</div>
    </div>
  );
}

export function SearchInput({
  value,
  onChange,
  placeholder,
  ariaLabel,
}: {
  value: string;
  onChange: (value: string) => void;
  placeholder: string;
  ariaLabel?: string;
}) {
  return (
    <div className="input-search">
      <Icon name="search" size={15} />
      <input
        className="control"
        type="search"
        value={value}
        placeholder={placeholder}
        aria-label={ariaLabel ?? placeholder}
        onChange={(event) => onChange(event.target.value)}
      />
    </div>
  );
}

export function Select<T extends string>({
  value,
  onChange,
  options,
  label,
  disabled,
  id,
}: {
  value: T;
  onChange: (value: T) => void;
  options: Array<{ value: T; label: string }>;
  label: string;
  disabled?: boolean;
  id?: string;
}) {
  const fallbackId = useId();
  return (
    <select
      id={id ?? fallbackId}
      className="control"
      value={value}
      disabled={disabled}
      aria-label={label}
      onChange={(event) => onChange(event.target.value as T)}
    >
      {options.map((option) => (
        <option key={option.value} value={option.value} disabled={option.value === "__disabled"}>
          {option.label}
        </option>
      ))}
    </select>
  );
}

export function ConfirmButton({
  onConfirm,
  children,
  className = "btn btn-outline btn-sm",
  title,
}: {
  onConfirm: () => void;
  children: ReactNode;
  className?: string;
  title?: string;
}) {
  const t = useT();
  return (
    <button
      type="button"
      className={className}
      title={title}
      onClick={() => {
        if (window.confirm(t("common.confirm") + "?")) onConfirm();
      }}
    >
      {children}
    </button>
  );
}
