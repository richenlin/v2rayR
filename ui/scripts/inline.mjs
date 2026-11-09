// Inline the built entry bundle into index.html.
//
// The packaged app loads its UI from Tauri's `tauri://` protocol, where the
// WebView refuses to pull external module scripts and stylesheets. A single
// self-contained document sidesteps the whole subresource path: no CORS, no
// MIME sniffing, no preload graph.
import { readFileSync, writeFileSync, rmSync, existsSync } from "node:fs";
import { dirname, join, resolve } from "node:path";
import { fileURLToPath } from "node:url";

const dist = resolve(dirname(fileURLToPath(import.meta.url)), "..", "dist");
const htmlPath = join(dist, "index.html");
let html = readFileSync(htmlPath, "utf8");

const stylesheet = /<link[^>]+rel="stylesheet"[^>]*href="\/?(assets\/[^"]+)"[^>]*>/g;
const script = /<script[^>]+src="\/?(assets\/[^"]+)"[^>]*><\/script>/g;

html = html.replace(stylesheet, (_match, href) => {
  const css = readFileSync(join(dist, href), "utf8");
  return `<style>\n${css}\n</style>`;
});

html = html.replace(script, (_match, src) => {
  const code = readFileSync(join(dist, src), "utf8");
  return `<script type="module">\n${code}\n</script>`;
});

writeFileSync(htmlPath, html);
if (existsSync(join(dist, "assets"))) rmSync(join(dist, "assets"), { recursive: true, force: true });
console.log(`inlined ${html.length} bytes into dist/index.html`);
