// Deterministic regression checks for races that native manual testing rarely hits.
import assert from "node:assert/strict";
import { readFileSync } from "node:fs";
import { createRequire } from "node:module";
import vm from "node:vm";
import ts from "typescript";
const require = createRequire(import.meta.url);
const saved = new Map();
const storage = { readStorage: (key) => saved.get(key) ?? null, writeStorage: (key, value) => saved.set(key, value) };
const document = { documentElement: { dataset: {}, style: {} }, addEventListener() {}, removeEventListener() {} };
const window = { location: { hash: "" }, matchMedia: () => ({ matches: false }), setTimeout, clearTimeout };
function load(file, mocks) {
  const exports = {};
  const code = ts.transpileModule(readFileSync(new URL(file, import.meta.url), "utf8"), {
    compilerOptions: { module: ts.ModuleKind.CommonJS, jsx: ts.JsxEmit.ReactJSX, target: ts.ScriptTarget.ES2022 },
  }).outputText;
  vm.runInNewContext(code, { exports, require: (id) => id in mocks ? mocks[id] : require(id), window, document, console });
  return exports;
}
let resolveSnapshot;
const api = { snapshot: () => new Promise((resolve) => { resolveSnapshot = resolve; }), rules: async () => [] };
function loadStore() {
  return load("../src/store.ts", {
    "./lib/api": { api, isPreview: false },
    "./lib/storage": storage,
    "./lib/i18n": { resolveLang: () => "en", translate: (_lang, key) => key },
  }).useStore;
}
const store = loadStore();
const snapshot = (total) => ({ settings: { appearance: { language: "en", theme: "dark" } }, connection: { phase: "connected" }, traffic: { totalDownBytes: total } });
store.getState().adoptSnapshot(snapshot(10));
const refresh = store.getState().refresh();
store.getState().applyTraffic({ totalDownBytes: 20 });
resolveSnapshot({ ...snapshot(10), nodes: ["refreshed"] });
await refresh;
assert.equal(store.getState().snapshot.nodes[0], "refreshed", "traffic ticks cannot starve node refreshes");
assert.equal(store.getState().traffic.totalDownBytes, 20, "late snapshot cannot undo traffic event");
const boot = store.getState().boot();
store.getState().adoptSnapshot(snapshot(30));
store.getState().applyTraffic({ totalDownBytes: 40 });
resolveSnapshot(snapshot(10));
await boot;
assert.equal(store.getState().snapshot.traffic.totalDownBytes, 30);
assert.equal(store.getState().traffic.totalDownBytes, 40, "boot cannot undo pushed state");
store.getState().navigate("nodes");
store.getState().setNodeQuery("example");
store.getState().setNodeScope("favorites");
store.getState().setNodeProtocol("vless");
window.location.hash = ""; // New webview has a fresh URL and JavaScript store.
const reopened = loadStore();
assert.equal(reopened.getState().nodeQuery, "example");
assert.equal(reopened.getState().nodeScope, "favorites");
assert.equal(reopened.getState().nodeProtocol, "vless");
assert.equal(load("../src/store.ts", {
  "./lib/api": { api, isPreview: false }, "./lib/storage": storage,
  "./lib/i18n": { resolveLang: () => "en", translate: (_lang, key) => key },
}).pageFromHash(), "nodes");

let effects = [];
let listeners = [];
let stops = 0;
let adoptions = 0;
let intervals = 0;
let visibilityListeners = 0;
let refreshes = 0;
const appState = { boot() {}, refresh() { refreshes += 1; }, navigate() {}, loading: true,
  adoptSnapshot() { adoptions += 1; }, applyTraffic() { adoptions += 1; }, toast() {} };
const useStore = (select) => select(appState);
useStore.getState = () => appState;
window.addEventListener = () => {};
window.removeEventListener = () => {};
window.setInterval = () => { intervals += 1; return 1; };
window.clearInterval = () => { intervals -= 1; };
document.addEventListener = () => { visibilityListeners += 1; };
document.removeEventListener = () => { visibilityListeners -= 1; };
const mocks = {
  react: { useEffect: (effect) => effects.push(effect) },
  "./lib/api": { isPreview: false },
  "./store": { useStore, useT: () => (key) => key, pageFromHash: () => "nodes" },
  "@tauri-apps/api/event": { listen: (name, callback) => new Promise((resolve) => listeners.push({ name, callback, resolve })) },
};
for (const [name, file] of [["Rail", "components/Rail"], ["TopBar", "components/TopBar"], ["Toasts", "components/Toasts"],
  ...["Connection", "Nodes", "Settings", "Speedtest", "Subscriptions"].map((name) => [`${name}Page`, `pages/${name}`])]) {
  mocks[`./${file}`] = { [name]: () => null };
}
const App = load("../src/App.tsx", mocks).default;
for (let cycle = 0; cycle < 3; cycle += 1) {
  effects = []; listeners = [];
  App();
  const cleanups = effects.map((effect) => effect()).filter(Boolean);
  assert.equal(intervals, 1);
  assert.equal(visibilityListeners, 1);
  // Cleanup before asynchronous registration finishes: React StrictMode's race.
  cleanups.forEach((cleanup) => cleanup());
  for (const entry of listeners) { entry.callback({ payload: {} }); entry.resolve(() => { stops += 1; }); }
  await new Promise((resolve) => setImmediate(resolve));
  assert.equal(intervals, 0);
  assert.equal(visibilityListeners, 0);
}
assert.equal(stops, 9, "all three listeners are removed on every unmount");
assert.equal(adoptions, 0, "callbacks cannot mutate an unmounted app");
assert.equal(refreshes, 0, "late registration cannot refresh an unmounted app");
console.log("PASS: boot/refresh races, reopened page and filters, repeated async listener cleanup");
effects = []; listeners = [];
App();
const cleanupMounted = effects.map((effect) => effect()).filter(Boolean);
listeners.forEach((entry) => entry.resolve(() => { stops += 1; }));
await new Promise((resolve) => setImmediate(resolve));
assert.equal(refreshes, 1, "refresh once both state listeners are ready");
listeners.filter((entry) => entry.name !== "app-update-available").forEach((entry) => entry.callback({ payload: {} }));
assert.equal(adoptions, 2);
cleanupMounted.forEach((cleanup) => cleanup());
assert.equal(stops, 12);
assert.equal(intervals, 0);
let hookValue;
const useStoredState = load("../src/lib/useStoredState.ts", {
  "./storage": storage,
  react: { useState: (initial) => {
    hookValue = typeof initial === "function" ? initial() : initial;
    return [hookValue, (next) => { hookValue = typeof next === "function" ? next(hookValue) : next; }];
  } },
}).useStoredState;
const [, setDraft] = useStoredState("draft-test", {});
setDraft((previous) => ({ ...previous, inbound: { port: 12345 } }));
const [restoredDraft] = useStoredState("draft-test", {});
assert.equal(restoredDraft.inbound.port, 12345);
console.log("PASS: mounted listener delivery and cleanup, settings draft restoration");
