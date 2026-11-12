/**
 * Wire types, mirroring `crates/ipc`.
 *
 * These are hand-written on purpose: the Rust side owns the contract and the
 * contract test in `crates/ipc/tests/contract.rs` pins field names. A generated
 * binding would drift silently; this file at least fails loudly in review.
 */

export type ConnectionPhase =
  | "disconnected"
  | "connecting"
  | "connected"
  | "disconnecting"
  | "failed";

export type TakeoverMode = "system-proxy" | "pac" | "tun" | "manual";
export type RoutingPolicy = "rule" | "global" | "direct";
export type CoreKind = "sing-box" | "xray" | "mihomo";
export type Theme = "system" | "light" | "dark";
export type Language = "system" | "zh-cn" | "en-us";
export type CoreOverride = "auto" | "sing-box" | "xray" | "mihomo";
export type LogLevel = "trace" | "debug" | "info" | "warn" | "error";
export type LatencyMethod = "tcp" | "http" | "through-kernel";

export interface Problem {
  code: string;
  key: string;
  detail?: string;
  requiredCore?: CoreKind;
  retryable: boolean;
}

export interface LatencyView {
  ms: number;
  method: LatencyMethod;
  testedAt: string;
}

export interface NodeView {
  id: string;
  remark: string;
  protocol: string;
  transport: string;
  address: string;
  port: number;
  tls: boolean;
  subscriptionId?: string;
  subscriptionName?: string;
  tags: string[];
  favorite: boolean;
  core: CoreKind;
  latency?: LatencyView;
  lastUsedAt?: string;
}

export interface QuotaView {
  upload: number;
  download: number;
  total: number;
  expire?: string;
}

export interface SubscriptionView {
  id: string;
  name: string;
  url: string;
  urlHost: string;
  enabled: boolean;
  autoUpdate: boolean;
  intervalHours: number;
  nodeCount: number;
  lastUpdatedAt?: string;
  nextUpdateAt?: string;
  lastError?: Problem;
  quota?: QuotaView;
  format?: string;
}

export interface CoreStatus {
  kind: CoreKind;
  installed: boolean;
  version?: string;
  path?: string;
  protocols: string[];
  lastError?: Problem;
}

export interface ConnectionState {
  phase: ConnectionPhase;
  mode: TakeoverMode;
  policy: RoutingPolicy;
  requestedPolicy?: RoutingPolicy;
  routingWarnings?: string[];
  profileId?: string;
  profileRemark?: string;
  core?: CoreKind;
  inboundPort?: number;
  apiPort?: number;
  pacUrl?: string;
  startedAt?: string;
  takeoverApplied: boolean;
  lan?: { address: string; socksPort: number; httpPort: number };
  takeoverError?: Problem | string;
  error?: Problem;
}

export interface TrafficSample {
  ts: number;
  downBps: number;
  upBps: number;
}

export interface TrafficView {
  downBps: number;
  upBps: number;
  totalDownBytes: number;
  totalUpBytes: number;
  connections: number;
  /** Omitted by the backend while a session has no samples yet. */
  samples?: TrafficSample[];
}

export interface StartupSettings {
  launchAtLogin: boolean;
  startMinimized: boolean;
  closeToTray: boolean;
  showTrayIcon: boolean;
  singleInstance: boolean;
  connectOnLaunch: boolean;
}

export interface InboundSettings {
  mixedPort: number;
  dokodemoListenPort: number;
  dokodemoAddress: string;
  dokodemoTargetPort: number;
  allowLan: boolean;
  lanBind?: string;
  bypassLocal: boolean;
  bypassExtra: string;
  tunEnabled: boolean;
}

export interface RoutingSettings {
  policy: RoutingPolicy;
  ruleSetEnabled: boolean;
  blockAds: boolean;
  dnsStrategy: string;
  dnsServers: string;
  fakeIp: boolean;
}

export interface SubscriptionSettings {
  intervalHours: number;
  updateOnStart: boolean;
  concurrency: number;
  timeoutSecs: number;
  userAgent: string;
  keepRemovedNodes: boolean;
}

export interface AppearanceSettings {
  theme: Theme;
  language: Language;
  compact: boolean;
  reduceMotion: boolean;
}

export interface AdvancedSettings {
  coreOverride: CoreOverride;
  logLevel: LogLevel;
  tunMtu: number;
  sniffing: boolean;
  tcpFastOpen: boolean;
  muxEnabled: boolean;
  muxConcurrency: number;
}

export interface Settings {
  schema: number;
  startup: StartupSettings;
  inbound: InboundSettings;
  routing: RoutingSettings;
  subscription: SubscriptionSettings;
  appearance: AppearanceSettings;
  advanced: AdvancedSettings;
}

export type SettingsGroupId =
  | "startup"
  | "inbound"
  | "routing"
  | "subscription"
  | "appearance"
  | "advanced";

export interface SettingsView {
  revision: number;
  startup: StartupSettings;
  inbound: InboundSettings;
  routing: RoutingSettings;
  subscription: SubscriptionSettings;
  appearance: AppearanceSettings;
  advanced: AdvancedSettings;
}

export interface SettingsPatch {
  group: SettingsGroupId;
  values: Record<string, unknown>;
}

export interface VersionInfo {
  appVersion: string;
  apiVersion: number;
  settingsSchema: number;
  dbSchema: number;
  commit?: string;
}

export interface AppSnapshot {
  version: VersionInfo;
  connection: ConnectionState;
  tunAvailable?: boolean;
  tunRuleRouting?: boolean;
  settings: SettingsView;
  activeNode?: NodeView;
  nodes: NodeView[];
  subscriptions: SubscriptionView[];
  traffic: TrafficView;
  cores: CoreStatus[];
  warnings: Problem[];
}

export interface ImportFailure {
  source: string;
  error: Problem;
}

export interface ImportReport {
  added: NodeView[];
  duplicates: number;
  failures: ImportFailure[];
  subscriptionUrls: string[];
  detectedFormat?: string;
}

export interface LatencyResult {
  id: string;
  latency?: LatencyView;
  error?: Problem;
}

export interface LogEntry {
  seq: number;
  ts: number;
  level: LogLevel;
  target: string;
  message: string;
}

export const emptyTraffic: TrafficView = {
  downBps: 0,
  upBps: 0,
  totalDownBytes: 0,
  totalUpBytes: 0,
  connections: 0,
  samples: [],
};

export interface RuleApplication {
  path: string;
  sha256: string;
}

/** One custom routing rule (mirrors `v2rayr_ipc::RuleView`). */
export interface RuleView {
  id: string;
  kind: "domain" | "domain-suffix" | "ip-cidr" | "process" | "geoip" | "geosite" | string;
  value: string;
  action: "proxy" | "direct" | "block" | string;
  enabled: boolean;
  position: number;
  applications?: RuleApplication[];
}
