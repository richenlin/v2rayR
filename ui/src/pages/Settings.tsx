import { useMemo, useState } from "react";
import { Icon } from "../components/Icon";
import { Field, SearchInput, Select, Toggle } from "../components/Controls";
import { useLang, useStore, useT } from "../store";
import type {
  AdvancedSettings,
  AppearanceSettings,
  InboundSettings,
  RoutingSettings,
  SettingsGroupId,
  StartupSettings,
  SubscriptionSettings,
} from "../lib/types";
import { resetPreviewData } from "../lib/api";
import { isPreview } from "../lib/api";
import type { RuleView } from "../lib/types";

type Group = SettingsGroupId | "diagnostics";

const groups: Array<{ id: Group; labelKey: string }> = [
  { id: "startup", labelKey: "settings.group.startup" },
  { id: "inbound", labelKey: "settings.group.inbound" },
  { id: "routing", labelKey: "settings.group.routing" },
  { id: "subscription", labelKey: "settings.group.subscription" },
  { id: "appearance", labelKey: "settings.group.appearance" },
  { id: "advanced", labelKey: "settings.group.advanced" },
  { id: "diagnostics", labelKey: "settings.cores" },
];

interface GroupDraft {
  startup: StartupSettings;
  inbound: InboundSettings;
  routing: RoutingSettings;
  subscription: SubscriptionSettings;
  appearance: AppearanceSettings;
  advanced: AdvancedSettings;
}


const RULE_KINDS = ["domain", "domain-suffix", "ip-cidr", "process", "geoip", "geosite"] as const;
const RULE_ACTIONS = ["direct", "proxy", "block"] as const;

function RulesEditor() {
  const t = useT();
  const rules = useStore((state) => state.rules);
  const addRule = useStore((state) => state.addRule);
  const toggleRule = useStore((state) => state.toggleRule);
  const removeRule = useStore((state) => state.removeRule);
  const [kind, setKind] = useState<string>("domain-suffix");
  const [value, setValue] = useState("");
  const [action, setAction] = useState<string>("direct");

  const submit = async () => {
    const ok = await addRule(kind, value, action);
    if (ok) setValue("");
  };

  return (
    <div className="rules-block">
      <div className="rules-head">
        <div>
          <p className="field-label">{t("settings.rules.title")}</p>
          <p className="field-hint">{t("settings.rules.hint")}</p>
        </div>
        <span className="badge" data-tone="active">
          {rules.length}
        </span>
      </div>
      <div className="rules-add">
        <Select
          label={t("settings.rules.kind")}
          value={kind}
          onChange={setKind}
          options={RULE_KINDS.map((item) => ({
            value: item,
            label: t(`settings.rules.kind.${item.replace(/-([a-z])/g, (_, c) => c.toUpperCase())}`),
          }))}
        />
        <input
          className="control control-mono"
          aria-label={t("settings.rules.value")}
          placeholder={kind === "ip-cidr" ? "10.0.0.0/8" : "example.com"}
          value={value}
          onChange={(event) => setValue(event.target.value)}
          onKeyDown={(event) => {
            if (event.key === "Enter") void submit();
          }}
        />
        <Select
          label={t("settings.rules.action")}
          value={action}
          onChange={setAction}
          options={RULE_ACTIONS.map((item) => ({ value: item, label: t(`settings.rules.action.${item}`) }))}
        />
        <button type="button" className="btn btn-outline btn-sm" onClick={() => void submit()} disabled={!value.trim()}>
          {t("settings.rules.add")}
        </button>
      </div>
      {rules.length === 0 ? (
        <p className="field-hint">{t("settings.rules.empty")}</p>
      ) : (
        <ul className="rules-list">
          {rules.map((rule: RuleView) => (
            <li key={rule.id} className="rules-row" data-disabled={!rule.enabled || undefined}>
              <Toggle checked={rule.enabled} label={t("settings.rules.enabled")} onChange={(checked) => void toggleRule(rule.id, checked)} />
              <span className="mono rules-kind">{rule.kind}</span>
              <span className="mono rules-value">{rule.value}</span>
              <span className="badge" data-tone={rule.action === "block" ? "danger" : rule.action === "direct" ? "ok" : "active"}>
                {t(`settings.rules.action.${rule.action}`)}
              </span>
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
    </div>
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
  const [group, setGroup] = useState<Group>("inbound");
  const [search, setSearch] = useState("");
  const [drafts, setDrafts] = useState<Partial<GroupDraft>>({});

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

  const dirty = group !== "diagnostics" && drafts[group as SettingsGroupId] !== undefined;
  const term = search.trim().toLowerCase();

  const matches = (...keys: string[]): boolean =>
    term.length === 0 || keys.some((key) => t(key).toLowerCase().includes(term));

  const save = async () => {
    if (group === "diagnostics") return;
    const key = group as SettingsGroupId;
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
        hit(["settings.launchAtLogin", "settings.startMinimized", "settings.closeToTray", "settings.showTrayIcon", "settings.singleInstance"]) ? "startup" : null,
        hit(["settings.mixedPort", "settings.allowLan", "settings.bypassLocal", "settings.bypassExtra", "settings.tunEnabled"]) ? "inbound" : null,
        hit(["settings.policy", "settings.ruleSet", "settings.blockAds", "settings.dnsStrategy", "settings.dnsServers", "settings.fakeIp", "settings.rules"]) ? "routing" : null,
        hit(["settings.interval", "settings.updateOnStart", "settings.concurrency", "settings.timeout", "settings.userAgent", "settings.keepRemoved"]) ? "subscription" : null,
        hit(["settings.theme", "settings.language", "settings.compact", "settings.reduceMotion"]) ? "appearance" : null,
        hit(["settings.coreOverride", "settings.logLevel", "settings.tunMtu", "settings.sniffing", "settings.tcpFastOpen", "settings.mux", "settings.muxConcurrency"]) ? "advanced" : null,
        hit(["settings.cores", "settings.paths", "settings.reset", "settings.version"]) ? "diagnostics" : null,
      ].filter((value): value is Group => value !== null),
    );
  }, [term, lang, t]);

  const showGroup = (id: Group): boolean => !term || (groupsWithMatches?.has(id) ?? false);
  const activeGroup = showGroup(group) ? group : groups.find((entry) => showGroup(entry.id))?.id ?? "inbound";

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
            {matches("settings.startMinimized") ? (
              <Field label={t("settings.startMinimized")} inline>
                <Toggle checked={value.startMinimized} label={t("settings.startMinimized")} onChange={(checked) => patch("startup", { startMinimized: checked })} />
              </Field>
            ) : null}
            {matches("settings.closeToTray") ? (
              <Field label={t("settings.closeToTray")} inline>
                <Toggle checked={value.closeToTray} label={t("settings.closeToTray")} onChange={(checked) => patch("startup", { closeToTray: checked })} />
              </Field>
            ) : null}
            {matches("settings.showTrayIcon") ? (
              <Field label={t("settings.showTrayIcon")} inline>
                <Toggle checked={value.showTrayIcon} label={t("settings.showTrayIcon")} onChange={(checked) => patch("startup", { showTrayIcon: checked })} />
              </Field>
            ) : null}
            {matches("settings.singleInstance") ? (
              <Field label={t("settings.singleInstance")} inline>
                <Toggle checked={value.singleInstance} label={t("settings.singleInstance")} onChange={(checked) => patch("startup", { singleInstance: checked })} />
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
            {matches("settings.bypassLocal", "settings.bypassLocal.hint") ? (
              <Field label={t("settings.bypassLocal")} hint={t("settings.bypassLocal.hint")} inline>
                <Toggle checked={value.bypassLocal} label={t("settings.bypassLocal")} onChange={(checked) => patch("inbound", { bypassLocal: checked })} />
              </Field>
            ) : null}
            {matches("settings.bypassExtra", "settings.bypassExtra.hint") ? (
              <Field label={t("settings.bypassExtra")} hint={t("settings.bypassExtra.hint")} htmlFor="bypass-extra">
                <textarea
                  id="bypass-extra"
                  className="control control-mono"
                  value={value.bypassExtra}
                  placeholder={"*.company.internal\n10.20.0.0/16"}
                  onChange={(event) => patch("inbound", { bypassExtra: event.target.value })}
                />
              </Field>
            ) : null}
            {matches("settings.allowLan", "settings.allowLan.hint") ? (
              <Field label={t("settings.allowLan")} hint={t("settings.allowLan.hint")} inline>
                <Toggle checked={value.allowLan} label={t("settings.allowLan")} onChange={(checked) => patch("inbound", { allowLan: checked })} />
              </Field>
            ) : null}
            {matches("settings.tunEnabled", "settings.tunEnabled.hint") ? (
              <Field label={t("settings.tunEnabled")} hint={t("settings.tunEnabled.hint")} inline>
                <Toggle checked={value.tunEnabled} label={t("settings.tunEnabled")} onChange={(checked) => patch("inbound", { tunEnabled: checked })} />
              </Field>
            ) : null}
          </>
        );
      }
      case "routing": {
        const value = draftOf("routing");
        return (
          <>
            {matches("settings.policy", "settings.policy.hint") ? (
              <Field label={t("settings.policy")} hint={t("settings.policy.hint")} htmlFor="routing-policy-setting">
                <Select
                  id="routing-policy-setting"
                  label={t("settings.policy")}
                  value={value.policy}
                  onChange={(next) => patch("routing", { policy: next })}
                  options={[
                    { value: "rule", label: t("connection.policy.rule") },
                    { value: "global", label: t("connection.policy.global") },
                    { value: "direct", label: t("connection.policy.direct") },
                  ]}
                />
              </Field>
            ) : null}
            {matches("settings.ruleSet", "settings.ruleSet.hint") ? (
              <Field label={t("settings.ruleSet")} hint={t("settings.ruleSet.hint")} inline>
                <Toggle checked={value.ruleSetEnabled} label={t("settings.ruleSet")} onChange={(checked) => patch("routing", { ruleSetEnabled: checked })} />
              </Field>
            ) : null}
            {matches("settings.blockAds") ? (
              <Field label={t("settings.blockAds")} inline>
                <Toggle checked={value.blockAds} label={t("settings.blockAds")} onChange={(checked) => patch("routing", { blockAds: checked })} />
              </Field>
            ) : null}
            {matches("settings.dnsStrategy", "settings.dnsStrategy.hint") ? (
              <Field label={t("settings.dnsStrategy")} hint={t("settings.dnsStrategy.hint")} htmlFor="dns-strategy">
                <Select
                  id="dns-strategy"
                  label={t("settings.dnsStrategy")}
                  value={value.dnsStrategy}
                  onChange={(next) => patch("routing", { dnsStrategy: next })}
                  options={[
                    { value: "prefer_ipv4", label: "prefer_ipv4" },
                    { value: "prefer_ipv6", label: "prefer_ipv6" },
                    { value: "ipv4_only", label: "ipv4_only" },
                    { value: "ipv6_only", label: "ipv6_only" },
                  ]}
                />
              </Field>
            ) : null}
            {matches("settings.dnsServers", "settings.dnsServers.hint") ? (
              <Field label={t("settings.dnsServers")} hint={t("settings.dnsServers.hint")} htmlFor="dns-servers">
                <textarea
                  id="dns-servers"
                  className="control control-mono"
                  value={value.dnsServers}
                  placeholder={"1.1.1.1\nhttps://dns.google/dns-query"}
                  onChange={(event) => patch("routing", { dnsServers: event.target.value })}
                />
              </Field>
            ) : null}
            {matches("settings.fakeIp", "settings.fakeIp.hint") ? (
              <Field label={t("settings.fakeIp")} hint={t("settings.fakeIp.hint")} inline>
                <Toggle checked={value.fakeIp} label={t("settings.fakeIp")} onChange={(checked) => patch("routing", { fakeIp: checked })} />
              </Field>
            ) : null}
            {matches("settings.rules", "settings.rules.hint") ? <RulesEditor /> : null}
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
            {matches("settings.updateOnStart") ? (
              <Field label={t("settings.updateOnStart")} inline>
                <Toggle checked={value.updateOnStart} label={t("settings.updateOnStart")} onChange={(checked) => patch("subscription", { updateOnStart: checked })} />
              </Field>
            ) : null}
            {matches("settings.concurrency", "settings.concurrency.hint") ? (
              <Field label={t("settings.concurrency")} hint={t("settings.concurrency.hint")} htmlFor="sub-concurrency">
                <input
                  id="sub-concurrency"
                  className="control control-mono"
                  type="number"
                  min={1}
                  max={16}
                  value={value.concurrency}
                  onChange={(event) => patch("subscription", { concurrency: Number(event.target.value) })}
                />
              </Field>
            ) : null}
            {matches("settings.timeout") ? (
              <Field label={t("settings.timeout")} htmlFor="sub-timeout">
                <input
                  id="sub-timeout"
                  className="control control-mono"
                  type="number"
                  min={1}
                  max={120}
                  value={value.timeoutSecs}
                  onChange={(event) => patch("subscription", { timeoutSecs: Number(event.target.value) })}
                />
              </Field>
            ) : null}
            {matches("settings.userAgent", "settings.userAgent.hint") ? (
              <Field label={t("settings.userAgent")} hint={t("settings.userAgent.hint")} htmlFor="sub-agent">
                <input
                  id="sub-agent"
                  className="control control-mono"
                  value={value.userAgent}
                  onChange={(event) => patch("subscription", { userAgent: event.target.value })}
                />
              </Field>
            ) : null}
            {matches("settings.keepRemoved", "settings.keepRemoved.hint") ? (
              <Field label={t("settings.keepRemoved")} hint={t("settings.keepRemoved.hint")} inline>
                <Toggle checked={value.keepRemovedNodes} label={t("settings.keepRemoved")} onChange={(checked) => patch("subscription", { keepRemovedNodes: checked })} />
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
      case "advanced": {
        const value = draftOf("advanced");
        return (
          <>
            {matches("settings.coreOverride", "settings.coreOverride.hint") ? (
              <Field label={t("settings.coreOverride")} hint={t("settings.coreOverride.hint")} htmlFor="core-override">
                <Select
                  id="core-override"
                  label={t("settings.coreOverride")}
                  value={value.coreOverride}
                  onChange={(next) => patch("advanced", { coreOverride: next })}
                  options={[
                    { value: "auto", label: "auto" },
                    { value: "sing-box", label: "sing-box" },
                    { value: "xray", label: "Xray" },
                    { value: "mihomo", label: "mihomo" },
                  ]}
                />
              </Field>
            ) : null}
            {matches("settings.logLevel") ? (
              <Field label={t("settings.logLevel")} htmlFor="log-level">
                <Select
                  id="log-level"
                  label={t("settings.logLevel")}
                  value={value.logLevel}
                  onChange={(next) => patch("advanced", { logLevel: next })}
                  options={[
                    { value: "error", label: "error" },
                    { value: "warn", label: "warn" },
                    { value: "info", label: "info" },
                    { value: "debug", label: "debug" },
                    { value: "trace", label: "trace" },
                  ]}
                />
              </Field>
            ) : null}
            {matches("settings.tunMtu", "settings.tunMtu.hint") ? (
              <Field label={t("settings.tunMtu")} hint={draftOf("inbound").tunEnabled ? t("settings.tunMtu.hint") : `${t("settings.tunMtu.hint")}（${t("connection.mode.tun")}）`} htmlFor="tun-mtu" disabled={!draftOf("inbound").tunEnabled}>
                <input
                  id="tun-mtu"
                  className="control control-mono"
                  type="number"
                  min={1280}
                  max={9000}
                  disabled={!draftOf("inbound").tunEnabled}
                  value={value.tunMtu}
                  onChange={(event) => patch("advanced", { tunMtu: Number(event.target.value) })}
                />
              </Field>
            ) : null}
            {matches("settings.sniffing", "settings.sniffing.hint") ? (
              <Field label={t("settings.sniffing")} hint={t("settings.sniffing.hint")} inline>
                <Toggle checked={value.sniffing} label={t("settings.sniffing")} onChange={(checked) => patch("advanced", { sniffing: checked })} />
              </Field>
            ) : null}
            {matches("settings.tcpFastOpen") ? (
              <Field label={t("settings.tcpFastOpen")} inline>
                <Toggle checked={value.tcpFastOpen} label={t("settings.tcpFastOpen")} onChange={(checked) => patch("advanced", { tcpFastOpen: checked })} />
              </Field>
            ) : null}
            {matches("settings.mux", "settings.mux.hint") ? (
              <Field label={t("settings.mux")} hint={t("settings.mux.hint")} inline>
                <Toggle checked={value.muxEnabled} label={t("settings.mux")} onChange={(checked) => patch("advanced", { muxEnabled: checked })} />
              </Field>
            ) : null}
            {matches("settings.muxConcurrency") ? (
              <Field label={t("settings.muxConcurrency")} htmlFor="mux-concurrency" disabled={!value.muxEnabled}>
                <input
                  id="mux-concurrency"
                  className="control control-mono"
                  type="number"
                  min={1}
                  max={256}
                  disabled={!value.muxEnabled}
                  value={value.muxConcurrency}
                  onChange={(event) => patch("advanced", { muxConcurrency: Number(event.target.value) })}
                />
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
                    <button type="button" className="btn btn-outline btn-sm" onClick={() => void installCore(core.kind)}>
                      {t("settings.cores.install")}
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
                  {activeGroup === "diagnostics" ? t("settings.restartHint") : t("settings.reconnectHint")}
                </p>
              </div>
              <span className="badge">M4</span>
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
                    const key = activeGroup as SettingsGroupId;
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
                  {t("settings.saveReconnect")}
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
