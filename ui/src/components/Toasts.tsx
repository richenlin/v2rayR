import { Icon } from "./Icon";
import { useStore } from "../store";

export function Toasts() {
  const toasts = useStore((state) => state.toasts);
  const dismiss = useStore((state) => state.dismissToast);
  return (
    <div className="toasts" role="status" aria-live="polite">
      {toasts.map((toast) => (
        <div key={toast.id} className="toast" data-tone={toast.tone}>
          <Icon
            name={toast.tone === "danger" ? "alert" : toast.tone === "ok" ? "check" : "spark"}
            size={15}
            style={{ marginTop: 2, flex: "none" }}
          />
          <div style={{ minWidth: 0 }}>
            <b>{toast.title}</b>
            {toast.detail ? <span style={{ overflowWrap: "anywhere" }}>{toast.detail}</span> : null}
          </div>
          <button
            type="button"
            className="btn btn-quiet"
            style={{ marginLeft: "auto", height: 22, padding: "0 4px" }}
            aria-label="close"
            onClick={() => dismiss(toast.id)}
          >
            <Icon name="close" size={13} />
          </button>
        </div>
      ))}
    </div>
  );
}
