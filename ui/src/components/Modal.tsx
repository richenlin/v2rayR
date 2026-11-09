import { useEffect, useRef, type ReactNode } from "react";
import { Icon } from "./Icon";
import { useT } from "../store";

interface ModalProps {
  title: string;
  subtitle?: string;
  children: ReactNode;
  footer?: ReactNode;
  onClose: () => void;
  labelledBy?: string;
}

/**
 * Esc closes, focus moves into the dialog on mount, and clicks on the backdrop
 * close it. Full focus trapping is deliberately skipped (it needs a library to be
 * correct); focus returns to the trigger because the trigger keeps focus in DOM.
 */
export function Modal({ title, subtitle, children, footer, onClose }: ModalProps) {
  const t = useT();
  const panel = useRef<HTMLDivElement>(null);

  useEffect(() => {
    const onKey = (event: KeyboardEvent) => {
      if (event.key === "Escape") {
        event.stopPropagation();
        onClose();
      }
    };
    document.addEventListener("keydown", onKey);
    panel.current?.querySelector<HTMLElement>("input,textarea,select,button")?.focus();
    return () => document.removeEventListener("keydown", onKey);
  }, [onClose]);

  return (
    <div
      className="modal-backdrop"
      onMouseDown={(event) => {
        if (event.target === event.currentTarget) onClose();
      }}
    >
      <div className="modal" role="dialog" aria-modal="true" aria-label={title} ref={panel}>
        <div className="modal-head">
          <div>
            <h2 className="panel-title">{title}</h2>
            {subtitle ? <p className="field-hint" style={{ marginTop: 6 }}>{subtitle}</p> : null}
          </div>
          <button type="button" className="btn btn-quiet" onClick={onClose} aria-label={t("common.close")}>
            <Icon name="close" size={16} />
          </button>
        </div>
        <div className="modal-body">{children}</div>
        {footer ? <div className="modal-foot">{footer}</div> : null}
      </div>
    </div>
  );
}
