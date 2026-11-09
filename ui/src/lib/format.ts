/** Formatting helpers shared by the pages. All numbers are tabular in the UI. */

export function rate(bytesPerSecond: number): { value: string; unit: string } {
  if (!Number.isFinite(bytesPerSecond) || bytesPerSecond <= 0) return { value: "0", unit: "B/s" };
  const units = ["B/s", "KB/s", "MB/s", "GB/s"];
  let value = bytesPerSecond;
  let index = 0;
  while (value >= 1024 && index < units.length - 1) {
    value /= 1024;
    index += 1;
  }
  return { value: value >= 100 ? value.toFixed(0) : value.toFixed(value >= 10 ? 1 : 2), unit: units[index] };
}

export function bytes(total: number): string {
  if (!Number.isFinite(total) || total <= 0) return "0 B";
  const units = ["B", "KB", "MB", "GB", "TB"];
  let value = total;
  let index = 0;
  while (value >= 1024 && index < units.length - 1) {
    value /= 1024;
    index += 1;
  }
  return `${value.toFixed(value >= 10 || index === 0 ? 0 : 1)} ${units[index]}`;
}

export function latencyTone(ms: number | undefined): "ok" | "mid" | "bad" | "none" {
  if (ms === undefined) return "none";
  if (ms < 120) return "ok";
  if (ms < 260) return "mid";
  return "bad";
}

export function relativeTime(iso: string | undefined, locale: string): string {
  if (!iso) return "—";
  const then = new Date(iso).getTime();
  if (Number.isNaN(then)) return "—";
  const diff = Date.now() - then;
  const minutes = Math.round(diff / 60_000);
  const formatter = new Intl.RelativeTimeFormat(locale, { numeric: "auto" });
  if (Math.abs(minutes) < 60) return formatter.format(-minutes, "minute");
  const hours = Math.round(minutes / 60);
  if (Math.abs(hours) < 48) return formatter.format(-hours, "hour");
  return formatter.format(-Math.round(hours / 24), "day");
}

export function shortDate(iso: string | undefined, locale: string): string {
  if (!iso) return "—";
  const date = new Date(iso);
  if (Number.isNaN(date.getTime())) return "—";
  return new Intl.DateTimeFormat(locale, { month: "short", day: "numeric", hour: "2-digit", minute: "2-digit" }).format(date);
}

export function clock(iso: string | undefined, locale: string): string {
  if (!iso) return "—";
  const date = new Date(iso);
  if (Number.isNaN(date.getTime())) return "—";
  return new Intl.DateTimeFormat(locale, { hour: "2-digit", minute: "2-digit" }).format(date);
}

export function protocolLabel(protocol: string): string {
  const map: Record<string, string> = {
    vless: "VLESS",
    vmess: "VMess",
    trojan: "Trojan",
    shadowsocks: "SS",
    hysteria2: "Hysteria2",
    tuic: "TUIC",
    anytls: "AnyTLS",
    naive: "Naive",
    wireguard: "WireGuard",
    socks: "SOCKS",
    http: "HTTP",
    chain: "Chain",
    custom: "Custom",
  };
  return map[protocol] ?? protocol.toUpperCase();
}

export function transportLabel(transport: string): string {
  const map: Record<string, string> = {
    raw: "TCP",
    ws: "WS",
    grpc: "gRPC",
    httpupgrade: "HTTPUpgrade",
    http: "HTTP",
    xhttp: "XHTTP",
    mkcp: "mKCP",
    quic: "QUIC",
  };
  return map[transport] ?? transport.toUpperCase();
}
