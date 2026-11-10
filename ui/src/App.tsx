import { listen } from "@tauri-apps/api/event";
import { isPreview } from "./lib/api";
import { useEffect } from "react";
import { Rail } from "./components/Rail";
import { TopBar } from "./components/TopBar";
import { Toasts } from "./components/Toasts";
import { ConnectionPage } from "./pages/Connection";
import { NodesPage } from "./pages/Nodes";
import { SettingsPage } from "./pages/Settings";
import { SpeedtestPage } from "./pages/Speedtest";
import { SubscriptionsPage } from "./pages/Subscriptions";
import { pageFromHash, useStore, useT } from "./store";

export default function App() {
  const t = useT();
  const page = useStore((state) => state.page);
  const navigate = useStore((state) => state.navigate);
  const boot = useStore((state) => state.boot);
  const refresh = useStore((state) => state.refresh);
  const loading = useStore((state) => state.loading);
  const bootError = useStore((state) => state.bootError);
  const appearance = useStore((state) => state.snapshot?.settings.appearance);
  const phase = useStore((state) => state.snapshot?.connection.phase);

  useEffect(() => {
    void boot();
  }, [boot]);

  useEffect(() => {
    if (isPreview) return;
    let disposed = false;
    let unlisten: (() => void) | undefined;
    void listen<string>("app-update-available", ({ payload }) => {
      useStore.getState().toast("info", t("maintenance.updateAvailable"), payload);
    }).then((stop) => { if (disposed) stop(); else unlisten = stop; }).catch(() => {});
    return () => { disposed = true; unlisten?.(); };
  }, [t]);

  // Keep the URL and the visible page in sync, so reloads land where the user was.
  useEffect(() => {
    const onHash = () => {
      const next = pageFromHash();
      if (next !== useStore.getState().page) navigate(next);
    };
    window.addEventListener("hashchange", onHash);
    return () => window.removeEventListener("hashchange", onHash);
  }, [navigate]);

  // One poll loop for every page: 1 s while a session is active, 5 s otherwise.
  useEffect(() => {
    let cancelled = false;
    let timer: number | undefined;
    const tick = async () => {
      if (cancelled) return;
      await refresh();
      const state = useStore.getState().snapshot?.connection.phase;
      const delay = state === "connected" || state === "connecting" ? 1000 : 5000;
      timer = window.setTimeout(tick, delay);
    };
    timer = window.setTimeout(tick, 1200);
    return () => {
      cancelled = true;
      if (timer) window.clearTimeout(timer);
    };
  }, [refresh]);

  useEffect(() => {
    const root = document.documentElement;
    root.dataset.compact = appearance?.compact ? "true" : "false";
    root.dataset.reduceMotion = appearance?.reduceMotion ? "true" : "false";
  }, [appearance?.compact, appearance?.reduceMotion]);

  useEffect(() => {
    const titleKey = { connection: "连接", nodes: "节点", subscriptions: "订阅", speedtest: "测速", settings: "设置" } as const;
    document.title = `v2rayR · ${titleKey[page]}`;
  }, [page]);

  if (loading) {
    return (
      <div className="shell" aria-busy="true">
        <Rail />
        <div className="main">
          <TopBar />
          <div className="page">
            <div className="eyebrow">NETWORK CONTROL</div>
            <div style={{ height: 36, width: 220, background: "var(--line)", borderRadius: 8, opacity: 0.5 }} />
            <p className="page-sub">正在读取本地状态…</p>
          </div>
        </div>
      </div>
    );
  }

  if (bootError) {
    return (
      <div className="shell">
        <Rail />
        <div className="main">
          <TopBar />
          <div className="page">
            <h1 className="page-title">无法启动</h1>
            <p className="page-sub">读取本地数据失败：{bootError}</p>
            <button type="button" className="btn btn-solid" style={{ marginTop: 16 }} onClick={() => window.location.reload()}>
              重试
            </button>
          </div>
        </div>
      </div>
    );
  }

  return (
    <div className="shell">
      <Rail />
      <div className="main">
        <TopBar />
        <main style={{ display: "flex", flexDirection: "column", minHeight: 0, flex: 1 }}>
          {page === "connection" ? <ConnectionPage /> : null}
          {page === "nodes" ? <NodesPage /> : null}
          {page === "subscriptions" ? <SubscriptionsPage /> : null}
          {page === "speedtest" ? <SpeedtestPage /> : null}
          {page === "settings" ? <SettingsPage /> : null}
        </main>
        <Toasts />
        <span className="sr-only" data-phase={phase ?? "disconnected"} />
      </div>
    </div>
  );
}
