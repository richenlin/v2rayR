// Build Tauri's static update manifest from the signed payloads, without publishing.
import { readFile, readdir, writeFile } from "node:fs/promises";
import { basename, join } from "node:path";

const [artifacts, base, output] = process.argv.slice(2);
if (!artifacts || !base || !output) throw Error("Usage: update-manifest.mjs ARTIFACT_DIR HTTPS_ASSET_BASE OUTPUT");
const url = new URL(base.endsWith("/") ? base : base + "/");
if (url.protocol !== "https:" || url.username || url.password || url.search || url.hash) {
  throw Error("Use a credential-free HTTPS asset base");
}
const { version } = JSON.parse(await readFile("apps/desktop/tauri.conf.json", "utf8"));
async function walk(path) {
  const entries = await readdir(path, { withFileTypes: true });
  return (await Promise.all(entries.map(entry => entry.isDirectory()
    ? walk(join(path, entry.name)) : [join(path, entry.name)]))).flat();
}
const files = await walk(artifacts);
const platforms = {};
for (const [platform, suffix] of [
  ["darwin-aarch64", ".app.tar.gz"],
  ["linux-x86_64", ".AppImage"],
  ["windows-x86_64", ".msi"],
  ["windows-x86_64-nsis", "-setup.exe"],
]) {
  const matches = files.filter(file => file.endsWith(suffix + ".sig"));
  if (matches.length !== 1) throw Error(`Expected exactly one signed ${platform} payload`);
  const sig = matches[0];
  const payload = sig.slice(0, -4);
  if (!files.includes(payload)) throw Error(`Missing payload for ${sig}`);
  const signature = (await readFile(sig, "utf8")).trim();
  if (!signature || !/^[A-Za-z0-9+/=]+$/.test(signature)) throw Error(`Invalid updater signature for ${platform}`);
  const comment = Buffer.from(signature, "base64").toString("utf8").split("\n")
    .find(line => line.startsWith("trusted comment: "))?.slice("trusted comment: ".length);
  if (!comment?.split("\t").includes(`version:${version}`)) {
    throw Error(`Signature for ${platform} is not bound to ${version}`);
  }
  platforms[platform] = { signature, url: new URL(encodeURIComponent(basename(payload)), url).href };
}
await writeFile(output, JSON.stringify({ version, pub_date: new Date().toISOString(), platforms }, null, 2) + "\n");
