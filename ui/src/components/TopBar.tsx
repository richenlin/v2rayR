import { Icon } from "./Icon";
import { isPreview } from "../lib/api";
import { useStore, useT } from "../store";

const pageKey: Record<string, string> = {
  connection: "nav.connection",
  nodes: "nav.nodes",
  subscriptions: "nav.subscriptions",
  speedtest: "nav.speedtest",
  settings: "nav.settings",
};

export function TopBar() {
  const t = useT();
  const page = useStore((state) => state.page);
  const connection = useStore((state) => state.snapshot?.connection);
  const warnings = useStore((state) => state.snapshot?.warnings);

  const tone = connection?.phase === "failed" ? "danger" : (warnings?.length ?? 0) ? "warn" : "ok";
  return (
    <header className="topbar">
      <div className="crumb">
        <span>{t("nav.workspace")}</span>
        <span className="crumb-sep">/</span>
        <strong>{t(pageKey[page])}</strong>
      </div>
      <div className="chip" data-tone={isPreview ? "warn" : tone} title={isPreview ? t("chip.previewHint") : undefined}>
        <span className="dot" />
        {isPreview ? t("chip.preview") : t("chip.live")}
        {!isPreview && (warnings?.length ?? 0) ? ` · ${(warnings?.length ?? 0)}` : ""}
        {isPreview ? <Icon name="spark" size={12} /> : null}
      </div>
    </header>
  );
}
