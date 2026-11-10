/**
 * One store for the whole window.
 *
 * Deliberately plain: every mutation goes through an `api` call, then refreshes
 * the snapshot. Optimistic updates are limited to UI-only state (filters, drafts,
 * toasts) so the screen never claims a state the backend has not accepted.
 */

import { create } from "zustand";
import { api, isPreview, toProblem } from "./lib/api";
import { resolveLang, translate, type Lang } from "./lib/i18n";
import { readStorage, writeStorage } from "./lib/storage";
import type {
  AppSnapshot,
  CoreKind,
  LatencyResult,
  Problem,
  RuleView,
  RuleApplication,
  SettingsPatch,
  TakeoverMode,
  RoutingPolicy,
} from "./lib/types";

export type Page = "connection" | "nodes" | "subscriptions" | "speedtest" | "settings";

export interface Toast {
  id: number;
  tone: "ok" | "info" | "danger";
  title: string;
  detail?: string;
}

interface State {
  snapshot?: AppSnapshot;
  loading: boolean;
  bootError?: string;
  page: Page;
  toasts: Toast[];
  busy: string | null;
  themeMode: "light" | "dark";
  lang: Lang;
  testing: boolean;
  testProgress: { done: number; total: number };
  lastResults: LatencyResult[];
  nodeQuery: string;
  nodeScope: "all" | "favorites";
  nodeProtocol: string;
  rules: RuleView[];

  boot: () => Promise<void>;
  refresh: () => Promise<void>;
  navigate: (page: Page) => void;
  toast: (tone: Toast["tone"], title: string, detail?: string) => void;
  dismissToast: (id: number) => void;
  toggleTheme: () => void;
  setLang: (lang: Lang) => void;

  connect: (id: string, mode: TakeoverMode, policy: RoutingPolicy) => Promise<void>;
  disconnect: () => Promise<void>;
  switchNode: (id: string) => Promise<void>;
  setMode: (mode: TakeoverMode) => Promise<void>;
  setPolicy: (policy: RoutingPolicy) => Promise<void>;
  retry: () => Promise<void>;

  importNodes: (text: string) => Promise<{ added: number; duplicates: number; failures: number; subscriptions: number }>;
  removeNode: (id: string) => Promise<void>;
  toggleFavorite: (id: string, favorite: boolean) => Promise<void>;
  saveTags: (id: string, tags: string[]) => Promise<void>;
  copyLink: (id: string) => Promise<void>;
  testLatency: (ids?: string[]) => Promise<void>;

  addSubscription: (url: string, name?: string) => Promise<void>;
  refreshSubscription: (id: string) => Promise<void>;
  removeSubscription: (id: string) => Promise<void>;
  setSubscriptionEnabled: (id: string, enabled: boolean) => Promise<void>;

  saveSettings: (patches: SettingsPatch[]) => Promise<boolean>;
  loadRules: () => Promise<void>;
  addRule: (kind: string, value: string, action: string, applications: RuleApplication[]) => Promise<boolean>;
  updateRule: (id: string, kind: string, value: string, action: string, applications: RuleApplication[]) => Promise<boolean>;
  toggleRule: (id: string, enabled: boolean) => Promise<void>;
  removeRule: (id: string) => Promise<void>;
  probeCores: () => Promise<void>;
  installCore: (kind: CoreKind) => Promise<boolean>;
  resetSettings: () => Promise<void>;

  setNodeQuery: (query: string) => void;
  setNodeScope: (scope: "all" | "favorites") => void;
  setNodeProtocol: (protocol: string) => void;
}

let toastSeq = 0;

function detectInitialTheme(): "light" | "dark" {
  if (typeof window === "undefined") return "light";
  const stored = readStorage("v2rayr.theme");
  if (stored === "light" || stored === "dark") return stored;
  return window.matchMedia?.("(prefers-color-scheme: dark)").matches ? "dark" : "light";
}

export const useStore = create<State>((set, get) => ({
  loading: true,
  page: "connection",
  toasts: [],
  busy: null,
  themeMode: detectInitialTheme(),
  lang: resolveLang(undefined),
  testing: false,
  testProgress: { done: 0, total: 0 },
  lastResults: [],
  nodeQuery: "",
  nodeScope: "all",
  nodeProtocol: "all",
  rules: [],

  boot: async () => {
    try {
      const snapshot = await api.snapshot();
      set({
        snapshot,
        loading: false,
        bootError: undefined,
        page: pageFromHash(),
        lang: resolveLang(snapshot.settings.appearance.language),
        themeMode:
          snapshot.settings.appearance.theme === "system"
            ? detectInitialTheme()
            : snapshot.settings.appearance.theme,
      });
      applyDocumentTheme(get().themeMode);
      applyDocumentLang(get().lang);
      void get().loadRules();
    } catch (error) {
      set({ loading: false, bootError: problemTextSafe(get().lang, toProblem(error)) });
    }
  },

  refresh: async () => {
    try {
      const snapshot = await api.snapshot();
      set({ snapshot });
    } catch {
      // A poll failure is not worth a toast; the next tick will tell.
    }
  },

  navigate: (page) => {
    set({ page });
    if (typeof window !== "undefined") {
      window.location.hash = `#/${page}`;
    }
  },

  toast: (tone, title, detail) => {
    toastSeq += 1;
    const id = toastSeq;
    set({ toasts: [...get().toasts, { id, tone, title, detail }].slice(-4) });
    window.setTimeout(() => get().dismissToast(id), tone === "danger" ? 7000 : 4200);
  },

  dismissToast: (id) => set({ toasts: get().toasts.filter((toast) => toast.id !== id) }),

  toggleTheme: () => {
    const next = get().themeMode === "dark" ? "light" : "dark";
    writeStorage("v2rayr.theme", next);
    set({ themeMode: next });
    applyDocumentTheme(next);
  },

  setLang: (lang) => {
    set({ lang });
    applyDocumentLang(lang);
  },

  connect: async (id, mode, policy) => {
    set({ busy: "connect" });
    try {
      const snapshot = await api.connect(id, mode, policy);
      set({ snapshot });
      get().toast("ok", "已连接", snapshot.connection.profileRemark);
    } catch (error) {
      const problem = toProblem(error) as Problem;
      await get().refresh();
      if (problem.requiredCore) get().navigate("connection");
      get().toast("danger", translate(get().lang, "connection.failed"), problemTextSafe(get().lang, problem));
    } finally {
      set({ busy: null });
    }
  },

  disconnect: async () => {
    set({ busy: "disconnect" });
    try {
      const snapshot = await api.disconnect();
      set({ snapshot });
      get().toast("info", translate(get().lang, "connection.disconnected"));
    } catch (error) {
      const problem = toProblem(error) as Problem;
      get().toast("danger", translate(get().lang, "connection.failed"), problemTextSafe(get().lang, problem));
    } finally {
      set({ busy: null });
    }
  },

  switchNode: async (id) => {
    set({ busy: "switch" });
    try {
      const snapshot = await api.switchNode(id);
      set({ snapshot });
      get().toast("ok", translate(get().lang, "connection.current"), snapshot.connection.profileRemark);
    } catch (error) {
      const problem = toProblem(error) as Problem;
      await get().refresh();
      get().toast("danger", translate(get().lang, "connection.failed"), problemTextSafe(get().lang, problem));
    } finally {
      set({ busy: null });
    }
  },

  setMode: async (mode) => {
    try {
      const snapshot = await api.setMode(mode);
      set({ snapshot });
    } catch (error) {
      const problem = toProblem(error) as Problem;
      get().toast("danger", translate(get().lang, "connection.failed"), problemTextSafe(get().lang, problem));
      await get().refresh();
    }
  },

  setPolicy: async (policy) => {
    try {
      const snapshot = await api.setPolicy(policy);
      set({ snapshot });
      get().toast("info", translate(get().lang, "connection.policy"), policyLabel(get().lang, policy));
    } catch (error) {
      const problem = toProblem(error) as Problem;
      get().toast("danger", translate(get().lang, "connection.failed"), problemTextSafe(get().lang, problem));
      await get().refresh();
    }
  },

  retry: async () => {
    set({ busy: "retry" });
    try {
      const snapshot = await api.retry();
      set({ snapshot });
    } catch (error) {
      const problem = toProblem(error) as Problem;
      await get().refresh();
      get().toast("danger", translate(get().lang, "connection.failed"), problemTextSafe(get().lang, problem));
    } finally {
      set({ busy: null });
    }
  },

  importNodes: async (text) => {
    try {
      const { report, subscriptionIds } = await api.importText(text);
      await get().refresh();
      const summary = {
        added: report.added.length,
        duplicates: report.duplicates,
        failures: report.failures.length,
        subscriptions: subscriptionIds.length,
      };
      get().toast(
        summary.failures > 0 ? "info" : "ok",
        translate(get().lang, "nodes.import.result", {
          added: summary.added,
          dup: summary.duplicates,
          failed: summary.failures,
        }),
        report.failures[0] ? `${report.failures[0].source} · ${problemTextSafe(get().lang, report.failures[0].error)}` : undefined,
      );
      return summary;
    } catch (error) {
      const problem = toProblem(error) as Problem;
      get().toast("danger", translate(get().lang, "nodes.add"), problemTextSafe(get().lang, problem));
      return { added: 0, duplicates: 0, failures: 1, subscriptions: 0 };
    }
  },

  removeNode: async (id) => {
    try {
      await api.removeNode(id);
      await get().refresh();
    } catch (error) {
      const problem = toProblem(error) as Problem;
      get().toast("danger", translate(get().lang, "common.delete"), problemTextSafe(get().lang, problem));
    }
  },

  toggleFavorite: async (id, favorite) => {
    try {
      await api.setFavorite(id, favorite);
      await get().refresh();
    } catch (error) {
      const problem = toProblem(error) as Problem;
      get().toast("danger", translate(get().lang, "nodes.favorite"), problemTextSafe(get().lang, problem));
    }
  },

  saveTags: async (id, tags) => {
    try {
      await api.setTags(id, tags);
      await get().refresh();
    } catch (error) {
      const problem = toProblem(error) as Problem;
      get().toast("danger", translate(get().lang, "nodes.tags"), problemTextSafe(get().lang, problem));
    }
  },

  copyLink: async (id) => {
    try {
      const link = await api.shareLink(id);
      await navigator.clipboard.writeText(link);
      get().toast("ok", translate(get().lang, "common.copied"));
    } catch (error) {
      const problem = toProblem(error) as Problem;
      get().toast("danger", translate(get().lang, "common.copy"), problemTextSafe(get().lang, problem));
    }
  },

  testLatency: async (ids) => {
    const nodes = get().snapshot?.nodes ?? [];
    const total = ids?.length ?? nodes.length;
    set({ testing: true, testProgress: { done: 0, total } });
    try {
      const results = await api.testLatency(ids ?? []);
      set({ lastResults: results });
      await get().refresh();
      const failed = results.filter((result) => result.error).length;
      get().toast(
        failed > 0 ? "info" : "ok",
        translate(get().lang, "speed.running"),
        `${results.length - failed} OK${failed ? ` · ${failed} ${translate(get().lang, "speed.failed")}` : ""}`,
      );
    } catch (error) {
      const problem = toProblem(error) as Problem;
      get().toast("danger", translate(get().lang, "speed.run"), problemTextSafe(get().lang, problem));
    } finally {
      set({ testing: false, testProgress: { done: total, total } });
    }
  },

  addSubscription: async (url, name) => {
    set({ busy: "subscription-add" });
    try {
      const result = await api.addSubscription(url, name);
      await get().refresh();
      get().toast(
        "ok",
        translate(get().lang, "subs.add"),
        translate(get().lang, "nodes.import.result", {
          added: result.added,
          dup: 0,
          failed: 0,
        }),
      );
    } catch (error) {
      const problem = toProblem(error) as Problem;
      get().toast("danger", translate(get().lang, "subs.add"), problemTextSafe(get().lang, problem));
    } finally {
      set({ busy: null });
    }
  },

  refreshSubscription: async (id) => {
    set({ busy: `subscription-${id}` });
    try {
      const result = await api.refreshSubscription(id);
      await get().refresh();
      const error = result.error as Problem | undefined;
      if (error) {
        get().toast("danger", translate(get().lang, "subs.update"), problemTextSafe(get().lang, error));
      } else {
        get().toast(
          "ok",
          translate(get().lang, "subs.update"),
          translate(get().lang, "nodes.import.result", {
            added: result.added,
            dup: 0,
            failed: 0,
          }),
        );
      }
    } catch (error) {
      const problem = toProblem(error) as Problem;
      get().toast("danger", translate(get().lang, "subs.update"), problemTextSafe(get().lang, problem));
    } finally {
      set({ busy: null });
    }
  },

  removeSubscription: async (id) => {
    try {
      await api.removeSubscription(id);
      await get().refresh();
      get().toast("info", translate(get().lang, "common.delete"));
    } catch (error) {
      const problem = toProblem(error) as Problem;
      get().toast("danger", translate(get().lang, "common.delete"), problemTextSafe(get().lang, problem));
    }
  },

  setSubscriptionEnabled: async (id, enabled) => {
    try {
      await api.setSubscriptionEnabled(id, enabled);
      await get().refresh();
    } catch (error) {
      const problem = toProblem(error) as Problem;
      get().toast("danger", translate(get().lang, "subs.enabled"), problemTextSafe(get().lang, problem));
    }
  },

  saveSettings: async (patches) => {
    set({ busy: "settings" });
    try {
      const settings = await api.saveSettings(patches);
      await get().refresh();
      set({ lang: resolveLang(settings.appearance.language) });
      applyDocumentLang(get().lang);
      if (settings.appearance.theme !== "system") {
        set({ themeMode: settings.appearance.theme });
        applyDocumentTheme(settings.appearance.theme);
        writeStorage("v2rayr.theme", settings.appearance.theme);
      }
      get().toast("ok", translate(get().lang, "common.saved"));
      return true;
    } catch (error) {
      const problem = toProblem(error) as Problem;
      get().toast("danger", translate(get().lang, "common.saveFailed"), problemTextSafe(get().lang, problem));
      return false;
    } finally {
      set({ busy: null });
    }
  },

  loadRules: async () => {
    try {
      set({ rules: await api.rules() });
    } catch {
      // Rules are secondary to the snapshot; a failure surfaces on the next edit.
    }
  },

  addRule: async (kind, value, action, applications) => {
    try {
      set({ rules: await api.addRule(kind, value, action, applications) });
      return true;
    } catch (error) {
      const problem = toProblem(error) as Problem;
      get().toast("danger", translate(get().lang, "settings.rules.add"), problemTextSafe(get().lang, problem));
      return false;
    }
  },

  updateRule: async (id, kind, value, action, applications) => {
    try {
      set({ rules: await api.updateRule(id, kind, value, action, applications) });
      return true;
    } catch (error) {
      get().toast("danger", translate(get().lang, "common.saveFailed"), problemTextSafe(get().lang, toProblem(error)));
      return false;
    }
  },

  toggleRule: async (id, enabled) => {
    try {
      set({ rules: await api.setRuleEnabled(id, enabled) });
    } catch (error) {
      const problem = toProblem(error) as Problem;
      get().toast("danger", translate(get().lang, "settings.rules.title"), problemTextSafe(get().lang, problem));
    }
  },

  removeRule: async (id) => {
    try {
      set({ rules: await api.removeRule(id) });
    } catch (error) {
      const problem = toProblem(error) as Problem;
      get().toast("danger", translate(get().lang, "settings.rules.title"), problemTextSafe(get().lang, problem));
    }
  },

  probeCores: async () => {
    try {
      const cores = await api.cores();
      const snapshot = get().snapshot;
      if (snapshot) set({ snapshot: { ...snapshot, cores } });
    } catch (error) {
      const problem = toProblem(error) as Problem;
      get().toast("danger", translate(get().lang, "settings.cores.probe"), problemTextSafe(get().lang, problem));
    }
  },

  installCore: async (kind) => {
    if (get().busy) return false;
    set({ busy: "install-core" });
    try {
      const status = await api.installCore(kind);
      const snapshot = get().snapshot;
      if (snapshot) {
        set({ snapshot: { ...snapshot, cores: snapshot.cores.map((core) => (core.kind === status.kind ? status : core)) } });
      }
      if (status.lastError) {
        get().toast("danger", translate(get().lang, "settings.cores.install"), problemTextSafe(get().lang, status.lastError));
      }
      return status.installed && !status.lastError;
    } catch (error) {
      const problem = toProblem(error) as Problem;
      get().toast("danger", translate(get().lang, "settings.cores.install"), problemTextSafe(get().lang, problem));
      return false;
    } finally {
      set({ busy: null });
    }
  },

  resetSettings: async () => {
    try {
      const settings = await api.resetSettings();
      await get().refresh();
      get().toast("info", translate(get().lang, "settings.reset"), `${settings.revision}`);
    } catch (error) {
      const problem = toProblem(error) as Problem;
      get().toast("danger", translate(get().lang, "settings.reset"), problemTextSafe(get().lang, problem));
    }
  },

  setNodeQuery: (nodeQuery) => set({ nodeQuery }),
  setNodeScope: (nodeScope) => set({ nodeScope }),
  setNodeProtocol: (nodeProtocol) => set({ nodeProtocol }),
}));

export function useT(): (key: string, vars?: Record<string, string | number>) => string {
  const lang = useStore((state) => state.lang);
  return (key, vars) => translate(lang, key, vars);
}

export function useLang(): Lang {
  return useStore((state) => state.lang);
}

export function pageFromHash(): Page {
  const hash = typeof window === "undefined" ? "" : window.location.hash.replace(/^#\/?/, "");
  const pages: Page[] = ["connection", "nodes", "subscriptions", "speedtest", "settings"];
  return (pages.find((page) => page === hash) ?? "connection") as Page;
}

export function applyDocumentTheme(theme: "light" | "dark"): void {
  document.documentElement.dataset.theme = theme;
  document.documentElement.style.colorScheme = theme;
}

export function applyDocumentLang(lang: Lang): void {
  document.documentElement.lang = lang;
}

export function problemTextSafe(lang: Lang, problem: Problem | undefined): string {
  if (!problem) return "";
  const translated = translate(lang, `error.${problem.code}`);
  const label = translated === `error.${problem.code}` ? problem.code : translated;
  return problem.detail ? `${label}：${problem.detail}` : label;
}

function policyLabel(lang: Lang, policy: RoutingPolicy): string {
  return translate(lang, `connection.policy.${policy}`);
}

export { isPreview };
