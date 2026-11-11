import { useState, type Dispatch, type SetStateAction } from "react";
import { readStorage, writeStorage } from "./storage";

/** Keep local editor state when the tray releases the webview. */
export function useStoredState<T>(key: string, initial: T): [T, Dispatch<SetStateAction<T>>] {
  const [value, setValue] = useState<T>(() => {
    try {
      const saved = readStorage(key);
      return saved === null ? initial : JSON.parse(saved) as T;
    } catch {
      return initial;
    }
  });
  const update: Dispatch<SetStateAction<T>> = (next) => {
    setValue((previous) => {
      const result = typeof next === "function" ? (next as (value: T) => T)(previous) : next;
      writeStorage(key, JSON.stringify(result));
      return result;
    });
  };
  return [value, update];
}
