import { useMemo, useRef, useState } from "react";
import { Icon } from "../components/Icon";
import { Modal } from "../components/Modal";
import { SearchInput } from "../components/Controls";
import { latencyTone, protocolLabel, transportLabel } from "../lib/format";
import { useLang, useStore, useT } from "../store";
import type { NodeView } from "../lib/types";

const ROW_HEIGHT = 58;
const VIEWPORT = 480;

export function NodesPage() {
  const t = useT();
  const lang = useLang();
  const snapshot = useStore((state) => state.snapshot);
  const query = useStore((state) => state.nodeQuery);
  const scope = useStore((state) => state.nodeScope);
  const protocol = useStore((state) => state.nodeProtocol);
  const setQuery = useStore((state) => state.setNodeQuery);
  const setScope = useStore((state) => state.setNodeScope);
  const setProtocol = useStore((state) => state.setNodeProtocol);
  const testLatency = useStore((state) => state.testLatency);
  const testing = useStore((state) => state.testing);
  const toggleFavorite = useStore((state) => state.toggleFavorite);
  const copyLink = useStore((state) => state.copyLink);
  const removeNode = useStore((state) => state.removeNode);
  const saveTags = useStore((state) => state.saveTags);
  const switchNode = useStore((state) => state.switchNode);
  const connect = useStore((state) => state.connect);
  const connection = useStore((state) => state.snapshot?.connection);
  const [importOpen, setImportOpen] = useState(false);
  const [tagEdit, setTagEdit] = useState<NodeView | null>(null);
  const [scrollTop, setScrollTop] = useState(0);
  const listRef = useRef<HTMLDivElement>(null);

  const nodes = snapshot?.nodes ?? [];
  const protocols = useMemo(() => {
    const set = new Set(nodes.map((node) => node.protocol));
    return [...set].sort();
  }, [nodes]);

  const filtered = useMemo(() => {
    const term = query.trim().toLowerCase();
    return nodes.filter((node) => {
      if (scope === "favorites" && !node.favorite) return false;
      if (protocol !== "all" && node.protocol !== protocol) return false;
      if (!term) return true;
      return `${node.remark} ${node.address} ${node.protocol} ${node.transport} ${node.tags.join(" ")}`
        .toLowerCase()
        .includes(term);
    });
  }, [nodes, query, scope, protocol]);

  if (!snapshot) return <div className="page" />;

  const activeId = connection?.profileId;
  const start = Math.max(0, Math.floor(scrollTop / ROW_HEIGHT) - 4);
  const visible = filtered.slice(start, start + Math.ceil(VIEWPORT / ROW_HEIGHT) + 8);

  return (
    <div className="page">
      <div className="eyebrow">{t("nodes.eyebrow")}</div>
      <h1 className="page-title">{t("nodes.title")}</h1>
      <p className="page-sub">{t("nodes.subtitle")}</p>

      <div className="toolbar nodes-toolbar">
        <div className="grow">
          <SearchInput value={query} onChange={setQuery} placeholder={t("nodes.search")} />
        </div>
        <div className="segmented" role="group" aria-label={t("common.all")}>
          <button type="button" aria-pressed={scope === "all"} onClick={() => setScope("all")}>
            {t("common.all")} {nodes.length}
          </button>
          <button type="button" aria-pressed={scope === "favorites"} onClick={() => setScope("favorites")}>
            {t("common.favorites")} {nodes.filter((node) => node.favorite).length}
          </button>
        </div>
        <select
          className="control"
          style={{ width: 132 }}
          value={protocol}
          aria-label={t("nodes.filter.protocol")}
          onChange={(event) => setProtocol(event.target.value)}
        >
          <option value="all">{t("nodes.filter.protocol")}</option>
          {protocols.map((item) => (
            <option key={item} value={item}>
              {protocolLabel(item)}
            </option>
          ))}
        </select>
        <button
          type="button"
          className="btn btn-outline"
          disabled={testing || nodes.length === 0}
          onClick={() => void testLatency([])}
        >
          <Icon name="gauge" size={15} className={testing ? "spin" : undefined} />
          {testing ? t("speed.running") : t("nodes.testAll")}
        </button>
        <button type="button" className="btn btn-solid" onClick={() => setImportOpen(true)}>
          <Icon name="plus" size={15} />
          {t("nodes.add")}
        </button>
      </div>

      <section className="panel">
        <div
          className="row row-head nodes-row"
          role="row"
        >
          <span>{t("nodes.col.node")}</span>
          <span>{t("nodes.col.endpoint")}</span>
          <span style={{ textAlign: "right" }}>{t("nodes.col.latency")}</span>
          <span style={{ textAlign: "right" }}>{t("nodes.col.actions")}</span>
        </div>
        {filtered.length === 0 ? (
          <div className="empty">
            <b>{t("nodes.empty")}</b>
            {t("nodes.emptyHint")}
          </div>
        ) : (
          <div
            ref={listRef}
            className="table-wrap"
            style={{ height: VIEWPORT }}
            onScroll={(event) => setScrollTop(event.currentTarget.scrollTop)}
            role="rowgroup"
          >
            <div style={{ height: start * ROW_HEIGHT }} />
            {visible.map((node) => (
              <NodeRow
                key={node.id}
                node={node}
                active={activeId === node.id}
                busy={testing}
                locale={lang}
                onToggleFavorite={() => void toggleFavorite(node.id, !node.favorite)}
                onCopy={() => void copyLink(node.id)}
                onTags={() => setTagEdit(node)}
                onDelete={() => {
                  if (window.confirm(t("nodes.deleteConfirm", { name: node.remark }))) {
                    void removeNode(node.id);
                  }
                }}
                onUse={() => {
                  // Acting on a node must not silently change the access mode the
                  // user picked on the connection page: keep the session's mode for
                  // a switch, and use the currently selected mode/policy for a fresh
                  // connection instead of a hard-coded system-proxy/rule.
                  if (activeId) void switchNode(node.id);
                  else
                    void connect(
                      node.id,
                      connection?.mode ?? "system-proxy",
                      connection?.policy ?? "rule",
                    );
                }}
              />
            ))}
            <div style={{ height: Math.max(0, (filtered.length - start - visible.length) * ROW_HEIGHT) }} />
          </div>
        )}
      </section>

      <div className="origin-note">
        <span>
          {t("connection.nodeCount", { n: filtered.length })}
          {filtered.length !== nodes.length ? ` / ${nodes.length}` : ""}
        </span>
        <span className="mono">{testing ? t("speed.running") : t("speed.legend")}</span>
      </div>

      {importOpen ? <ImportModal onClose={() => setImportOpen(false)} /> : null}
      {tagEdit ? (
        <TagsModal
          node={tagEdit}
          onClose={() => setTagEdit(null)}
          onSave={(tags) => {
            void saveTags(tagEdit.id, tags);
            setTagEdit(null);
          }}
        />
      ) : null}
    </div>
  );
}

function NodeRow({
  node,
  active,
  busy,
  locale,
  onToggleFavorite,
  onCopy,
  onTags,
  onDelete,
  onUse,
}: {
  node: NodeView;
  active: boolean;
  busy: boolean;
  locale: string;
  onToggleFavorite: () => void;
  onCopy: () => void;
  onTags: () => void;
  onDelete: () => void;
  onUse: () => void;
}) {
  const t = useT();
  const [menu, setMenu] = useState(false);
  void locale;
  return (
    <div
      className="row nodes-row"
      style={{ height: ROW_HEIGHT, position: "relative" }}
      role="row"
    >
      <div style={{ minWidth: 0 }}>
        <div className="node-name">
          <button
            type="button"
            className="btn btn-quiet"
            style={{ height: 20, padding: 0, color: node.favorite ? "var(--warn)" : "var(--muted)" }}
            aria-label={node.favorite ? t("nodes.unfavorite") : t("nodes.favorite")}
            aria-pressed={node.favorite}
            onClick={onToggleFavorite}
          >
            <Icon name="star" size={13} />
          </button>
          <span title={node.remark}>{node.remark}</span>
          {active ? (
            <span className="badge" data-tone="active">
              {t("connection.inUse")}
            </span>
          ) : null}
        </div>
        <div className="node-sub" title={`${protocolLabel(node.protocol)} · ${transportLabel(node.transport)} · ${node.core}${node.tags.length ? ` · ${node.tags.join(" / ")}` : ""}${node.subscriptionName ? ` · ${node.subscriptionName}` : ""}`}>
          {protocolLabel(node.protocol)} · {transportLabel(node.transport)} · {node.core}
          {node.tags.length ? ` · ${node.tags.join(" / ")}` : ""}
          {node.subscriptionName ? ` · ${node.subscriptionName}` : ""}
        </div>
      </div>
      <div className="mono node-endpoint" title={`${node.address}:${node.port}`}>
        {node.address}:{node.port}
      </div>
      <div className="latency" data-tone={latencyTone(node.latency?.ms)}>
        {node.latency ? `${node.latency.ms} ms` : "—"}
      </div>
      <div style={{ display: "flex", gap: 6, justifyContent: "flex-end", alignItems: "center" }}>
        <button type="button" className="btn btn-outline btn-sm" disabled={busy} onClick={onUse}>
          {active ? t("connection.reconnect") : t("connection.switch")}
        </button>
        <div style={{ position: "relative" }}>
          <button
            type="button"
            className="btn btn-outline btn-sm"
            aria-label={t("nodes.menu")}
            aria-expanded={menu}
            onClick={() => setMenu((open) => !open)}
          >
            <Icon name="chevron" size={14} />
          </button>
          {menu ? (
            <>
              <div
                style={{ position: "fixed", inset: 0, zIndex: 8 }}
                onClick={() => setMenu(false)}
                aria-hidden="true"
              />
              <div
                className="panel"
                style={{ position: "absolute", right: 0, top: 36, zIndex: 9, width: 168, padding: 6 }}
                role="menu"
              >
                <MenuItem icon="copy" label={t("common.copy")} onClick={() => { setMenu(false); onCopy(); }} />
                <MenuItem icon="tag" label={t("nodes.editTags")} onClick={() => { setMenu(false); onTags(); }} />
                <MenuItem icon="trash" label={t("common.delete")} danger onClick={() => { setMenu(false); onDelete(); }} />
              </div>
            </>
          ) : null}
        </div>
      </div>
    </div>
  );
}

function MenuItem({
  icon,
  label,
  onClick,
  danger,
}: {
  icon: "copy" | "tag" | "trash";
  label: string;
  onClick: () => void;
  danger?: boolean;
}) {
  return (
    <button
      type="button"
      role="menuitem"
      className="nav-item"
      style={{ padding: "8px 9px", color: danger ? "var(--danger)" : undefined, fontSize: 13 }}
      onClick={onClick}
    >
      <Icon name={icon} size={14} />
      {label}
    </button>
  );
}

function ImportModal({ onClose }: { onClose: () => void }) {
  const t = useT();
  const importNodes = useStore((state) => state.importNodes);
  const [text, setText] = useState("");
  const [result, setResult] = useState<{ added: number; duplicates: number; failures: number; subscriptions: number } | null>(null);
  const [busy, setBusy] = useState(false);
  const [httpProxy, setHttpProxy] = useState(false);

  return (
    <Modal
      title={t("nodes.import.title")}
      subtitle={t("nodes.import.hint")}
      onClose={onClose}
      footer={
        <>
          <button type="button" className="btn btn-outline" onClick={onClose}>
            {t("common.cancel")}
          </button>
          <button
            type="button"
            className="btn btn-solid"
            disabled={busy || text.trim().length === 0}
            onClick={async () => {
              setBusy(true);
              try {
                const input = httpProxy
                  ? text.split(/\r?\n/).map((line) => line.trim().replace(/^(https?):\/\//i, "$1-proxy://")).join("\n")
                  : text;
                const summary = await importNodes(input);
                if (summary.failures === 0 && (summary.added + summary.duplicates + summary.subscriptions > 0)) {
                  onClose();
                } else {
                  setResult(summary);
                }
              } finally {
                setBusy(false);
              }
            }}
          >
            {busy ? <Icon name="refresh" size={14} className="spin" /> : null}
            {t("nodes.import.action")}
          </button>
        </>
      }
    >
      <label className="field-label" style={{ display: "block", marginBottom: 12 }}>
        <input type="checkbox" checked={httpProxy} onChange={(event) => setHttpProxy(event.target.checked)} />
        {" "}{t("nodes.import.httpProxy")}
      </label>
      <textarea
        className="control control-mono"
        style={{ minHeight: 190 }}
        value={text}
        placeholder={t("nodes.import.placeholder")}
        aria-label={t("nodes.import.placeholder")}
        onChange={(event) => setText(event.target.value)}
      />
      {result ? (
        <div style={{ marginTop: 12 }} className="field-hint">
          <p>
            {t("nodes.import.result", {
              added: result.added,
              dup: result.duplicates,
              failed: result.failures,
            })}
          </p>
          {result.subscriptions > 0 ? (
            <p>{t("nodes.import.subscriptions", { n: result.subscriptions })}</p>
          ) : null}
        </div>
      ) : null}
    </Modal>
  );
}

function TagsModal({
  node,
  onClose,
  onSave,
}: {
  node: NodeView;
  onClose: () => void;
  onSave: (tags: string[]) => void;
}) {
  const t = useT();
  const [value, setValue] = useState(node.tags.join(", "));
  return (
    <Modal
      title={t("nodes.editTags")}
      subtitle={node.remark}
      onClose={onClose}
      footer={
        <>
          <button type="button" className="btn btn-outline" onClick={onClose}>
            {t("common.cancel")}
          </button>
          <button
            type="button"
            className="btn btn-solid"
            onClick={() =>
              onSave(
                value
                  .split(/[,，]/)
                  .map((tag) => tag.trim())
                  .filter(Boolean)
                  .slice(0, 8),
              )
            }
          >
            {t("common.save")}
          </button>
        </>
      }
    >
      <input
        className="control"
        value={value}
        placeholder={t("nodes.tagsHint")}
        aria-label={t("nodes.tags")}
        onChange={(event) => setValue(event.target.value)}
      />
      <p className="field-hint">{t("nodes.tagsHint")}</p>
    </Modal>
  );
}
