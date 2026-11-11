import { api } from "../lib/api";
import { useMemo, useState } from "react";
import { Icon } from "../components/Icon";
import { Modal } from "../components/Modal";
import { Sparkline } from "../components/Sparkline";
import { SearchInput } from "../components/Controls";
import { bytes, clock, latencyTone, protocolLabel, rate, transportLabel } from "../lib/format";
import { useLang, useStore, useT } from "../store";
import type { ConnectionState, NodeView, RoutingPolicy, TakeoverMode } from "../lib/types";

/** Circuit-board motif on the status card, drawn once. */
function Motif() {
  return (
    <svg className="status-motif" viewBox="0 0 620 260" preserveAspectRatio="xMidYMid slice" aria-hidden="true">
      <g stroke="#cfe6d6" strokeOpacity="0.22" fill="none" strokeWidth="1.2">
        <path d="M330 0v52l44 44v70" />
        <path d="M420 0v44l-36 36v56l40 40v84" />
        <path d="M500 0v70l-30 30v80l52 52v28" />
        <path d="M560 0v40l-24 24v60" />
        <path d="M390 260v-54l-34-34v-46" />
        <path d="M470 260v-40l36-36v-64" />
      </g>
      <g fill="#d9f6a9">
        <circle cx="374" cy="96" r="3.2" fillOpacity="0.85" />
        <circle cx="464" cy="126" r="2.6" fillOpacity="0.6" />
        <circle cx="522" cy="230" r="2.4" fillOpacity="0.5" />
      </g>
      <g stroke="#d9f6a9" strokeOpacity="0.5" fill="none" strokeWidth="1.2">
        <circle cx="464" cy="126" r="6.5" />
        <circle cx="374" cy="96" r="4.5" />
      </g>
    </svg>
  );
}

function phaseHeadline(t: (key: string) => string, phase: string): string {
  switch (phase) {
    case "connected":
      return t("connection.connected");
    case "connecting":
      return t("connection.connecting");
    case "disconnecting":
      return t("connection.disconnecting");
    case "failed":
      return t("connection.failed");
    default:
      return t("connection.disconnected");
  }
}

function phaseSub(t: (key: string) => string, phase: string, mode: TakeoverMode): string {
  if (phase === "connected" && mode === "manual") return t("connection.takeoverManual");
  switch (phase) {
    case "connected":
      return t("connection.connectedSub");
    case "connecting":
      return t("connection.connectingSub");
    case "failed":
      return t("connection.failedSub");
    default:
      return t("connection.disconnectedSub");
  }
}

function LanAccess({ lan }: { lan: NonNullable<ConnectionState["lan"]> }) {
  const t = useT();
  const [credentials, setCredentials] = useState<{ username: string; password: string }>();
  const [error, setError] = useState(false);
  return (
    <section className="panel" style={{ marginTop: 16 }}>
      <div className="panel-head">
        <h2 className="panel-title">{t("settings.allowLan")}</h2>
        <button type="button" className="btn btn-outline btn-sm" onClick={async () => {
          if (credentials) { setCredentials(undefined); return; }
          setError(false);
          try { setCredentials(await api.lanCredentials()); } catch { setError(true); }
        }}>{t(credentials ? "connection.hideLan" : "connection.showLan")}</button>
      </div>
      <div className="panel-body">
        <p className="mono">HTTP {lan.address}:{lan.httpPort} · SOCKS {lan.address}:{lan.socksPort}</p>
        <p className="field-hint">{t("connection.lanHint")}</p>
        {credentials ? <p className="mono">{credentials.username} / {credentials.password}</p> : null}
        {error ? <p role="alert">{t("connection.lanUnavailable")}</p> : null}
      </div>
    </section>
  );
}

function TrafficPanel() {
  const t = useT();
  const traffic = useStore((state) => state.traffic);
  const down = rate(traffic?.downBps ?? 0);
  const up = rate(traffic?.upBps ?? 0);
  return (
    <section className="panel">
      <div className="panel-head">
        <h2 className="panel-title">{t("connection.traffic")}</h2>
        <span className="panel-note">{t("connection.traffic.window")}</span>
      </div>
      <div className="panel-body" style={{ paddingTop: 14 }}>
        <div className="traffic-numbers">
          <div className="traffic-number">
            <span>{t("connection.down")}</span>
            <b>
              {down.value} <span style={{ fontSize: 14, fontWeight: 500 }}>{down.unit}</span>
            </b>
          </div>
          <div className="traffic-number">
            <span>{t("connection.up")}</span>
            <b>
              {up.value} <span style={{ fontSize: 14, fontWeight: 500 }}>{up.unit}</span>
            </b>
          </div>
        </div>
        <Sparkline samples={traffic?.samples} />
        <p className="traffic-foot">
          {t("connection.traffic.note")} · {bytes(traffic?.totalDownBytes ?? 0)} ↓ /{" "}
          {bytes(traffic?.totalUpBytes ?? 0)} ↑
        </p>
      </div>
    </section>
  );
}

export function ConnectionPage() {
  const t = useT();
  const lang = useLang();
  const snapshot = useStore((state) => state.snapshot);
  const busy = useStore((state) => state.busy);
  const connect = useStore((state) => state.connect);
  const disconnect = useStore((state) => state.disconnect);
  const retry = useStore((state) => state.retry);
  const installCore = useStore((state) => state.installCore);
  const switchNode = useStore((state) => state.switchNode);
  const setMode = useStore((state) => state.setMode);
  const setPolicy = useStore((state) => state.setPolicy);
  const navigate = useStore((state) => state.navigate);
  const [pickerOpen, setPickerOpen] = useState(false);
  const [query, setQuery] = useState("");

  const nodes = snapshot?.nodes ?? [];
  const connection = snapshot?.connection;
  const phase = connection?.phase ?? "disconnected";
  const mode = connection?.mode === "pac" ? "system-proxy" : connection?.mode ?? "system-proxy";
  const policy = connection?.policy ?? "rule";

  const candidates = useMemo(() => {
    const sorted = [...nodes].sort((a, b) => Number(b.favorite) - Number(a.favorite));
    return sorted.slice(0, 5);
  }, [nodes]);

  const selected = useMemo(() => {
    if (connection?.profileId) return nodes.find((node) => node.id === connection.profileId);
    return candidates[0];
  }, [connection?.profileId, candidates, nodes]);

  const filtered = useMemo(() => {
    const term = query.trim().toLowerCase();
    if (!term) return nodes.slice(0, 40);
    return nodes
      .filter((node) => `${node.remark} ${node.address} ${node.protocol}`.toLowerCase().includes(term))
      .slice(0, 40);
  }, [nodes, query]);

  if (!snapshot || !connection) {
    return <div className="page" />;
  }

  const isBusy = phase === "connecting" || phase === "disconnecting" || busy !== null;

  const headline = phase === "connected" && mode === "manual" ? t("connection.localOnly") : phase === "failed" && mode === "tun" ? t("connection.failedTun") : phaseHeadline(t, phase);
  const subline = phase === "failed" && mode === "tun" ? t("connection.failedTunSub") : phaseSub(t, phase, mode);

  const issue = connection.error ?? connection.takeoverError;
  const issueText = typeof issue === "string" ? issue : issue?.detail ?? (issue ? t(issue.key) : undefined);
  const requiredCore = phase === "failed" && typeof issue !== "string" && issue?.code === "core_missing"
    ? issue.requiredCore : undefined;
  const missingCore = requiredCore && !snapshot.cores.some((core) => core.kind === requiredCore && core.installed && !core.lastError)
    ? requiredCore : undefined;

  const primary =
    phase === "connected"
      ? { label: t("connection.disconnect"), action: () => void disconnect(), tone: "btn-primary" }
      : missingCore
        ? {
            label: busy === "install-core" ? t("connection.installingCore") : t("connection.installAndRetry", { core: missingCore }),
            action: async () => {
              if (await installCore(missingCore)) await retry();
            },
            tone: "btn-primary",
          }
      : phase === "failed"
        ? { label: t(connection.error?.code === "proxy_recovery_pending" ? "connection.retryRecovery" : "common.retry"), action: () => void retry(), tone: "btn-primary" }
        : {
            label: phase === "connecting" ? t("connection.connecting") : t("connection.connect"),
            action: () => {
              if (selected) void connect(selected.id, mode, policy);
            },
            tone: "btn-primary",
          };

  return (
    <div className="page">
      <div className="hero-grid">
        <section className="status-card" aria-live="polite">
          <Motif />
          <div className="status-eyebrow">01 / CONNECTION</div>
          <h2 className="status-headline">{headline}</h2>
          <p className="status-line">{subline}</p>
          <div className="status-node">
            {selected ? (
              <>
                <span>{connection.profileRemark ?? selected.remark}</span>
                <span className="sep">·</span>
                <span style={{ fontFamily: "var(--font-mono)", fontSize: 12 }}>
                  {protocolLabel(selected.protocol)} · {transportLabel(selected.transport)}
                  {connection.core ? ` · ${connection.core}` : ""}
                </span>
              </>
            ) : (
              <span>{t("connection.noNodesHint")}</span>
            )}
          </div>
          {issueText ? <p className="status-error">{issueText}</p> : null}
          <div className="status-actions">
            <button
              type="button"
              className={`btn ${primary.tone}`}
              onClick={primary.action}
              disabled={isBusy || (!selected && phase !== "failed")}
            >
              {phase === "connecting" || busy === "install-core" ? <Icon name="refresh" size={15} className="spin" /> : null}
              {primary.label}
            </button>
            <button type="button" className="btn btn-outline" onClick={() => setPickerOpen(true)} disabled={nodes.length === 0}>
              {t("connection.changeNode")}
            </button>
          </div>
        </section>

        <TrafficPanel />
      </div>

      {phase === "connected" && connection.lan
        ? <LanAccess key={connection.startedAt} lan={connection.lan} /> : null}

      <section className="panel" style={{ marginTop: 16 }}>
        <div className="panel-body">
          <div className="form-grid">
            <div>
              <label className="field-label" htmlFor="access-mode">
                {t("connection.mode")}
              </label>
              <p className="field-hint">{t("connection.mode.hint")}</p>
              <select
                id="access-mode"
                className="control"
                style={{ marginTop: 10 }}
                value={mode}
                onChange={(event) => void setMode(event.target.value as TakeoverMode)}
              >
                <option value="system-proxy">{t("connection.mode.system")}</option>
                <option value="manual">{t("connection.mode.manual")}</option>
                {(snapshot.tunAvailable || mode === "tun") ? <option value="tun" disabled={!snapshot.tunAvailable}>{t("connection.mode.tun")}</option> : null}
              </select>
              {mode === "tun" ? <p className="field-hint">{t(snapshot.tunRuleRouting ? "connection.mode.tunRuleHint" : "connection.mode.tunHint")}</p> : null}
              {mode === "system-proxy" && policy === "rule" ? <p className="field-hint">{t("connection.mode.pacHint")}</p> : null}
            </div>
            <div>
              <label className="field-label" htmlFor="routing-policy">
                {t("connection.policy")}
              </label>
              <p className="field-hint">{t("connection.policy.hint")}</p>
              <select
                id="routing-policy"
                className="control"
                style={{ marginTop: 10 }}
                value={policy}
                disabled={mode === "tun" && !snapshot.tunRuleRouting}
                onChange={(event) => void setPolicy(event.target.value as RoutingPolicy)}
              >
                {mode !== "tun" || snapshot.tunRuleRouting ? <option value="rule">{t("connection.policy.rule")}</option> : null}
                <option value="global">{t("connection.policy.global")}</option>
                {mode !== "tun" ? <option value="direct">{t("connection.policy.direct")}</option> : null}
              </select>
              {snapshot.connection.inboundPort ? (
                <p className="field-hint mono" style={{ marginTop: 8 }}>
                  {snapshot.connection.mode === "system-proxy" || snapshot.connection.mode === "pac" ? "HTTP/SOCKS" : "SOCKS"} 127.0.0.1:
                  {snapshot.connection.inboundPort}
                  {snapshot.connection.apiPort ? ` · API ${snapshot.connection.apiPort}` : ""}
                </p>
              ) : null}
              {snapshot.connection.pacUrl ? (
                <p className="field-hint mono" style={{ marginTop: 6 }}>
                  PAC {snapshot.connection.pacUrl}
                </p>
              ) : null}
            </div>
          </div>
        </div>
      </section>

      <section className="panel" style={{ marginTop: 16 }}>
        <div className="panel-head">
          <h2 className="panel-title">{t("connection.favorite")}</h2>
          <button type="button" className="btn btn-outline btn-sm" onClick={() => navigate("nodes")}>
            {t("connection.allNodes")} <Icon name="arrow-right" size={13} />
          </button>
        </div>
        <div>
          {candidates.length === 0 ? (
            <div className="empty">
              <b>{t("connection.noNodes")}</b>
              {t("connection.noNodesHint")}
            </div>
          ) : (
            candidates.map((node) => (
              <div
                key={node.id}
                className="row"
                style={{ gridTemplateColumns: "minmax(0,1fr) 80px 96px" }}
              >
                <div style={{ minWidth: 0 }}>
                  <div className="node-name">
                    {node.favorite ? <Icon name="star" size={12} style={{ color: "var(--warn)" }} /> : null}
                    <span>{node.remark}</span>
                  </div>
                  <div className="node-sub">
                    {protocolLabel(node.protocol)} · {transportLabel(node.transport)} · {connection.profileId === node.id ? connection.core ?? node.core : node.core}
                  </div>
                </div>
                <div className="latency" data-tone={latencyTone(node.latency?.ms)}>
                  {node.latency ? `${node.latency.ms} ms` : "—"}
                </div>
                {connection.profileId === node.id ? (
                  <span className="badge" data-tone="active" style={{ justifySelf: "end" }}>
                    <Icon name="check" size={12} /> {t("connection.inUse")}
                  </span>
                ) : (
                  <button
                    type="button"
                    className="btn btn-outline btn-sm"
                    style={{ justifySelf: "end" }}
                    disabled={isBusy}
                    onClick={() => void switchNode(node.id)}
                  >
                    {t("connection.switch")}
                  </button>
                )}
              </div>
            ))
          )}
        </div>
        <div className="panel-body" style={{ borderTop: "1px solid var(--line)", paddingTop: 12 }}>
          <p className="field-hint" style={{ marginTop: 0 }}>
            {t("connection.nodeCount", { n: nodes.length })}
            {connection.startedAt ? ` · 连接于 ${clock(connection.startedAt, lang)}` : ""}
          </p>
        </div>
      </section>

      <div className="origin-note">
        <span>v2rayR / {t("connection.title")}</span>
        <span className="mono">API v{snapshot.version.apiVersion} · schema {snapshot.version.settingsSchema}</span>
      </div>

      {pickerOpen ? (
        <NodePicker
          nodes={filtered}
          query={query}
          onQuery={setQuery}
          currentId={connection.profileId}
          onPick={(node) => {
            setPickerOpen(false);
            setQuery("");
            void switchNode(node.id);
          }}
          onClose={() => setPickerOpen(false)}
        />
      ) : null}
    </div>
  );
}

function NodePicker({
  nodes,
  query,
  onQuery,
  currentId,
  onPick,
  onClose,
}: {
  nodes: NodeView[];
  query: string;
  onQuery: (value: string) => void;
  currentId?: string;
  onPick: (node: NodeView) => void;
  onClose: () => void;
}) {
  const t = useT();
  return (
    <Modal title={t("connection.changeNode")} subtitle={t("nodes.search")} onClose={onClose}>
      <SearchInput value={query} onChange={onQuery} placeholder={t("nodes.search")} />
      <div style={{ marginTop: 12, maxHeight: 360, overflowY: "auto" }}>
        {nodes.length === 0 ? (
          <div className="empty">
            <b>{t("nodes.empty")}</b>
            {t("nodes.emptyHint")}
          </div>
        ) : (
          nodes.map((node) => (
            <button
              key={node.id}
              type="button"
              className="row"
              style={{
                gridTemplateColumns: "minmax(0,1fr) 70px 18px",
                width: "100%",
                background: "transparent",
                border: "0",
                borderTop: "1px solid var(--line)",
                textAlign: "left",
                cursor: "pointer",
              }}
              onClick={() => onPick(node)}
            >
              <div style={{ minWidth: 0 }}>
                <div className="node-name">
                  <span>{node.remark}</span>
                </div>
                <div className="node-sub">
                  {protocolLabel(node.protocol)} · {node.address}:{node.port}
                </div>
              </div>
              <div className="latency" data-tone={latencyTone(node.latency?.ms)}>
                {node.latency ? `${node.latency.ms} ms` : "—"}
              </div>
              {currentId === node.id ? <Icon name="check" size={14} /> : null}
            </button>
          ))
        )}
      </div>
    </Modal>
  );
}
