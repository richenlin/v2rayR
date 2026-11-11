import { BrandMark, Icon, type IconName } from "./Icon";
import { useStore, useT, type Page } from "../store";

const items: Array<{ page: Page; labelKey: string; icon: IconName }> = [
  { page: "connection", labelKey: "nav.connection", icon: "plug" },
  { page: "nodes", labelKey: "nav.nodes", icon: "nodes" },
  { page: "subscriptions", labelKey: "nav.subscriptions", icon: "rss" },
  { page: "speedtest", labelKey: "nav.speedtest", icon: "gauge" },
  { page: "settings", labelKey: "nav.settings", icon: "sliders" },
];

export function Rail() {
  const t = useT();
  const page = useStore((state) => state.page);
  const navigate = useStore((state) => state.navigate);
  const themeMode = useStore((state) => state.themeMode);
  const toggleTheme = useStore((state) => state.toggleTheme);
  const version = useStore((state) => state.snapshot?.version.appVersion ?? import.meta.env.VITE_APP_VERSION);

  return (
    <aside className="rail" aria-label={t("nav.workspace")}>
      <div className="brand">
        <span className="brand-mark">
          <BrandMark />
        </span>
        <span className="brand-name">v2rayR</span>
      </div>

      <div className="rail-label">{t("nav.workspace")}</div>
      <nav style={{ display: "flex", flexDirection: "column", gap: 2 }}>
        {items.map((item) => (
          <button
            key={item.page}
            type="button"
            className="nav-item"
            aria-current={page === item.page ? "page" : undefined}
            onClick={() => navigate(item.page)}
          >
            <Icon name={item.icon} size={17} />
            <span>{t(item.labelKey)}</span>
          </button>
        ))}
      </nav>

      <div className="rail-foot">
        <button type="button" className="btn btn-outline btn-sm" onClick={toggleTheme}>
          <Icon name={themeMode === "dark" ? "sun" : "moon"} size={14} />
          {themeMode === "dark" ? t("nav.theme.light") : t("nav.theme.dark")}
        </button>
        <div className="rail-note">
          {t("nav.build")} {version} · M4
        </div>
      </div>
    </aside>
  );
}
