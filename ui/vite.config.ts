import { readFileSync } from "node:fs";
import { defineConfig } from "vite";
import react from "@vitejs/plugin-react";
import tailwindcss from "@tailwindcss/vite";

function workspaceVersion(): string {
  const text = readFileSync(new URL("../Cargo.toml", import.meta.url), "utf8");
  const section = text.split("[workspace.package]")[1]?.split("\n") ?? [];
  for (const line of section) {
    const trimmed = line.trim();
    if (trimmed.startsWith("[")) break;
    const match = trimmed.match(/^version\s*=\s*"([^"]+)"$/);
    if (match) return match[1];
  }
  throw new Error("Cargo.toml 没有 [workspace.package] version");
}

// Tauri expects a fixed port and no clearing of its terminal output.
// WKWebView refuses `crossorigin` fetches over Tauri's `tauri://` scheme, which
// leaves the packaged app on a blank page. Vite adds the attribute to every
// script and stylesheet; strip it so the bundled UI loads from the custom
// protocol as a same-origin document.
const stripCrossOrigin = {
  name: "v2rayr-strip-crossorigin",
  transformIndexHtml: (html: string) => html.replace(/\s+crossorigin(?:="[^"]*")?/g, ""),
};

export default defineConfig({
  define: {
    "import.meta.env.VITE_APP_VERSION": JSON.stringify(workspaceVersion()),
  },
  plugins: [react(), tailwindcss(), stripCrossOrigin],
  clearScreen: false,
  server: {
    port: 1420,
    strictPort: true,
    watch: { ignored: ["**/src-tauri/**"] },
  },
  build: {
    target: "safari15",
    sourcemap: false,
  },
});
