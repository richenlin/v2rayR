import { useStoredState } from "../lib/useStoredState";
import { useCallback, useEffect, useMemo, useState } from "react";
import { Modal } from "../components/Modal";
import { Icon } from "../components/Icon";
import { Field, SearchInput, Select, Toggle } from "../components/Controls";
import { problemTextSafe, useLang, useStore, useT } from "../store";
import type {
  AppearanceSettings,
  CoreOverride,
  InboundSettings,
  RoutingSettings,
  SettingsGroupId,
  StartupSettings,
  SubscriptionSettings,
} from "../lib/types";
import { api, resetPreviewData, toProblem, type BackupPreview } from "../lib/api";
import { isPreview } from "../lib/api";
import type { RuleApplication, RuleView } from "../lib/types";

type EditableGroup = Exclude<SettingsGroupId, "advanced">;
type Group = EditableGroup | "diagnostics";

const groups: Array<{ id: Group; labelKey: string }> = [
  { id: "startup", labelKey: "settings.group.startup" },
  { id: "inbound", labelKey: "settings.group.inbound" },
  { id: "routing", labelKey: "settings.group.routing" },
  { id: "subscription", labelKey: "settings.group.subscription" },
  { id: "appearance", labelKey: "settings.group.appearance" },
  { id: "diagnostics", labelKey: "settings.group.maintenance" },
];

interface GroupDraft {
  startup: StartupSettings;
  inbound: InboundSettings;
  routing: RoutingSettings;
  subscription: SubscriptionSettings;
  appearance: AppearanceSettings;
}


const RULE_KINDS = ["domain", "domain-suffix", "ip-cidr", "process", "geoip", "geosite"] as const;
const RULE_ACTIONS = ["direct", "proxy", "block"] as const;

function PacUpdate() {
  const t = useT();
  const lang = useLang();
  const [busy, setBusy] = useState(false);
  const [result, setResult] = useState("");
  async function update() {
    setBusy(true);
    setResult("");
    try {
      const count = await api.updatePac();
      setResult(t("settings.pac.updated", { count }));
    } catch (error) {
      setResult(error && typeof error === "object" && "code" in error
        ? problemTextSafe(lang, error as Parameters<typeof problemTextSafe>[1])
        : String(error));
    } finally {
      setBusy(false);
    }
  }
  return (
    <Field label={t("settings.pac")} hint={t("settings.pac.hint")}>
      <button type="button" className="btn btn-outline btn-sm" disabled={busy} onClick={() => void update()}>
        {t("settings.pac.update")}
      </button>
      {result ? <p role="status" className="field-hint">{result}</p> : null}
    </Field>
  );
}

function applicationName(path: string) {
  return path.split(/[\\/]/).pop() ?? path;
}

function ruleKindLabel(t: ReturnType<typeof useT>, kind: string) {
  return t(`settings.rules.kind.${kind.replace(/-([a-z])/g, (_, c) => c.toUpperCase())}`);
}

function RulesEditor() {
  const t = useT();
  const rules = useStore((state) => state.rules);
  const connection = useStore((state) => state.snapshot?.connection);
  const toggleRule = useStore((state) => state.toggleRule);
  const removeRule = useStore((state) => state.removeRule);
  const [editor, setEditor] = useState<RuleView | "new" | null>(null);
  const close = useCallback(() => setEditor(null), []);
  return (
    <div className="rules-block">
      <div className="rules-head">
        <div>
          <p className="field-label">{t("settings.rules.title")} <span className="badge" data-tone="active">{rules.length}</span></p>
          <p className="field-hint">{t("settings.rules.hint")}</p>
        </div>
        <button type="button" className="btn btn-outline btn-sm" onClick={() => setEditor("new")}>{t("settings.rules.add")}</button>
      </div>
      {rules.length === 0 ? (
        <p className="field-hint">{t("settings.rules.empty")}</p>
      ) : (
        <ul className="rules-list">
          {rules.map((rule: RuleView) => (
            <li key={rule.id} className="rules-row" data-disabled={!rule.enabled || undefined}>
              <Toggle checked={rule.enabled} label={t("settings.rules.enabled")} onChange={(checked) => void toggleRule(rule.id, checked)} />
              <span className="rules-kind">{ruleKindLabel(t, rule.kind)}
                {rule.enabled && (rule.applications?.length || rule.kind === "process") && (connection?.mode !== "tun" || connection.policy !== "rule" || rule.kind === "process") ? <span className="field-hint">{t("settings.rules.unsupported")}</span> : null}
              </span>
              <span className="mono rules-value">{rule.value}</span>
              <span className="mono rules-value" aria-label={t("settings.rules.application")} title={rule.applications?.map((app) => app.path).join("\n")}>
                {rule.applications?.length ? rule.applications.map((app) => applicationName(app.path)).join(", ") : "*"}
              </span>
              <span className="badge" data-tone={rule.action === "block" ? "danger" : rule.action === "direct" ? "ok" : "active"}>
                {t(`settings.rules.action.${rule.action}`)}
              </span>
              <button type="button" className="btn btn-quiet btn-sm rules-edit" onClick={() => setEditor(rule)}>
                {t("settings.rules.edit")}
              </button>
              <button
                type="button"
                className="btn btn-quiet btn-sm rules-remove"
                aria-label={t("settings.rules.remove")}
                onClick={() => void removeRule(rule.id)}
              >
                <Icon name="trash" size={14} />
              </button>
            </li>
          ))}
        </ul>
      )}
      {editor ? <RuleModal rule={editor === "new" ? undefined : editor} onClose={close} /> : null}
    </div>
  );
}

function RuleModal({ rule, onClose }: { rule?: RuleView; onClose: () => void }) {
  const t = useT();
  const addRule = useStore((state) => state.addRule);
  const updateRule = useStore((state) => state.updateRule);
  const [saving, setSaving] = useState(false);
  const [kind, setKind] = useState(rule?.kind ?? "domain-suffix");
  const [value, setValue] = useState(rule?.value ?? "");
  const [action, setAction] = useState(rule?.action ?? "direct");

  const [candidates, setCandidates] = useState<RuleApplication[]>(rule?.applications ?? []);
  const [selected, setSelected] = useState<RuleApplication[]>(rule?.applications ?? []);
  const [picking, setPicking] = useState(false);
  const [pickerError, setPickerError] = useState("");

  const pick = async () => {
    setPicking(true);
    setPickerError("");
    try {
      const found = await api.pickRuleApplications();
      const main = found[0];
      if (!main) return;
      setCandidates((current) => [...new Map([...current, ...found].map((app) => [app.path, app])).values()]);
      setSelected((current) => [...current.filter((app) => app.path !== main.path), main]);
    } catch (error) {
      setPickerError(problemTextSafe(useStore.getState().lang, toProblem(error)));
    } finally {
      setPicking(false);
    }
  };

  const submit = async () => {
    if (saving || picking || !value.trim()) return;
    setSaving(true);
    try {
      const ok = rule ? await updateRule(rule.id, kind, value, action, selected) : await addRule(kind, value, action, selected);
      if (ok) onClose();
    } finally {
      setSaving(false);
    }
  };
  const close = useCallback(() => {
    if (!saving && !picking) onClose();
  }, [onClose, saving, picking]);

  return (
    <Modal title={t(rule ? "settings.rules.edit" : "settings.rules.add")} onClose={close}
      footer={<>
        <button type="button" className="btn btn-outline" onClick={close} disabled={saving || picking}>{t("common.cancel")}</button>
        <button type="submit" form="custom-rule-form" className="btn btn-primary" disabled={saving || picking || !value.trim()}>{t("settings.rules.save")}</button>
      </>}
    >
      <form id="custom-rule-form" onSubmit={(event) => { event.preventDefault(); void submit(); }}>
      <div className="rules-add">
        <Select
          label={t("settings.rules.kind")}
          value={kind}
          onChange={setKind}
          options={RULE_KINDS.map((item) => ({
            value: item,
            label: ruleKindLabel(t, item),
          }))}
        />
        <input
          className="control control-mono"
          aria-label={t("settings.rules.value")}
          placeholder={kind === "ip-cidr" ? "10.0.0.0/8" : "example.com"}
          value={value}
          onChange={(event) => setValue(event.target.value)}
        />
        <Select
          label={t("settings.rules.action")}
          value={action}
          onChange={setAction}
          options={RULE_ACTIONS.map((item) => ({ value: item, label: t(`settings.rules.action.${item}`) }))}
        />
      </div>
      <div className="rule-app-picker">
        <span className="field-label">{t("settings.rules.application")}</span>
        <span className="mono">{selected.length || "*"}</span>
        <button type="button" className="btn btn-outline btn-sm" disabled={picking || api.kind === "mock"} onClick={() => void pick()}>
          {t("settings.rules.pickApplication")}
        </button>
        {candidates.length > 0 ? (
          <div className="rule-app-candidates">
            {candidates.map((app) => (
              <label key={app.path} title={app.path}>
                <input
                  type="checkbox"
                  checked={selected.some((item) => item.path === app.path)}
                  onChange={(event) => {
                    const checked = event.target.checked;
                    setSelected((current) => checked ? [...current, app] : current.filter((item) => item.path !== app.path));
                  }}
                />
                <span>{applicationName(app.path)}<small className="mono">{app.path}</small></span>
              </label>
            ))}
          </div>
        ) : null}
        {pickerError ? <p className="field-hint" role="alert">{pickerError}</p> : null}
      </div>
      <p className="field-hint">{t("settings.rules.applicationPending")}</p>
      </form>
    </Modal>
  );
}

function Maintenance() {
  const t = useT();
  const refresh = useStore((state) => state.refresh);
  const phase = useStore((state) => state.snapshot?.connection.phase);
  const [backup, setBackup] = useState<BackupPreview | null>(null);
  const [enabled, setEnabled] = useState(false);
  const [version, setVersion] = useState<string | null>(null);
  const [pending, setPending] = useState(false);
  const [remote, setRemote] = useState(false);
  const [url, setUrl] = useState("");
  const [user, setUser] = useState("");
  const [password, setPassword] = useState("");
  const [status, setStatus] = useState("");
  useEffect(() => { void api.updatesConfigured().then(setEnabled).catch(() => {}); }, []);
  const run = async (operation: () => Promise<void>) => {
    setPending(true);
    setStatus("");
    try { await operation(); }
    catch (error) {
      setStatus((error && typeof error === "object" && "detail" in error && typeof error.detail === "string")
        ? error.detail : t("maintenance.failed"));
    } finally { setPending(false); }
  };
  return (
    <>
      <div className="field">
        <div>
          <div className="field-label">{t("maintenance.backup")}</div>
          <p className="field-hint">{t("maintenance.backupHint")}</p>
        </div>
        <div className="field-control is-inline" style={{ display: "flex", gap: 8, flexWrap: "wrap" }}>
          <button className="btn btn-outline btn-sm" disabled={pending} onClick={() => void run(async () => {
            if (await api.exportBackup()) setStatus(t("maintenance.saved"));
          })}>{t("maintenance.export")}</button>
          <button className="btn btn-outline btn-sm" disabled={pending} onClick={() => void run(async () => {
            const selected = await api.previewBackup();
            if (selected) setBackup(selected);
          })}>{t("maintenance.preview")}</button>
        </div>
      </div>
      <div className="field">
        <div>
          <div className="field-label">WebDAV</div>
          <p className="field-hint">{t("maintenance.webdavHint")}</p>
        </div>
        <div className="field-control is-inline">
          <button className="btn btn-outline btn-sm" disabled={pending} aria-expanded={remote} onClick={() => {
            setRemote(!remote);
            setPassword("");
          }}>WebDAV</button>
        </div>
      </div>
      {remote ? (
        <div className="field">
          <div style={{ display: "grid", gap: 6 }}>
            <label htmlFor="backup-webdav-url">{t("maintenance.webdavUrl")}</label>
            <input id="backup-webdav-url" className="control" placeholder="https://dav.example/backup.json" value={url}
              onChange={(event) => setUrl(event.target.value)} />
            <label htmlFor="backup-webdav-user">{t("maintenance.webdavUser")}</label>
            <input id="backup-webdav-user" className="control" value={user} autoComplete="off"
              onChange={(event) => setUser(event.target.value)} />
            {user ? <>
              <label htmlFor="backup-webdav-password">{t("maintenance.webdavPassword")}</label>
              <input id="backup-webdav-password" className="control" type="password" value={password} autoComplete="off"
                onChange={(event) => setPassword(event.target.value)} />
            </> : null}
          </div>
          <div className="field-control is-inline" style={{ display: "flex", gap: 8, flexWrap: "wrap" }}>
            <button className="btn btn-outline btn-sm" disabled={pending || !url.trim()} onClick={() => void run(async () => {
              try { setBackup(await api.readWebDav(url.trim(), user || undefined, user ? password : undefined)); }
              finally { setPassword(""); }
            })}>{t("maintenance.webdavRead")}</button>
            <button className="btn btn-outline btn-sm" disabled={pending || !url.trim()} onClick={() => void run(async () => {
              try { await api.createWebDav(url.trim(), user || undefined, user ? password : undefined); setStatus(t("maintenance.saved")); }
              finally { setPassword(""); }
            })}>{t("maintenance.webdavCreate")}</button>
          </div>
        </div>
      ) : null}
      {backup ? (
        <div className="field">
          <div>
            <b>{t("maintenance.restore")}</b>
            <p className="field-hint">{t("maintenance.restoreHint")} · {backup.summary.rules}</p>
            {phase !== "disconnected" ? <p className="field-hint">{t("maintenance.disconnectFirst")}</p> : null}
          </div>
          <div className="field-control is-inline" style={{ display: "flex", gap: 8 }}>
            <button className="btn btn-outline btn-sm" disabled={pending} onClick={() => setBackup(null)}>{t("common.cancel")}</button>
            <button className="btn btn-solid btn-sm" disabled={pending || phase !== "disconnected"} onClick={() => void run(async () => {
              const added = await api.applyBackup(backup.content);
              setBackup(null);
              await refresh();
              setStatus(t("maintenance.restored") + " · " + added);
            })}>{t("common.confirm")}</button>
          </div>
        </div>
      ) : null}
      {enabled ? (
        <div className="field">
          <div>
            <div className="field-label">{t("maintenance.update")}</div>
            <p className="field-hint">{version ? t("maintenance.updateAvailable") + " · " + version : t("maintenance.updateHint")}</p>
          </div>
          <div className="field-control is-inline" style={{ display: "flex", gap: 8 }}>
            <button className="btn btn-outline btn-sm" disabled={pending} onClick={() => void run(async () => {
              const available = await api.checkUpdate();
              setVersion(available);
              if (!available) setStatus(t("maintenance.upToDate"));
            })}>{t("maintenance.check")}</button>
            {version ? <button className="btn btn-solid btn-sm" disabled={pending} onClick={() => void run(async () => {
              await api.installUpdate(version);
            })}>{t("maintenance.install")}</button> : null}
          </div>
        </div>
      ) : null}
      {pending || status ? <p className="field-hint" role="status">{pending ? t("maintenance.working") : status}</p> : null}
    </>
  );
}

export function SettingsPage() {
  const t = useT();
  const lang = useLang();
  const snapshot = useStore((state) => state.snapshot);
  const busy = useStore((state) => state.busy);
  const saveSettings = useStore((state) => state.saveSettings);
  const resetSettings = useStore((state) => state.resetSettings);
  const probeCores = useStore((state) => state.probeCores);
  const installCore = useStore((state) => state.installCore);
  const [group, setGroup] = useStoredState<Group>("v2rayr.settings.group", "inbound");
  const [search, setSearch] = useStoredState("v2rayr.settings.search", "");
  const [drafts, setDrafts] = useStoredState<Partial<GroupDraft>>("v2rayr.settings.drafts", {});

  const settings = snapshot?.settings;
  if (!settings || !snapshot) return <div className="page" />;

  const draftOf = <K extends keyof GroupDraft>(key: K): GroupDraft[K] =>
    (drafts[key] as GroupDraft[K] | undefined) ?? (settings[key] as GroupDraft[K]);

  const patch = <K extends keyof GroupDraft>(key: K, values: Partial<GroupDraft[K]>) => {
    setDrafts((current) => ({
      ...current,
      [key]: { ...(current[key] ?? settings[key]), ...values } as GroupDraft[K],
    }));
  };

  const term = search.trim().toLowerCase();

  const matches = (...keys: string[]): boolean =>
    term.length === 0 || keys.some((key) => t(key).toLowerCase().includes(term));

  const save = async () => {
    if (activeGroup === "diagnostics") return;
    const key = activeGroup;
    const ok = await saveSettings([{ group: key, values: draftOf(key) as unknown as Record<string, unknown> }]);
    if (ok) {
      setDrafts((current) => {
        const next = { ...current };
        delete next[key];
        return next;
      });
    }
  };

  const groupsWithMatches = useMemo(() => {
    if (!term) return null;
    const hit = (keys: string[]) => keys.some((key) => t(key).toLowerCase().includes(term));
    return new Set<Group>(
      [
        hit(["settings.launchAtLogin", "settings.connectOnLaunch", "settings.closeToTray"]) ? "startup" : null,
        hit(["settings.mixedPort", "settings.bypassLocal", "settings.bypassExtra", "settings.allowLan", "settings.lanBind", "settings.dokodemo"]) ? "inbound" : null,
        hit(["settings.ruleSet", "settings.blockAds", "settings.dnsServers", "settings.rules", "settings.pac"]) ? "routing" : null,
        hit(["settings.interval"]) ? "subscription" : null,
        hit(["settings.theme", "settings.language", "settings.compact", "settings.reduceMotion"]) ? "appearance" : null,
        hit(["settings.cores", "settings.paths", "settings.reset", "settings.version", "maintenance.backup", "maintenance.update"]) ? "diagnostics" : null,
      ].filter((value): value is Group => value !== null),
    );
  }, [term, lang, t]);

  const showGroup = (id: Group): boolean => !term || (groupsWithMatches?.has(id) ?? false);
  const activeGroup = showGroup(group) ? group : groups.find((entry) => showGroup(entry.id))?.id ?? "inbound";

  const dirty = activeGroup !== "diagnostics" && drafts[activeGroup] !== undefined;
  const body = (() => {
    switch (activeGroup) {
      case "startup": {
        const value = draftOf("startup");
        return (
          <>
            {matches("settings.launchAtLogin", "settings.launchAtLogin.hint") ? (
              <Field label={t("settings.launchAtLogin")} hint={t("settings.launchAtLogin.hint")} inline htmlFor="launch-at-login">
                <Toggle
                  checked={value.launchAtLogin}
                  label={t("settings.launchAtLogin")}
                  onChange={(checked) => patch("startup", { launchAtLogin: checked })}
                />
              </Field>
            ) : null}
            {matches("settings.connectOnLaunch", "settings.connectOnLaunch.hint") ? (
              <Field label={t("settings.connectOnLaunch")} hint={t("settings.connectOnLaunch.hint")} inline>
                <Toggle checked={value.connectOnLaunch} label={t("settings.connectOnLaunch")} onChange={(checked) => patch("startup", { connectOnLaunch: checked })} />
              </Field>
            ) : null}
            {matches("settings.closeToTray") ? (
              <Field label={t("settings.closeToTray")} inline>
                <Toggle checked={value.closeToTray} label={t("settings.closeToTray")} onChange={(checked) => patch("startup", { closeToTray: checked })} />
              </Field>
            ) : null}
          </>
        );
      }
      case "inbound": {
        const value = draftOf("inbound");
        return (
          <>
            {matches("settings.mixedPort", "settings.mixedPort.hint") ? (
              <Field label={t("settings.mixedPort")} hint={t("settings.mixedPort.hint")} htmlFor="mixed-port">
                <input
                  id="mixed-port"
                  className="control control-mono"
                  type="number"
                  min={0}
                  max={65535}
                  value={value.mixedPort}
                  onChange={(event) => patch("inbound", { mixedPort: Number(event.target.value) })}
                />
              </Field>
            ) : null}
            <p className="field-hint">{t("settings.rules.bypassLegacy")}</p>
            {matches("settings.allowLan", "settings.allowLan.hint", "settings.lanBind") ? (
              <Field label={t("settings.allowLan")} hint={t("settings.allowLan.hint")} inline>
                <Toggle checked={value.allowLan} label={t("settings.allowLan")} onChange={(checked) => patch("inbound", { allowLan: checked })} />
              </Field>
            ) : null}
            {value.allowLan && matches("settings.lanBind", "settings.allowLan") ? (
              <Field label={t("settings.lanBind")} hint={t("settings.lanBind.hint")} htmlFor="lan-bind">
                <input id="lan-bind" className="control control-mono" value={value.lanBind ?? ""}
                  placeholder="192.168.1.20" onChange={(event) => patch("inbound", { lanBind: event.target.value })} />
              </Field>
            ) : null}
            {matches("settings.dokodemo", "settings.dokodemo.hint") ? (
              <details open={Boolean(term || value.dokodemoListenPort)}>
                <summary className="field-label">{t("settings.dokodemo")}</summary>
                <Field label={t("settings.dokodemo.listen")} hint={t("settings.dokodemo.hint")} htmlFor="dokodemo-listen">
                  <input id="dokodemo-listen" className="control" type="number" min={0} max={65535}
                    value={value.dokodemoListenPort}
                    onChange={(event) => patch("inbound", { dokodemoListenPort: Number(event.target.value) })} />
                </Field>
                {value.dokodemoListenPort ? (
                  <>
                    <Field label={t("settings.dokodemo.address")} htmlFor="dokodemo-address">
                      <input id="dokodemo-address" className="control" value={value.dokodemoAddress}
                        onChange={(event) => patch("inbound", { dokodemoAddress: event.target.value })} />
                    </Field>
                    <Field label={t("settings.dokodemo.port")} htmlFor="dokodemo-port">
                      <input id="dokodemo-port" className="control" type="number" min={1} max={65535}
                        value={value.dokodemoTargetPort}
                        onChange={(event) => patch("inbound", { dokodemoTargetPort: Number(event.target.value) })} />
                    </Field>
                  </>
                ) : null}
              </details>
            ) : null}
          </>
        );
      }
      case "routing": {
        const value = draftOf("routing");
        return (
          <>
            <p className="field-hint">{t("settings.rules.explicit")}</p>
            {matches("settings.dnsServers", "settings.dnsServers.hint") ? (
              <details open={Boolean(term || value.dnsServers.trim())}>
                <summary className="field-label" style={{ cursor: "pointer", padding: "14px 0" }}>{t("settings.customDns")}</summary>
              <Field label={t("settings.dnsServers")} hint={t("settings.dnsServers.hint")} htmlFor="dns-servers">
                <textarea
                  id="dns-servers"
                  className="control control-mono"
                  value={value.dnsServers}
                  placeholder={"1.1.1.1\nhttps://dns.google/dns-query"}
                  onChange={(event) => patch("routing", { dnsServers: event.target.value })}
                />
              </Field>
              </details>
            ) : null}
            {matches("settings.rules", "settings.rules.hint") ? <RulesEditor /> : null}
            {!isPreview && matches("settings.pac", "settings.pac.hint") ? <PacUpdate /> : null}
          </>
        );
      }
      case "subscription": {
        const value = draftOf("subscription");
        return (
          <>
            {matches("settings.interval", "settings.interval.hint") ? (
              <Field label={t("settings.interval")} hint={t("settings.interval.hint")} htmlFor="sub-interval">
                <input
                  id="sub-interval"
                  className="control control-mono"
                  type="number"
                  min={1}
                  max={168}
                  value={value.intervalHours}
                  onChange={(event) => patch("subscription", { intervalHours: Number(event.target.value) })}
                />
              </Field>
            ) : null}
          </>
        );
      }
      case "appearance": {
        const value = draftOf("appearance");
        return (
          <>
            {matches("settings.theme", "settings.theme.hint") ? (
              <Field label={t("settings.theme")} hint={t("settings.theme.hint")} htmlFor="theme">
                <Select
                  id="theme"
                  label={t("settings.theme")}
                  value={value.theme}
                  onChange={(next) => patch("appearance", { theme: next })}
                  options={[
                    { value: "system", label: t("settings.theme.system") },
                    { value: "light", label: t("settings.theme.light") },
                    { value: "dark", label: t("settings.theme.dark") },
                  ]}
                />
              </Field>
            ) : null}
            {matches("settings.language") ? (
              <Field label={t("settings.language")} htmlFor="language">
                <Select
                  id="language"
                  label={t("settings.language")}
                  value={value.language}
                  onChange={(next) => patch("appearance", { language: next })}
                  options={[
                    { value: "system", label: t("settings.language.system") },
                    { value: "zh-cn", label: t("settings.language.zh") },
                    { value: "en-us", label: t("settings.language.en") },
                  ]}
                />
              </Field>
            ) : null}
            {matches("settings.compact", "settings.compact.hint") ? (
              <Field label={t("settings.compact")} hint={t("settings.compact.hint")} inline>
                <Toggle checked={value.compact} label={t("settings.compact")} onChange={(checked) => patch("appearance", { compact: checked })} />
              </Field>
            ) : null}
            {matches("settings.reduceMotion") ? (
              <Field label={t("settings.reduceMotion")} inline>
                <Toggle checked={value.reduceMotion} label={t("settings.reduceMotion")} onChange={(checked) => patch("appearance", { reduceMotion: checked })} />
              </Field>
            ) : null}
          </>
        );
      }
      default:
        return (
          <>
            <div className="field">
              <div>
                <div className="field-label">{t("settings.cores")}</div>
                <p className="field-hint">{t("settings.cores.hint")}</p>
              </div>
              <div className="field-control" style={{ maxWidth: 360 }}>
                <button type="button" className="btn btn-outline btn-sm" onClick={() => void probeCores()}>
                  <Icon name="refresh" size={13} />
                  {t("settings.cores.probe")}
                </button>
              </div>
            </div>
            {!isPreview ? <Maintenance /> : null}
            <Field label={t("settings.cores.selection")} hint={t("settings.cores.selectionHint")} htmlFor="core-selection">
              <select id="core-selection" className="control" disabled={busy !== null}
                value={snapshot.settings.advanced.coreOverride}
                onChange={(event) => void saveSettings([{
                  group: "advanced",
                  values: { ...snapshot.settings.advanced, coreOverride: event.target.value as CoreOverride },
                }])}>
                <option value="auto">{t("settings.cores.auto")}</option>
                {snapshot.cores.map((core) => (
                  <option key={core.kind} value={core.kind} disabled={!core.installed}>{core.kind}</option>
                ))}
              </select>
            </Field>
            {snapshot.cores.map((core) => (
              <div key={core.kind} className="field">
                <div>
                  <div className="field-label" style={{ display: "flex", gap: 8, alignItems: "center" }}>
                    {core.kind}
                    <span className="badge" data-tone={core.installed ? "ok" : "warn"}>
                      {core.installed ? t("settings.cores.installed") : t("settings.cores.missing")}
                    </span>
                  </div>
                  <p className="field-hint mono">{core.path ?? core.protocols.join(" / ")}</p>
                </div>
                <div className="field-control is-inline">
                  {core.installed ? (
                    <span className="mono" style={{ fontSize: 12, color: "var(--muted)" }}>{core.version ?? "—"}</span>
                  ) : (
                    <button type="button" className="btn btn-outline btn-sm" disabled={busy !== null} onClick={() => void installCore(core.kind)}>
                      {busy === "install-core" ? t("connection.installingCore") : t("settings.cores.install")}
                    </button>
                  )}
                </div>
              </div>
            ))}
            <div className="field">
              <div>
                <div className="field-label">{t("settings.paths")}</div>
                <p className="field-hint mono">{snapshot.version.appVersion} · schema {snapshot.version.settingsSchema} · db {snapshot.version.dbSchema}</p>
              </div>
              <div className="field-control is-inline">
                <span className="badge">API v{snapshot.version.apiVersion}</span>
              </div>
            </div>
            <div className="field">
              <div>
                <div className="field-label">{t("settings.reset")}</div>
                <p className="field-hint">{t("settings.version")} {snapshot.version.appVersion}</p>
              </div>
              <div className="field-control is-inline" style={{ display: "flex", gap: 8 }}>
                {isPreview ? (
                  <button type="button" className="btn btn-outline btn-sm" onClick={() => { if (window.confirm(t("common.confirm") + "?")) resetPreviewData(); }}>
                    <Icon name="refresh" size={13} /> Preview reset
                  </button>
                ) : null}
                <button
                  type="button"
                  className="btn btn-outline btn-sm"
                  onClick={() => {
                    if (window.confirm(t("settings.resetConfirm"))) void resetSettings();
                  }}
                >
                  <Icon name="refresh" size={13} />
                  {t("settings.reset")}
                </button>
              </div>
            </div>
          </>
        );
    }
  })();

  const groupHasContent = body && (!Array.isArray(body) ? true : body.filter(Boolean).length > 0);

  return (
    <div className="page">
      <div className="eyebrow">{t("settings.eyebrow")}</div>
      <h1 className="page-title">{t("settings.title")}</h1>
      <p className="page-sub">{t("settings.subtitle")}</p>

      <div style={{ marginTop: 22, maxWidth: 560 }}>
        <SearchInput value={search} onChange={setSearch} placeholder={t("settings.search")} />
      </div>

      <div className="settings-shell" style={{ marginTop: 20 }}>
        <nav className="settings-nav" aria-label={t("settings.title")}>
          {groups.map((entry) => (
            <button
              key={entry.id}
              type="button"
              aria-current={activeGroup === entry.id}
              style={{ opacity: showGroup(entry.id) ? 1 : 0.45 }}
              onClick={() => setGroup(entry.id)}
            >
              {t(entry.labelKey)}
            </button>
          ))}
        </nav>

        <div className="panel settings-body">
          <div style={{ padding: "18px 18px 0" }}>
            <div className="settings-group-head">
              <div>
                <h2 className="settings-group-title">{t(groups.find((entry) => entry.id === activeGroup)?.labelKey ?? "settings.title")}</h2>
                <p className="field-hint" style={{ marginTop: 4 }}>
                  {activeGroup === "inbound" || activeGroup === "routing" ? t("settings.reconnectHint") : t("settings.restartHint")}
                </p>
              </div>
            </div>
            <div>
              {groupHasContent ? body : (
                <div className="empty">
                  <b>{t("settings.searchEmpty")}</b>
                  {t("settings.searchEmptyHint")}
                </div>
              )}
            </div>
          </div>
          {activeGroup === "diagnostics" ? null : (
            <div className="settings-foot">
              <span className="hint">{dirty ? t("common.unsaved") : t("common.saved")}</span>
              <div style={{ display: "flex", gap: 10 }}>
                <button
                  type="button"
                  className="btn btn-outline"
                  disabled={!dirty || busy === "settings"}
                  onClick={() => {
                    const key = activeGroup;
                    setDrafts((current) => {
                      const next = { ...current };
                      delete next[key];
                      return next;
                    });
                  }}
                >
                  {t("common.revert")}
                </button>
                <button type="button" className="btn btn-solid" disabled={!dirty || busy === "settings"} onClick={() => void save()}>
                  {busy === "settings" ? <Icon name="refresh" size={14} className="spin" /> : null}
                  {t(activeGroup === "inbound" || activeGroup === "routing" ? "settings.saveReconnect" : "common.save")}
                </button>
              </div>
            </div>
          )}
        </div>
      </div>

      <div className="origin-note">
        <span>v2rayR / {t("settings.title")}</span>
        <span className="mono">{t("settings.subtitle")}</span>
      </div>
    </div>
  );
}
