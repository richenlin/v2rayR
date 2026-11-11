/**
 * The single door between the UI and the backend.
 *
 * Pages must import from here and nowhere else (the design document forbids
 * `invoke()` in components). Two implementations share the same shape:
 *
 *  - `tauriBackend` calls the Rust commands registered in `apps/desktop`.
 *  - `mockBackend` runs the same flows entirely in the browser, so the interface
 *    can be reviewed and screenshotted without a desktop build. It is clearly
 *    labelled in the header; it never pretends to touch the system.
 */

import { invoke } from "@tauri-apps/api/core";
import { readStorage, removeStorage, writeStorage } from "./storage";
import type {
  AppSnapshot,
  CoreKind,
  CoreStatus,
  ImportReport,
  LatencyResult,
  LogEntry,
  NodeView,
  RuleView,
  RuleApplication,
  SettingsGroupId,
  SettingsPatch,
  SettingsView,
  SubscriptionView,
  TakeoverMode,
  RoutingPolicy,
  TrafficSample,
  VersionInfo,
} from "./types";

export interface BackupPreview {
  summary: { version: number; rules: number };
  content: unknown;
}

export interface Backend {
  readonly kind: "tauri" | "mock";
  snapshot(): Promise<AppSnapshot>;
  version(): Promise<VersionInfo>;
  connect(profileId: string, mode: TakeoverMode, policy: RoutingPolicy): Promise<AppSnapshot>;
  disconnect(): Promise<AppSnapshot>;
  lanCredentials(): Promise<{ username: string; password: string }>;
  switchNode(profileId: string): Promise<AppSnapshot>;
  setPolicy(policy: RoutingPolicy): Promise<AppSnapshot>;
  setMode(mode: TakeoverMode): Promise<AppSnapshot>;
  retry(): Promise<AppSnapshot>;
  removeNode(id: string): Promise<void>;
  setFavorite(id: string, favorite: boolean): Promise<void>;
  setTags(id: string, tags: string[]): Promise<void>;
  shareLink(id: string): Promise<string>;
  importText(text: string): Promise<{ report: ImportReport; subscriptionIds: string[] }>;
  updatePac(): Promise<number>;
  testLatency(ids: string[]): Promise<LatencyResult[]>;
  addSubscription(url: string, name?: string): Promise<{ added: number; updated: number; removed: number }>;
  refreshSubscription(id: string): Promise<{ added: number; updated: number; removed: number; error?: unknown }>;
  removeSubscription(id: string): Promise<void>;
  setSubscriptionEnabled(id: string, enabled: boolean): Promise<void>;
  saveSettings(patches: SettingsPatch[]): Promise<SettingsView>;
  resetSettings(): Promise<SettingsView>;
  exportBackup(): Promise<boolean>;
  previewBackup(): Promise<BackupPreview | null>;
  applyBackup(content: unknown): Promise<number>;
  readWebDav(url: string, username?: string, password?: string): Promise<BackupPreview>;
  createWebDav(url: string, username?: string, password?: string): Promise<void>;
  updatesConfigured(): Promise<boolean>;
  checkUpdate(): Promise<string | null>;
  installUpdate(expectedVersion: string): Promise<void>;
  cores(): Promise<CoreStatus[]>;
  installCore(kind: CoreKind, url?: string, sha256?: string): Promise<CoreStatus>;
  rules(): Promise<RuleView[]>;
  pickRuleApplications(): Promise<RuleApplication[]>;
  addRule(kind: string, value: string, action: string, applications: RuleApplication[]): Promise<RuleView[]>;
  updateRule(id: string, kind: string, value: string, action: string, applications: RuleApplication[]): Promise<RuleView[]>;
  setRuleEnabled(id: string, enabled: boolean): Promise<RuleView[]>;
  removeRule(id: string): Promise<RuleView[]>;
  logs(limit: number): Promise<LogEntry[]>;
  paths(): Promise<Record<string, string>>;
}

const hasTauri = (): boolean =>
  typeof window !== "undefined" &&
  ("__TAURI_INTERNALS__" in window || "__TAURI__" in window);

/** Normalise a rejected command into a `{ code, key, detail, retryable }`. */
function toProblem(error: unknown): { code: string; key: string; detail?: string; retryable: boolean } {
  if (error && typeof error === "object" && "code" in error && "key" in error) {
    return error as { code: string; key: string; detail?: string; retryable: boolean };
  }
  return {
    code: "internal",
    key: "error.internal",
    detail: typeof error === "string" ? error : String(error),
    retryable: false,
  };
}

const tauriBackend: Backend = {
  kind: "tauri",
  exportBackup: () => invoke("backup_export"),
  previewBackup: () => invoke("backup_preview"),
  applyBackup: (content) => invoke("backup_apply", { content }),
  readWebDav: (url, username, password) => invoke("backup_webdav_read", { url, username, password }),
  createWebDav: (url, username, password) => invoke("backup_webdav_create", { url, username, password }),
  updatesConfigured: () => invoke("app_updates_configured"),
  checkUpdate: () => invoke("app_check_update"),
  installUpdate: (expectedVersion) => invoke("app_install_update", { expectedVersion }),
  snapshot: () => invoke<AppSnapshot>("app_snapshot"),
  version: () => invoke<VersionInfo>("app_version"),
  connect: (profileId, mode, policy) =>
    invoke<AppSnapshot>("session_connect", { request: { profileId, mode, policy } }),
  disconnect: () => invoke<AppSnapshot>("session_disconnect"),
  lanCredentials: () => invoke("session_lan_credentials"),
  switchNode: (profileId) => invoke<AppSnapshot>("session_switch", { profileId }),
  setPolicy: (policy) => invoke<AppSnapshot>("session_set_policy", { policy }),
  setMode: (mode) => invoke<AppSnapshot>("session_set_mode", { mode }),
  retry: () => invoke<AppSnapshot>("session_retry"),
  removeNode: (id) => invoke<void>("node_remove", { id }),
  setFavorite: (id, favorite) => invoke<void>("node_set_favorite", { id, favorite }),
  setTags: (id, tags) => invoke<void>("node_set_tags", { id, tags }),
  shareLink: (id) => invoke<string>("node_share_link", { id }),
  importText: (text) => invoke("nodes_import_text", { text }),
  updatePac: () => invoke("pac_update"),
  testLatency: (ids) => invoke<LatencyResult[]>("nodes_test_latency", { ids }),
  addSubscription: (url, name) => invoke("subscription_add", { url, name }),
  refreshSubscription: (id) => invoke("subscription_update", { id }),
  removeSubscription: (id) => invoke<void>("subscription_remove", { id }),
  setSubscriptionEnabled: (id, enabled) => invoke<void>("subscription_set_enabled", { id, enabled }),
  saveSettings: async (patches) => {
    await invoke("settings_update", { patches });
    const snapshot = await tauriBackend.snapshot();
    return snapshot.settings;
  },
  resetSettings: () => invoke<SettingsView>("settings_reset"),
  cores: () => invoke<CoreStatus[]>("cores_probe"),
  installCore: (kind, url, sha256) => invoke<CoreStatus>("cores_install", { kind, url, sha256 }),
  rules: () => invoke<RuleView[]>("rules_list"),
  pickRuleApplications: () => invoke<RuleApplication[]>("rule_pick_applications"),
  addRule: (kind, value, action, applications) => invoke<RuleView[]>("rule_add", { kind, value, action, applications }),
  updateRule: (id, kind, value, action, applications) => invoke<RuleView[]>("rule_update", { id, kind, value, action, applications }),
  setRuleEnabled: (id, enabled) => invoke<RuleView[]>("rule_set_enabled", { id, enabled }),
  removeRule: (id) => invoke<RuleView[]>("rule_remove", { id }),
  logs: (limit) => invoke<LogEntry[]>("logs_tail", { limit }),
  paths: () => invoke<Record<string, string>>("paths"),
};

const FIELD_NAMES: Record<string, string> = {
  mixedPort: "本地混合端口",
  intervalHours: "更新间隔",
  concurrency: "并发数",
  timeoutSecs: "超时秒数",
  tunMtu: "TUN MTU",
  muxConcurrency: "多路复用并发",
};

/** The same bounds `crates/app/src/settings.rs` enforces. */
function validateSettingsPatch(patch: SettingsPatch): void {
  const values = patch.values as Record<string, unknown>;
  const invalid = (detail: string): never => {
    throw { code: "invalid_value", key: "error.invalid_value", detail, retryable: false };
  };
  const number = (key: string, min: number, max: number, allowZero = false): void => {
    if (!(key in values)) return;
    const raw = Number(values[key]);
    if (!Number.isFinite(raw) || (raw === 0 ? !allowZero : raw < min || raw > max)) {
      invalid(`${FIELD_NAMES[key] ?? key}必须在 ${allowZero ? `0 或 ${min}` : min}–${max} 之间`);
    }
  };
  switch (patch.group) {
    case "inbound":
      number("mixedPort", 1024, 65535, true);
      break;
    case "routing":
      if (typeof values.dnsStrategy === "string" && !["prefer_ipv4", "prefer_ipv6", "ipv4_only", "ipv6_only"].includes(values.dnsStrategy)) {
        invalid("未知的 DNS 策略");
      }
      if (typeof values.dnsServers === "string") {
        const lines = values.dnsServers.split("\n").map((line) => line.trim()).filter((line) => line.length > 0 && !line.startsWith("#"));
        if (lines.length > 8) invalid("DNS 服务器最多 8 个");
      }
      break;
    case "subscription":
      number("intervalHours", 1, 168);
      number("concurrency", 1, 16);
      number("timeoutSecs", 1, 120);
      break;
    case "advanced":
      number("tunMtu", 1280, 9000);
      number("muxConcurrency", 1, 256);
      break;
    default:
      break;
  }
}

/* ------------------------------------------------------------------------- */
/* Mock backend                                                              */
/* ------------------------------------------------------------------------- */

const PROTOCOLS = ["vless", "trojan", "hysteria2", "shadowsocks", "tuic", "anytls"] as const;
const REGIONS = ["东京", "新加坡", "香港", "大阪", "洛杉矶", "法兰克福", "首尔", "悉尼"];
const TRANSPORTS = ["tcp", "ws", "grpc", "httpupgrade", "quic"];

const uuid = (): string =>
  "xxxxxxxx-xxxx-4xxx-yxxx-xxxxxxxxxxxx".replace(/[xy]/g, (char) => {
    const value = (Math.random() * 16) | 0;
    const digit = char === "x" ? value : (value & 0x3) | 0x8;
    return digit.toString(16);
  });

const iso = (offsetMs = 0): string => new Date(Date.now() + offsetMs).toISOString();

function seedNodes(): NodeView[] {
  const nodes: NodeView[] = [];
  const base = Date.now();
  for (let index = 0; index < 14; index += 1) {
    const region = REGIONS[index % REGIONS.length];
    const protocol = PROTOCOLS[index % PROTOCOLS.length];
    const latency = [42, 58, 76, 91, 118, 134, 168, 205, 260][index % 9];
    nodes.push({
      id: uuid(),
      remark: `${region} · 线路 ${String(index + 1).padStart(2, "0")}`,
      protocol,
      transport: TRANSPORTS[index % TRANSPORTS.length],
      address: `node${String(index + 1).padStart(2, "0")}.relay.example.com`,
      port: 443 + (index % 3) * 1000,
      tls: protocol !== "shadowsocks",
      subscriptionId: index < 5 ? "sub-1" : index < 10 ? "sub-2" : undefined,
      subscriptionName: index < 5 ? "Tokyo Relay" : index < 10 ? "Hong Kong Edge" : undefined,
      tags: index % 3 === 0 ? ["工作"] : index % 4 === 0 ? ["流媒体"] : [],
      favorite: index < 3,
      core: protocol === "vless" && index % 7 === 0 ? "xray" : "sing-box",
      latency: {
        ms: latency,
        method: "tcp",
        testedAt: new Date(base - (index + 1) * 60_000).toISOString(),
      },
      lastUsedAt: index === 0 ? new Date(base - 240_000).toISOString() : undefined,
    });
  }
  return nodes;
}

function seedSubscriptions(): SubscriptionView[] {
  const host = (url: string): string => {
    try {
      return new URL(url).host;
    } catch {
      return url;
    }
  };
  return [
    {
      id: "sub-1",
      name: "Tokyo Relay",
      url: "https://sub.example.com/api/v1/client/subscribe?token=abc",
      urlHost: host("https://sub.example.com/api/v1/client/subscribe?token=abc"),
      enabled: true,
      autoUpdate: true,
      intervalHours: 12,
      nodeCount: 5,
      lastUpdatedAt: iso(-42 * 60_000),
      nextUpdateAt: iso(11 * 3600_000),
      quota: { upload: 42 * 1024 * 1024 * 1024, download: 168 * 1024 * 1024 * 1024, total: 500 * 1024 * 1024 * 1024, expire: iso(26 * 86400_000) },
      format: "links",
    },
    {
      id: "sub-2",
      name: "Hong Kong Edge",
      url: "https://edge.example.net/sub/xyz",
      urlHost: host("https://edge.example.net/sub/xyz"),
      enabled: true,
      autoUpdate: true,
      intervalHours: 24,
      nodeCount: 5,
      lastUpdatedAt: iso(-26 * 3600_000),
      nextUpdateAt: iso(22 * 3600_000),
      format: "base64-links",
    },
    {
      id: "sub-3",
      name: "自建节点",
      url: "https://vps.example.org/clash.yaml",
      urlHost: host("https://vps.example.org/clash.yaml"),
      enabled: false,
      autoUpdate: false,
      intervalHours: 168,
      nodeCount: 4,
      lastUpdatedAt: iso(-3 * 86400_000),
      lastError: {
        code: "network_error",
        key: "error.network_error",
        detail: "连接被拒绝（预览模拟）",
        retryable: true,
      },
      format: "clash",
    },
  ];
}

const defaultSettings = (): SettingsView => ({
  revision: 1,
  startup: {
    launchAtLogin: false,
    startMinimized: false,
    closeToTray: true,
    showTrayIcon: true,
    singleInstance: true,
    connectOnLaunch: false,
  },
  inbound: {
    mixedPort: 0,
    dokodemoListenPort: 0,
    dokodemoAddress: "",
    dokodemoTargetPort: 0,
    allowLan: false,
    lanBind: "",
    bypassLocal: true,
    bypassExtra: "",
    tunEnabled: false,
  },
  routing: {
    policy: "rule",
    ruleSetEnabled: true,
    blockAds: false,
    dnsStrategy: "prefer_ipv4",
    dnsServers: "",
    fakeIp: false,
  },
  subscription: {
    intervalHours: 12,
    updateOnStart: false,
    concurrency: 8,
    timeoutSecs: 10,
    userAgent: "v2rayR/<ver>",
    keepRemovedNodes: false,
  },
  appearance: { theme: "system", language: "system", compact: false, reduceMotion: false },
  advanced: {
    coreOverride: "auto",
    logLevel: "warn",
    tunMtu: 1500,
    sniffing: true,
    tcpFastOpen: true,
    muxEnabled: false,
    muxConcurrency: 8,
  },
});

interface MockState {
  nodes: NodeView[];
  subscriptions: SubscriptionView[];
  settings: SettingsView;
  connection: AppSnapshot["connection"];
  samples: TrafficSample[];
  totalDown: number;
  totalUp: number;
  cores: CoreStatus[];
  rules: RuleView[];
  logs: LogEntry[];
  seq: number;
  lastTick: number;
  connectingUntil?: number;
}

const STORAGE_KEY = "v2rayr.preview.v1";

function seedState(): MockState {
  return {
    nodes: seedNodes(),
    subscriptions: seedSubscriptions(),
    settings: defaultSettings(),
    connection: {
      phase: "disconnected",
      mode: "system-proxy",
      policy: "rule",
      takeoverApplied: false,
    },
    samples: [],
    totalDown: 0,
    totalUp: 0,
    cores: [
      {
        kind: "sing-box",
        installed: true,
        version: "1.11.3",
        path: "/Applications/v2rayR.app/Contents/Resources/cores/sing-box",
        protocols: ["vless", "vmess", "trojan", "shadowsocks", "hysteria2", "tuic", "anytls", "naive"],
      },
      {
        kind: "xray",
        installed: true,
        version: "1.8.24",
        path: "/Applications/v2rayR.app/Contents/Resources/cores/xray",
        protocols: ["vless", "vmess", "trojan", "shadowsocks", "wireguard"],
      },
      {
        kind: "mihomo",
        installed: false,
        protocols: ["vless", "vmess", "trojan", "shadowsocks", "hysteria2", "tuic"],
      },
    ],
    rules: [
      { id: "r1", kind: "domain-suffix", value: "company.internal", action: "direct", enabled: true, position: 0 },
      { id: "r2", kind: "geosite", value: "category-ads-all", action: "block", enabled: true, position: 1 },
    ],
    logs: [
      { seq: 1, ts: Date.now() - 90_000, level: "info", target: "app", message: `v2rayR ${import.meta.env.VITE_APP_VERSION} 预览环境已就绪` },
      { seq: 2, ts: Date.now() - 60_000, level: "info", target: "cores", message: "检测到 sing-box 1.11.3 / Xray 1.8.24" },
      { seq: 3, ts: Date.now() - 30_000, level: "warn", target: "subscription", message: "「自建节点」上次更新失败，保留旧节点" },
    ],
    seq: 3,
    lastTick: Date.now(),
  };
}

function loadState(): MockState {
  try {
    const raw = readStorage(STORAGE_KEY);
    if (!raw) return seedState();
    const parsed = JSON.parse(raw) as Partial<MockState>;
    const seed = seedState();
    return {
      ...seed,
      ...parsed,
      settings: {
        ...seed.settings,
        ...parsed.settings,
        startup: { ...seed.settings.startup, ...parsed.settings?.startup },
        inbound: { ...seed.settings.inbound, ...parsed.settings?.inbound },
        routing: { ...seed.settings.routing, ...parsed.settings?.routing },
        subscription: { ...seed.settings.subscription, ...parsed.settings?.subscription },
        appearance: { ...seed.settings.appearance, ...parsed.settings?.appearance },
        advanced: { ...seed.settings.advanced, ...parsed.settings?.advanced },
      },
      rules: parsed.rules ?? seed.rules,
      cores: seed.cores,
      logs: parsed.logs ?? seed.logs,
      lastTick: Date.now(),
      connectingUntil: undefined,
    };
  } catch {
    return seedState();
  }
}

const state = loadState();

function persist(): void {
  try {
    writeStorage(
      STORAGE_KEY,
      JSON.stringify({
        nodes: state.nodes,
        subscriptions: state.subscriptions,
        settings: state.settings,
        connection: state.connection,
        samples: state.samples.slice(-60),
        totalDown: state.totalDown,
        totalUp: state.totalUp,
        logs: state.logs.slice(-200),
        seq: state.seq,
      }),
    );
  } catch {
    // Quota or private mode: the preview still works, it just forgets.
  }
}

function log(level: LogEntry["level"], target: string, message: string): void {
  state.seq += 1;
  state.logs.push({ seq: state.seq, ts: Date.now(), level, target, message });
  if (state.logs.length > 400) state.logs.splice(0, state.logs.length - 400);
}

const wait = (ms: number): Promise<void> => new Promise((resolve) => setTimeout(resolve, ms));

function advanceTraffic(): void {
  const now = Date.now();
  if (state.connection.phase === "connected") {
    const elapsed = Math.min(8000, now - state.lastTick);
    for (let stamp = state.lastTick + 1000; stamp <= now; stamp += 1000) {
      const down = Math.round(2_400_000 + Math.sin(stamp / 9000) * 700_000 + (Math.random() - 0.5) * 900_000);
      const up = Math.round(186_000 + Math.cos(stamp / 7000) * 60_000 + (Math.random() - 0.5) * 40_000);
      state.samples.push({ ts: stamp, downBps: Math.max(0, down), upBps: Math.max(0, up) });
      state.totalDown += Math.max(0, down);
      state.totalUp += Math.max(0, up);
    }
    if (state.samples.length > 60) state.samples.splice(0, state.samples.length - 60);
    void elapsed;
  }
  state.lastTick = now;
}

function trafficView(): AppSnapshot["traffic"] {
  const last = state.samples.at(-1);
  return {
    downBps: state.connection.phase === "connected" ? (last?.downBps ?? 0) : 0,
    upBps: state.connection.phase === "connected" ? (last?.upBps ?? 0) : 0,
    totalDownBytes: state.totalDown,
    totalUpBytes: state.totalUp,
    connections: state.connection.phase === "connected" ? 12 + (state.samples.length % 5) : 0,
    samples: state.samples,
  };
}

function snapshot(): AppSnapshot {
  advanceTraffic();
  if (state.connectingUntil && Date.now() >= state.connectingUntil) {
    state.connectingUntil = undefined;
    state.connection.phase = "connected";
    state.connection.startedAt = iso();
    state.connection.takeoverApplied = state.connection.mode !== "manual";
  }
  if (state.connection.phase === "disconnected" || state.connection.phase === "failed") {
    // Nothing is listening; showing a port here would be a stale readout.
    state.connection.inboundPort = undefined;
    state.connection.apiPort = undefined;
  }
  const active = state.nodes.find((node) => node.id === state.connection.profileId);
  return {
    version: {
      appVersion: import.meta.env.VITE_APP_VERSION,
      apiVersion: 1,
      settingsSchema: 1,
      dbSchema: 1,
      commit: "preview",
    },
    connection: { ...state.connection, policy: state.connection.mode === "tun" ? "global" : state.connection.policy, lan: state.connection.phase === "connected" && state.settings.inbound.allowLan
      ? { address: state.settings.inbound.lanBind ?? "", socksPort: 10818, httpPort: 10818 }
      : undefined },
    settings: structuredClone(state.settings),
    activeNode: active,
    nodes: structuredClone(state.nodes),
    subscriptions: structuredClone(state.subscriptions),
    traffic: trafficView(),
    cores: structuredClone(state.cores),
    warnings: [
      {
        code: "core_missing",
        key: "error.core_missing",
        detail: "mihomo 未安装，选择 mihomo 内核的节点会提示安装路径",
        retryable: false,
      },
    ],
  };
}

function requireNode(id: string): NodeView {
  const node = state.nodes.find((item) => item.id === id);
  if (!node) {
    throw { code: "not_found", key: "error.not_found", detail: "节点不存在", retryable: false };
  }
  return node;
}

const mockBackend: Backend = {
  updatePac: async () => 0,
  kind: "mock",
  exportBackup: async () => false,
  previewBackup: async () => null,
  applyBackup: async () => 0,
  readWebDav: async () => { throw new Error("WebDAV requires the desktop app"); },
  createWebDav: async () => { throw new Error("WebDAV requires the desktop app"); },
  updatesConfigured: async () => false,
  checkUpdate: async () => null,
  installUpdate: async () => { throw new Error("Updates require a signed desktop build"); },
  snapshot: async () => {
    await wait(30);
    return snapshot();
  },
  version: async () => snapshot().version,
  connect: async (profileId, mode, policy) => {
    const node = requireNode(profileId);
    state.connection = {
      phase: "connecting",
      mode,
      policy: mode === "tun" ? state.connection.policy : policy,
      profileId: node.id,
      profileRemark: node.remark,
      core: node.core,
      inboundPort: mode === "tun" ? 10814 : 10808,
      apiPort: 10812,
      takeoverApplied: false,
    };
    persist();
    await wait(900);
    if (mode === "tun") {
      state.connection = {
        ...state.connection,
        phase: "failed",
        takeoverApplied: false,
        takeoverError: "TUN 模式需要授权特权 helper；预览环境无法完成授权。",
        error: {
          code: "permission_denied",
          key: "error.permission_denied",
          detail: "TUN 需要以 root 运行的 helper 创建虚拟网卡；预览环境未接通，可改用系统代理或 PAC。",
          retryable: false,
        },
      };
      log("error", "session", "TUN 接管失败，内核已停止");
      persist();
      throw state.connection.error;
    }
    state.connection = {
      ...state.connection,
      phase: "connected",
      startedAt: iso(),
      takeoverApplied: mode !== "manual",
    };
    node.lastUsedAt = iso();
    log("info", "session", `已连接 ${node.remark}（${node.core}）`);
    persist();
    return snapshot();
  },
  lanCredentials: async () => {
    if (!snapshot().connection.lan) throw new Error("No active LAN proxy");
    return { username: "v2rayr", password: "preview-only" };
  },
  disconnect: async () => {
    state.connection = { ...state.connection, phase: "disconnecting" };
    await wait(320);
    state.connection = {
      ...state.connection,
      phase: "disconnected",
      takeoverApplied: false,
      takeoverError: undefined,
      error: undefined,
      profileId: undefined,
      profileRemark: undefined,
      core: undefined,
      startedAt: undefined,
    };
    log("info", "session", "已断开，系统代理已恢复");
    persist();
    return snapshot();
  },
  switchNode: async (profileId) => {
    const node = requireNode(profileId);
    if (state.connection.phase !== "connected") {
      return mockBackend.connect(profileId, state.connection.mode, state.connection.policy);
    }
    state.connection = { ...state.connection, phase: "connecting", profileId: node.id, profileRemark: node.remark, core: node.core };
    await wait(600);
    state.connection = { ...state.connection, phase: "connected", startedAt: iso(), takeoverApplied: state.connection.mode !== "manual" };
    log("info", "session", `已切换到 ${node.remark}`);
    persist();
    return snapshot();
  },
  setPolicy: async (policy) => {
    if (state.connection.mode === "tun") return snapshot();
    state.settings.routing.policy = policy;
    if (state.connection.phase === "connected") {
      state.connection = { ...state.connection, phase: "connecting", policy };
      await wait(420);
      state.connection = { ...state.connection, phase: "connected", startedAt: iso() };
    } else {
      state.connection = { ...state.connection, policy };
    }
    persist();
    return snapshot();
  },
  setMode: async (mode) => {
    if (state.connection.phase === "connected") {
      state.connection = { ...state.connection, mode };
    } else {
      state.connection = {
        ...state.connection,
        mode,
        phase: "disconnected",
        inboundPort: undefined,
        apiPort: undefined,
        takeoverError: undefined,
        error: undefined,
      };
    }
    if (state.connection.phase === "connected") {
      state.connection = { ...state.connection, phase: "connecting" };
      await wait(520);
      state.connection = { ...state.connection, phase: "connected", takeoverApplied: mode !== "manual" };
    }
    persist();
    return snapshot();
  },
  retry: async () => {
    const id = state.connection.profileId;
    if (!id) throw { code: "not_connected", key: "error.not_connected", retryable: false };
    return mockBackend.connect(id, state.connection.mode, state.connection.policy);
  },
  removeNode: async (id) => {
    state.nodes = state.nodes.filter((node) => node.id !== id);
    log("info", "nodes", "已删除节点");
    persist();
  },
  setFavorite: async (id, favorite) => {
    const node = requireNode(id);
    node.favorite = favorite;
    persist();
  },
  setTags: async (id, tags) => {
    const node = requireNode(id);
    node.tags = tags;
    persist();
  },
  shareLink: async (id) => {
    const node = requireNode(id);
    return `${node.protocol}://${node.id}@${node.address}:${node.port}#${encodeURIComponent(node.remark)}`;
  },
  importText: async (text) => {
    await wait(320);
    const failures: ImportReport["failures"] = [];
    const added: NodeView[] = [];
    const subscriptionUrls: string[] = [];
    const subscriptionIds: string[] = [];
    const lines = text.split(/\r?\n/).map((line) => line.trim()).filter(Boolean);
    let duplicates = 0;
    for (const line of lines) {
      if (/^https?:\/\//i.test(line)) {
        subscriptionUrls.push(line);
        continue;
      }
      const match = line.match(/^([a-z0-9+.-]+):\/\/(?:[^@]*@)?(\[[^\]]+\]|[^:/?#]+):(\d+)/i);
      if (!match) {
        failures.push({
          source: `${line.slice(0, 24)}…`,
          error: { code: "not_a_profile_link", key: "error.not_a_profile_link", detail: "无法识别的链接", retryable: false },
        });
        continue;
      }
      const scheme = match[1].toLowerCase();
      const protocol = scheme === "http-proxy" || scheme === "https-proxy" ? "http" : scheme === "socks5" ? "socks" : scheme;
      if (!["vless", "vmess", "trojan", "ss", "shadowsocks", "hysteria2", "hy2", "tuic", "anytls", "naive", "http", "socks"].includes(protocol)) {
        failures.push({
          source: `${protocol}://…`,
          error: { code: "unsupported_scheme", key: "error.unsupported_scheme", detail: protocol, retryable: false },
        });
        continue;
      }
      const address = match[2].replace(/^\[|\]$/g, "");
      const port = Number(match[3]);
      const fragment = line.split("#")[1] ?? "";
      const remark = decodeURIComponent(fragment) || `${protocol} ${address}`;
      if (state.nodes.some((node) => node.address === address && node.port === port && node.protocol === (protocol === "hy2" ? "hysteria2" : protocol))) {
        duplicates += 1;
        continue;
      }
      const node: NodeView = {
        id: uuid(),
        remark,
        protocol: protocol === "hy2" ? "hysteria2" : protocol,
        transport: "tcp",
        address,
        port,
        tls: protocol === "http" ? scheme === "https-proxy" : !["ss", "shadowsocks", "socks"].includes(protocol),
        tags: [],
        favorite: false,
        core: protocol === "vless" ? "xray" : "sing-box",
      };
      state.nodes.unshift(node);
      added.push(node);
    }
    for (const url of subscriptionUrls) {
      const existing = state.subscriptions.find((subscription) => subscription.url === url);
      if (existing) {
        subscriptionIds.push(existing.id);
      } else {
        const host = (() => {
          try {
            return new URL(url).host;
          } catch {
            return url;
          }
        })();
        const id = uuid();
        subscriptionIds.push(id);
        state.subscriptions.unshift({
          id,
          name: host,
          url,
          urlHost: host,
          enabled: true,
          autoUpdate: true,
          intervalHours: state.settings.subscription.intervalHours,
          nodeCount: 0,
          format: "links",
        });
      }
    }
    log("info", "import", `导入完成：新增 ${added.length}，重复 ${duplicates}，失败 ${failures.length}`);
    persist();
    return { report: { added, duplicates, failures, subscriptionUrls }, subscriptionIds };
  },
  testLatency: async (ids) => {
    const targets = ids.length ? state.nodes.filter((node) => ids.includes(node.id)) : state.nodes;
    const results: LatencyResult[] = [];
    const queue = [...targets];
    const workers = Array.from({ length: Math.min(8, queue.length) }, async () => {
      for (;;) {
        const node = queue.shift();
        if (!node) return;
        await wait(180 + Math.random() * 700);
        if (Math.random() < 0.07) {
          results.push({
            id: node.id,
            error: { code: "network_error", key: "error.network_error", detail: "连接超时（预览模拟）", retryable: true },
          });
          continue;
        }
        const ms = Math.round(28 + Math.random() * 240);
        node.latency = { ms, method: "tcp", testedAt: iso() };
        results.push({ id: node.id, latency: node.latency });
      }
    });
    await Promise.all(workers);
    log("info", "latency", `测速完成：${results.length} 个节点`);
    persist();
    return results;
  },
  addSubscription: async (url, name) => {
    await wait(400);
    const host = (() => {
      try {
        return new URL(url).host;
      } catch {
        throw { code: "invalid_url", key: "error.invalid_url", detail: "订阅地址无效", retryable: false };
      }
    })();
    if (!/^https?:/i.test(url)) {
      throw { code: "invalid_url", key: "error.invalid_url", detail: "只接受 http(s)", retryable: false };
    }
    if (state.subscriptions.some((subscription) => subscription.url === url)) {
      throw { code: "duplicate_node", key: "error.duplicate_node", detail: "订阅已存在", retryable: false };
    }
    const added = 3 + Math.floor(Math.random() * 6);
    const subscription: SubscriptionView = {
      id: uuid(),
      name: name?.trim() || host,
      url,
      urlHost: host,
      enabled: true,
      autoUpdate: true,
      intervalHours: state.settings.subscription.intervalHours,
      nodeCount: added,
      lastUpdatedAt: iso(),
      nextUpdateAt: iso(state.settings.subscription.intervalHours * 3600_000),
      format: "links",
    };
    state.subscriptions.unshift(subscription);
    for (let index = 0; index < added; index += 1) {
      state.nodes.push({
        id: uuid(),
        remark: `${subscription.name} · ${String(index + 1).padStart(2, "0")}`,
        protocol: PROTOCOLS[index % PROTOCOLS.length],
        transport: TRANSPORTS[index % TRANSPORTS.length],
        address: `edge${index + 1}.${host}`,
        port: 443,
        tls: true,
        subscriptionId: subscription.id,
        subscriptionName: subscription.name,
        tags: [],
        favorite: false,
        core: "sing-box",
      });
    }
    log("info", "subscription", `已添加订阅 ${subscription.name}`);
    persist();
    return { added, updated: 0, removed: 0 };
  },
  refreshSubscription: async (id) => {
    const subscription = state.subscriptions.find((item) => item.id === id);
    if (!subscription) throw { code: "not_found", key: "error.not_found", retryable: false };
    await wait(700);
    const added = Math.random() < 0.5 ? 0 : Math.floor(Math.random() * 3);
    subscription.lastUpdatedAt = iso();
    subscription.nextUpdateAt = iso(subscription.intervalHours * 3600_000);
    subscription.nodeCount += added;
    delete subscription.lastError;
    log("info", "subscription", `订阅「${subscription.name}」已更新：新增 ${added}`);
    persist();
    return { added, updated: Math.max(0, subscription.nodeCount - added), removed: 0 };
  },
  removeSubscription: async (id) => {
    state.subscriptions = state.subscriptions.filter((item) => item.id !== id);
    state.nodes = state.nodes.filter((node) => node.subscriptionId !== id || node.favorite);
    log("info", "subscription", "已删除订阅");
    persist();
  },
  setSubscriptionEnabled: async (id, enabled) => {
    const subscription = state.subscriptions.find((item) => item.id === id);
    if (subscription) subscription.enabled = enabled;
    persist();
  },
  saveSettings: async (patches) => {
    for (const patch of patches) {
      // Mirror `Settings::validate` in crates/app: the preview must fail the
      // same input the real backend rejects, or it teaches the wrong thing.
      if (patch.group === "inbound" && patch.values.allowLan) {
        const parts = String(patch.values.lanBind ?? "").trim().split(".");
        const ip = parts.map(Number);
        if (parts.length !== 4 || parts.some((part, i) => !/^\d{1,3}$/.test(part) || String(ip[i]) !== part || ip[i] > 255)
          || !(ip[0] === 10 || ip[0] === 192 && ip[1] === 168 || ip[0] === 172 && ip[1] >= 16 && ip[1] <= 31)) {
          throw { code: "invalid_value", key: "error.invalid_value", detail: "LAN requires a private IPv4 address.", retryable: false };
        }
      }
      validateSettingsPatch(patch);
    }
    for (const patch of patches) {
      const group = patch.group as SettingsGroupId;
      const target = state.settings[group] as unknown as Record<string, unknown>;
      Object.assign(target, patch.values);
    }
    state.settings.revision += 1;
    log("info", "settings", `设置已保存（revision ${state.settings.revision}）`);
    persist();
    return snapshot().settings;
  },
  resetSettings: async () => {
    state.settings = { ...defaultSettings(), revision: state.settings.revision + 1 };
    log("info", "settings", "设置已恢复默认");
    persist();
    return snapshot().settings;
  },
  cores: async () => {
    await wait(260);
    return structuredClone(state.cores);
  },
  installCore: async (kind) => {
    throw {
      code: "core_missing",
      key: "error.core_missing",
      detail: `预览环境不会下载 ${kind}；请把二进制放到 cores/ 目录`,
      retryable: false,
    };
  },
  rules: async () => {
    await wait(20);
    return state.rules.map((rule) => ({ ...rule }));
  },
  pickRuleApplications: async () => [],
  addRule: async (kind, value, action, applications) => {
    await wait(60);
    const trimmed = value.trim();
    if (!trimmed) {
      throw { code: "invalid_value", key: "error.invalid_value", detail: "规则内容不能为空", retryable: false };
    }
    state.rules.push({ id: `r${Date.now()}`, kind, value: trimmed, action, applications, enabled: true, position: state.rules.length });
    log("info", "rules", `新增规则 ${kind} ${trimmed}`);
    persist();
    return state.rules.map((rule) => ({ ...rule }));
  },
  updateRule: async (id, kind, value, action, applications) => {
    const rule = state.rules.find((item) => item.id === id);
    if (!rule) throw { code: "not_found", key: "error.not_found", retryable: false };
    const trimmed = value.trim();
    if (!trimmed) throw { code: "invalid_value", key: "error.invalid_value", detail: "规则内容不能为空", retryable: false };
    Object.assign(rule, { kind, value: trimmed, action, applications });
    persist();
    return state.rules.map((item) => ({ ...item }));
  },
  setRuleEnabled: async (id, enabled) => {
    await wait(40);
    const rule = state.rules.find((item) => item.id === id);
    if (!rule) throw { code: "not_found", key: "error.not_found", retryable: false };
    rule.enabled = enabled;
    persist();
    return state.rules.map((item) => ({ ...item }));
  },
  removeRule: async (id) => {
    await wait(40);
    const index = state.rules.findIndex((item) => item.id === id);
    if (index < 0) throw { code: "not_found", key: "error.not_found", retryable: false };
    state.rules.splice(index, 1);
    persist();
    return state.rules.map((rule) => ({ ...rule }));
  },

  logs: async (limit) => state.logs.slice(-limit),
  paths: async () => ({
    root: "~/Library/Application Support/v2rayR",
    database: "~/Library/Application Support/v2rayR/data/v2rayr.sqlite3",
    settings: "~/Library/Application Support/v2rayR/config/settings.json",
    logs: "~/Library/Application Support/v2rayR/logs",
    cores: "~/Library/Application Support/v2rayR/data/cores",
  }),
};

export const api: Backend = hasTauri() ? tauriBackend : mockBackend;
export const isPreview = api.kind === "mock";
export { toProblem };

/** Reset the preview to its seeded state; used by the "preview scenario" group. */
export function resetPreviewData(): void {
  removeStorage(STORAGE_KEY);
  window.location.reload();
}
