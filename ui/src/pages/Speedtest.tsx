import { useMemo } from "react";
import { Icon } from "../components/Icon";
import { latencyTone, protocolLabel, relativeTime } from "../lib/format";
import { useLang, useStore, useT } from "../store";

export function SpeedtestPage() {
  const t = useT();
  const lang = useLang();
  const snapshot = useStore((state) => state.snapshot);
  const testing = useStore((state) => state.testing);
  const progress = useStore((state) => state.testProgress);
  const testLatency = useStore((state) => state.testLatency);
  const lastResults = useStore((state) => state.lastResults);

  const rows = useMemo(() => {
    const nodes = snapshot?.nodes ?? [];
    const failed = new Map(lastResults.filter((result) => result.error).map((result) => [result.id, result.error!]));
    return nodes
      .map((node) => ({ node, error: failed.get(node.id) }))
      .sort((a, b) => {
        const aMs = a.node.latency?.ms ?? Number.POSITIVE_INFINITY;
        const bMs = b.node.latency?.ms ?? Number.POSITIVE_INFINITY;
        return aMs - bMs;
      });
  }, [snapshot?.nodes, lastResults]);

  if (!snapshot) return <div className="page" />;
  const measured = rows.filter((row) => row.node.latency).length;

  return (
    <div className="page">
      <div className="eyebrow">{t("speed.eyebrow")}</div>
      <h1 className="page-title">{t("speed.title")}</h1>
      <p className="page-sub">{t("speed.subtitle")}</p>

      <div className="toolbar">
        <div className="grow" />
        <span className="chip">
          <span className="dot" style={{ background: measured ? "var(--ok)" : "var(--line-strong)" }} />
          {measured} / {rows.length}
        </span>
        <button type="button" className="btn btn-solid" disabled={testing || rows.length === 0} onClick={() => void testLatency([])}>
          <Icon name="gauge" size={15} className={testing ? "spin" : undefined} />
          {testing ? t("speed.running") : t("speed.run")}
        </button>
      </div>

      {testing ? (
        <div className="panel" style={{ padding: 16, marginBottom: 14 }}>
          <div style={{ display: "flex", justifyContent: "space-between", fontSize: 12, color: "var(--muted)", marginBottom: 8 }}>
            <span>{t("speed.running")}</span>
            <span className="mono">
              {progress.done}/{progress.total}
            </span>
          </div>
          <div className="quota">
            <i style={{ width: `${progress.total ? (progress.done / progress.total) * 100 : 0}%`, transition: "width 200ms" }} />
          </div>
        </div>
      ) : null}

      <section className="panel">
        <div className="row row-head" style={{ gridTemplateColumns: "minmax(0,1fr) 110px 110px 110px" }}>
          <span>{t("speed.col.node")}</span>
          <span style={{ textAlign: "right" }}>{t("speed.col.latency")}</span>
          <span style={{ textAlign: "right" }}>{t("speed.col.method")}</span>
          <span style={{ textAlign: "right" }}>{t("speed.col.tested")}</span>
        </div>
        {rows.length === 0 ? (
          <div className="empty">
            <b>{t("speed.empty")}</b>
            {t("speed.emptyHint")}
          </div>
        ) : (
          rows.map(({ node, error }) => (
            <div key={node.id} className="row" style={{ gridTemplateColumns: "minmax(0,1fr) 110px 110px 110px" }}>
              <div style={{ minWidth: 0 }}>
                <div className="node-name">
                  <span>{node.remark}</span>
                </div>
                <div className="node-sub">
                  {protocolLabel(node.protocol)} · {node.address}:{node.port}
                </div>
              </div>
              <div style={{ textAlign: "right" }} className="latency" data-tone={error ? "bad" : latencyTone(node.latency?.ms)}>
                {error ? t("speed.failed") : node.latency ? `${node.latency.ms} ms` : "—"}
              </div>
              <div style={{ textAlign: "right", fontSize: 12, color: "var(--muted)" }}>
                {node.latency ? t(`speed.method.${node.latency.method === "through-kernel" ? "kernel" : node.latency.method}`) : "—"}
              </div>
              <div style={{ textAlign: "right", fontSize: 12, color: "var(--muted)" }}>
                {node.latency ? relativeTime(node.latency.testedAt, lang) : "—"}
              </div>
            </div>
          ))
        )}
      </section>

      <div className="origin-note">
        <span>{t("speed.legend")}</span>
        <span className="mono">{t("settings.concurrency")} ≤ 16</span>
      </div>
    </div>
  );
}
