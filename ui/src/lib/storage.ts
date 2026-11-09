/**
 * Storage that never throws.
 *
 * The packaged app runs on Tauri's `tauri://` scheme, where WebKit can refuse
 * `localStorage` (SecurityError) — touching it unguarded took the whole boot
 * down with a blank window. Everything here degrades to an in-memory value.
 */
const memory = new Map<string, string>();

function backing(): Storage | null {
  try {
    const storage = globalThis.localStorage;
    if (!storage) return null;
    // Probe: a disabled store throws on write, not on access.
    const probe = "__v2rayr_probe__";
    storage.setItem(probe, "1");
    storage.removeItem(probe);
    return storage;
  } catch {
    return null;
  }
}

const store = backing();

export function readStorage(key: string): string | null {
  if (store) {
    try {
      return store.getItem(key);
    } catch {
      // fall through to memory
    }
  }
  return memory.get(key) ?? null;
}

export function writeStorage(key: string, value: string): void {
  memory.set(key, value);
  if (!store) return;
  try {
    store.setItem(key, value);
  } catch {
    // Quota or private mode: the value stays in memory for this session.
  }
}

export function removeStorage(key: string): void {
  memory.delete(key);
  if (!store) return;
  try {
    store.removeItem(key);
  } catch {
    // ignore
  }
}
