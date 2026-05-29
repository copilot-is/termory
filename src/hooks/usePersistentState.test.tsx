import React from "react";
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import { act, render } from "@testing-library/react";
import { usePersistentState } from "./usePersistentState";

// `config.ts` proxies into Tauri's invoke. In jsdom there's no Tauri,
// so we mock the module before importing the hook. Vitest hoists vi.mock
// to the top so this works even with later test-side overrides.
const getConfigMock = vi.fn();
const setConfigMock = vi.fn();
vi.mock("../config", () => ({
  getConfig: (key: string) => getConfigMock(key),
  setConfig: (key: string, value: unknown) => setConfigMock(key, value)
}));

// Tiny harness — exposes the hook's current value + setter so tests
// can assert and invoke from outside React.
function Harness<T>({
  hookKey,
  initial,
  validate,
  emit
}: {
  hookKey: string;
  initial: T;
  validate?: (raw: unknown) => raw is T;
  emit: (state: T, setter: React.Dispatch<React.SetStateAction<T>>) => void;
}) {
  const [value, setValue] = usePersistentState(hookKey, initial, validate);
  emit(value, setValue);
  return null;
}

beforeEach(() => {
  getConfigMock.mockReset();
  setConfigMock.mockReset();
});

afterEach(() => {
  vi.useRealTimers();
});

describe("usePersistentState", () => {
  it("returns `initial` synchronously on mount", () => {
    getConfigMock.mockResolvedValue(undefined);
    let latest: number | null = null;
    render(<Harness hookKey="foo" initial={42} emit={(v) => (latest = v)} />);
    expect(latest).toBe(42);
  });

  it("swaps to the persisted value on async load WITHOUT writing it back", async () => {
    getConfigMock.mockResolvedValue("stored");
    let latest: string | null = null;
    render(
      <Harness
        hookKey="theme"
        initial="initial"
        emit={(v) => (latest = v)}
      />
    );
    // First micro-tick: getConfig resolves → setValue("stored") via skipNextPersist
    await act(async () => {
      await Promise.resolve();
    });
    expect(latest).toBe("stored");
    // The persist effect should have been SKIPPED for the load swap,
    // so setConfig was not called as a side effect of loading.
    expect(setConfigMock).not.toHaveBeenCalled();
  });

  it("persists subsequent user-driven setValue calls", async () => {
    getConfigMock.mockResolvedValue(undefined);
    let setter: React.Dispatch<React.SetStateAction<number>> | null = null;
    render(
      <Harness
        hookKey="count"
        initial={0}
        emit={(_v, s) => (setter = s)}
      />
    );
    // Wait for the load effect to flip loaded.current = true.
    await act(async () => {
      await Promise.resolve();
    });
    await act(async () => {
      setter!(5);
    });
    expect(setConfigMock).toHaveBeenCalledWith("count", 5);
  });

  it("ignores persisted value when validate rejects it", async () => {
    getConfigMock.mockResolvedValue("corrupt");
    const isNumber = (raw: unknown): raw is number => typeof raw === "number";
    let latest: number | null = null;
    render(
      <Harness
        hookKey="x"
        initial={1}
        validate={isNumber}
        emit={(v) => (latest = v)}
      />
    );
    await act(async () => {
      await Promise.resolve();
    });
    expect(latest).toBe(1);
    expect(setConfigMock).not.toHaveBeenCalled();
  });

  it("does NOT persist anything before the initial load resolves", async () => {
    // Keep getConfig pending so loaded.current stays false.
    let resolveLoad: (v: unknown) => void = () => {};
    getConfigMock.mockReturnValue(
      new Promise((resolve) => {
        resolveLoad = resolve;
      })
    );
    let setter: React.Dispatch<React.SetStateAction<number>> | null = null;
    render(
      <Harness
        hookKey="early"
        initial={0}
        emit={(_v, s) => (setter = s)}
      />
    );
    // setValue before load resolves
    await act(async () => {
      setter!(99);
    });
    expect(setConfigMock).not.toHaveBeenCalled();
    // Resolve load → still no persist for the load itself.
    await act(async () => {
      resolveLoad(undefined);
      await Promise.resolve();
    });
    expect(setConfigMock).not.toHaveBeenCalled();
  });
});
