import React from "react";
import { getConfig, setConfig } from "../config";

// React state that mirrors a key in the tauri-plugin-store backing
// file. Behavior:
// * mount → returns `initial` immediately; kicks off an async load
// * load resolves → if a value was persisted, swaps in (without
//   re-persisting it)
// * any later setValue → writes through to the store
//
// `validate` lets callers reject corrupt persisted data (e.g. an
// out-of-range enum) and fall back to `initial`. Returns the same
// `[value, setValue]` tuple as `React.useState` so callers can
// drop-in replace.
export function usePersistentState<T>(
  key: string,
  initial: T,
  validate?: (raw: unknown) => raw is T
): [T, React.Dispatch<React.SetStateAction<T>>] {
  const [value, setValue] = React.useState<T>(initial);
  const loaded = React.useRef(false);
  const skipNextPersist = React.useRef(false);

  React.useEffect(() => {
    let cancelled = false;
    getConfig<unknown>(key)
      .then((stored) => {
        if (cancelled) return;
        if (stored == null) return;
        if (validate && !validate(stored)) return;
        skipNextPersist.current = true;
        setValue(stored as T);
      })
      .catch(() => {})
      .finally(() => {
        if (!cancelled) loaded.current = true;
      });
    return () => {
      cancelled = true;
    };
  }, [key]);

  React.useEffect(() => {
    if (!loaded.current) return;
    if (skipNextPersist.current) {
      skipNextPersist.current = false;
      return;
    }
    void setConfig(key, value);
  }, [key, value]);

  return [value, setValue];
}
