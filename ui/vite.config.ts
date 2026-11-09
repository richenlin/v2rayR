import { defineConfig } from "vite";
import react from "@vitejs/plugin-react";
import tailwindcss from "@tailwindcss/vite";

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
