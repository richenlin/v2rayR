import { useState } from "react";
import { Icon } from "../components/Icon";
import { Modal } from "../components/Modal";
import { bytes, relativeTime, shortDate } from "../lib/format";
import { useLang, useStore, useT } from "../store";
import type { SubscriptionView } from "../lib/types";

export function SubscriptionsPage() {
  const t = useT();
  const lang = useLang();
  const snapshot = useStore((state) => state.snapshot);
  const busy = useStore((state) => state.busy);
  const addSubscription = useStore((state) => state.addSubscription);
  const refreshSubscription = useStore((state) => state.refreshSubscription);
  const removeSubscription = useStore((state) => state.removeSubscription);
  const setEnabled = useStore((state) => state.setSubscriptionEnabled);
  const [addOpen, setAddOpen] = useState(false);

  if (!snapshot) return <div className="page" />;
  const subscriptions = snapshot.subscriptions;

  return (
    <div className="page">
      <div className="eyebrow">{t("subs.eyebrow")}</div>
      <h1 className="page-title">{t("subs.title")}</h1>
      <p className="page-sub">{t("subs.subtitle")}</p>

      <div className="toolbar">
        <div className="grow" />
        <span className="chip">
          <span className="dot" style={{ background: "var(--primary)" }} />
          {t("connection.nodeCount", { n: snapshot.nodes.length })}
        </span>
        <button type="button" className="btn btn-solid" onClick={() => setAddOpen(true)}>
          <Icon name="plus" size={15} />
          {t("subs.add")}
        </button>
      </div>

      {subscriptions.length === 0 ? (
        <section className="panel">
          <div className="empty">
            <b>{t("subs.empty")}</b>
            {t("subs.emptyHint")}
          </div>
        </section>
      ) : (
        <div style={{ display: "flex", flexDirection: "column", gap: 12 }}>
          {subscriptions.map((subscription) => (
            <section key={subscription.id} className="panel">
              <div className="sub-card">
                <div style={{ minWidth: 0 }}>
                  <div style={{ display: "flex", alignItems: "center", gap: 8, flexWrap: "wrap" }}>
                    <h2 className="panel-title" style={{ fontSize: 16 }}>{subscription.name}</h2>
                    <span className="badge" data-tone={subscription.enabled ? "ok" : undefined}>
                      {subscription.enabled ? t("subs.enabled") : t("subs.disabled")}
                    </span>
                    {subscription.format ? <span className="badge">{subscription.format}</span> : null}
                    {subscription.autoUpdate ? (
                      <span className="badge">{t("subs.interval", { n: subscription.intervalHours })}</span>
                    ) : null}
                  </div>
                  <div className="node-sub" style={{ marginTop: 6 }}>
                    {subscription.urlHost} · {subscription.url.replace(/\/\/[^@/]+@/, "//***@")}
                  </div>
                  <div className="kv" style={{ marginTop: 12 }}>
                    <span>{t("subs.nodes", { n: subscription.nodeCount })}</span>
                    <span>
                      {t("subs.lastUpdate")}：{relativeTime(subscription.lastUpdatedAt, lang)}
                    </span>
                    {subscription.enabled ? (
                      <span>
                        {t("subs.nextUpdate")}：{subscription.nextUpdateAt ? shortDate(subscription.nextUpdateAt, lang) : "—"}
                      </span>
                    ) : null}
                  </div>
                  {subscription.quota && subscription.quota.total > 0 ? (
                    <div style={{ marginTop: 14, maxWidth: 420 }}>
                      <div style={{ display: "flex", justifyContent: "space-between", fontSize: 12, color: "var(--muted)", marginBottom: 6 }}>
                        <span>
                          {t("subs.quotaUsed", {
                            used: bytes(subscription.quota.upload + subscription.quota.download),
                            total: bytes(subscription.quota.total),
                          })}
                        </span>
                        {subscription.quota.expire ? (
                          <span>{t("subs.quotaExpire", { date: shortDate(subscription.quota.expire, lang) })}</span>
                        ) : null}
                      </div>
                      <div className="quota">
                        <i
                          style={{
                            width: `${Math.min(
                              100,
                              ((subscription.quota.upload + subscription.quota.download) / subscription.quota.total) * 100,
                            ).toFixed(1)}%`,
                          }}
                        />
                      </div>
                    </div>
                  ) : null}
                  {subscription.lastError ? (
                    <p style={{ marginTop: 12, color: "var(--danger)", fontSize: 12 }}>
                      {t("subs.error", { message: subscription.lastError.detail ?? subscription.lastError.code })}
                    </p>
                  ) : null}
                </div>
                <div className="sub-actions">
                  <button
                    type="button"
                    className="btn btn-outline btn-sm"
                    disabled={busy === `subscription-${subscription.id}`}
                    onClick={() => void refreshSubscription(subscription.id)}
                  >
                    <Icon
                      name="refresh"
                      size={13}
                      className={busy === `subscription-${subscription.id}` ? "spin" : undefined}
                    />
                    {busy === `subscription-${subscription.id}` ? t("subs.updating") : t("subs.update")}
                  </button>
                  <button
                    type="button"
                    className="btn btn-outline btn-sm"
                    aria-pressed={subscription.enabled}
                    onClick={() => void setEnabled(subscription.id, !subscription.enabled)}
                  >
                    {subscription.enabled ? t("subs.disable") : t("subs.enable")}
                  </button>
                  <button
                    type="button"
                    className="btn btn-quiet btn-sm"
                    aria-label={t("common.delete")}
                    onClick={() => {
                      if (window.confirm(t("subs.removeConfirm", { name: subscription.name }))) {
                        void removeSubscription(subscription.id);
                      }
                    }}
                  >
                    <Icon name="trash" size={14} />
                  </button>
                </div>
              </div>
            </section>
          ))}
        </div>
      )}

      <div className="origin-note">
        <span>v2rayR / {t("subs.title")}</span>
        <span className="mono">{t("subs.footnote")}</span>
      </div>

      {addOpen ? (
        <AddSubscriptionModal
          onClose={() => setAddOpen(false)}
          onSubmit={async (url, name) => {
            await addSubscription(url, name);
            setAddOpen(false);
          }}
        />
      ) : null}
    </div>
  );
}

function AddSubscriptionModal({
  onClose,
  onSubmit,
}: {
  onClose: () => void;
  onSubmit: (url: string, name?: string) => Promise<void>;
}) {
  const t = useT();
  const [url, setUrl] = useState("");
  const [name, setName] = useState("");
  const [busy, setBusy] = useState(false);
  const valid = /^https?:\/\//i.test(url.trim());

  return (
    <Modal
      title={t("subs.add")}
      onClose={onClose}
      footer={
        <>
          <button type="button" className="btn btn-outline" onClick={onClose}>
            {t("common.cancel")}
          </button>
          <button
            type="button"
            className="btn btn-solid"
            disabled={!valid || busy}
            onClick={async () => {
              setBusy(true);
              await onSubmit(url.trim(), name.trim() || undefined);
              setBusy(false);
            }}
          >
            {busy ? <Icon name="refresh" size={14} className="spin" /> : null}
            {t("subs.addAndFetch")}
          </button>
        </>
      }
    >
      <div style={{ display: "flex", flexDirection: "column", gap: 14 }}>
        <label className="field-label" htmlFor="sub-url">
          {t("subs.url")}
          <input
            id="sub-url"
            className="control control-mono"
            style={{ marginTop: 8 }}
            value={url}
            placeholder="https://example.com/sub?token=…"
            onChange={(event) => setUrl(event.target.value)}
          />
          <span className="field-hint" style={{ display: "block", fontWeight: 400 }}>
            {t("subs.urlHint")}
          </span>
        </label>
        <label className="field-label" htmlFor="sub-name">
          {t("subs.name")}
          <input
            id="sub-name"
            className="control"
            style={{ marginTop: 8 }}
            value={name}
            placeholder={t("subs.namePlaceholder")}
            onChange={(event) => setName(event.target.value)}
          />
        </label>
      </div>
    </Modal>
  );
}

export type { SubscriptionView };
