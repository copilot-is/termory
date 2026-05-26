import React from "react";
import { invoke } from "@tauri-apps/api/core";
import { listen } from "@tauri-apps/api/event";
import { Plug, Plus } from "lucide-react";
import { toast } from "sonner";
import { Button } from "@/components/ui/button";
import { cn } from "@/lib/utils";
import {
  ACTIVE_STATE_REFRESH_EVENT,
  CLI_APPS,
  CLI_APP_LABEL,
  CLI_APP_SOURCE_BADGE
} from "@/constants";
import { blankProvider } from "@/lib/provider-utils";
import type { ActiveState, CliApp, Provider, TestResult } from "@/types";
import { BrandIcon } from "@/components/BrandIcon";
import { EmptyState } from "@/components/EmptyState";
import { ProviderCard } from "./ProviderCard";
import { ProviderEditor } from "./ProviderEditor";
import { ProviderOfficialCard } from "./ProviderOfficialCard";

export function ProvidersPage({
  providers,
  setProviders,
  app,
  setApp
}: {
  providers: Provider[];
  setProviders: React.Dispatch<React.SetStateAction<Provider[]>>;
  app: CliApp;
  setApp: (next: CliApp) => void;
}) {
  const [editing, setEditing] = React.useState<Provider | null>(null);
  const [editingIsNew, setEditingIsNew] = React.useState(false);
  const [activeStates, setActiveStates] = React.useState<Record<CliApp, ActiveState | null>>({
    claude: null,
    codex: null,
    gemini: null,
    opencode: null
  });
  const [toggling, setToggling] = React.useState<string | null>(null);
  const [testing, setTesting] = React.useState<string | null>(null);
  const [testResults, setTestResults] = React.useState<Record<string, TestResult>>({});
  const [settingDefault, setSettingDefault] = React.useState<string | null>(null);

  const refreshActive = React.useCallback(async () => {
    try {
      const states = await invoke<ActiveState[]>("provider_active_states", { providers });
      const next: Record<CliApp, ActiveState | null> = {
        claude: null,
        codex: null,
        gemini: null,
        opencode: null
      };
      for (const s of states) next[s.app] = s;
      setActiveStates(next);
    } catch (err) {
      toast.error(`Read live state failed: ${String(err)}`);
    }
  }, [providers]);

  React.useEffect(() => {
    void refreshActive();
  }, [refreshActive]);

  // Auto refresh when the Rust watcher detects any change in the
  // CLI dirs (live config files live inside those dirs). Reuse the
  // existing `termory:sources-changed` event — payload is ignored.
  React.useEffect(() => {
    const unlistenPromise = listen("termory:sources-changed", () => {
      void refreshActive();
    });
    const peerHandler = () => void refreshActive();
    window.addEventListener(ACTIVE_STATE_REFRESH_EVENT, peerHandler);
    return () => {
      void unlistenPromise.then((fn) => fn()).catch(() => {});
      window.removeEventListener(ACTIVE_STATE_REFRESH_EVENT, peerHandler);
    };
  }, [refreshActive]);

  const providersForApp = React.useMemo(
    () => providers.filter((p) => p.app === app),
    [providers, app]
  );
  const customProviders = React.useMemo(
    () => providersForApp.filter((p) => p.kind === "custom"),
    [providersForApp]
  );
  const activeState = activeStates[app];

  const startNew = () => {
    setEditing(blankProvider(app));
    setEditingIsNew(true);
  };
  const startEdit = (p: Provider) => {
    setEditing({ ...p });
    setEditingIsNew(false);
  };
  const closeEditor = () => {
    setEditing(null);
    setEditingIsNew(false);
  };

  const saveProvider = (next: Provider) => {
    setProviders((cur) => {
      const idx = cur.findIndex((p) => p.id === next.id);
      if (idx === -1) return [...cur, next];
      const copy = cur.slice();
      copy[idx] = next;
      return copy;
    });
    closeEditor();
  };

  const deleteProvider = async (id: string) => {
    if (!window.confirm("Delete this provider?")) return;
    const target = providers.find((p) => p.id === id);
    if (!target) return;
    const isInUse = activeStates[target.app]?.matchedProviderId === id;
    try {
      if (target.app === "opencode") {
        // OpenCode is multi-slot — delete only this provider's slot
        // from opencode.json (and the top-level model if it pointed
        // at this provider). Other Termory slots stay intact.
        await invoke("delete_provider_entry", { provider: target });
      } else if (isInUse) {
        // Single-slot CLIs — when the deleted one is the live record,
        // full deactivate clears Termory's writes so the CLI falls
        // back to its native auth.
        await invoke("deactivate_provider", {
          app: target.app,
          providersForApp: providers.filter((p) => p.app === target.app)
        });
      }
    } catch (err) {
      toast.error(`Could not clear ${CLI_APP_LABEL[target.app]} live config: ${String(err)}`);
      return;
    }
    setProviders((cur) => cur.filter((p) => p.id !== id));
    await refreshActive();
  };

  // OpenCode-only: toggle the provider's slot in opencode.json.
  // Enabled means the slot exists (multi-slot coexist). Other CLIs
  // don't have this concept — they only have "Set as default".
  const toggleEnabled = async (target: Provider) => {
    if (target.app !== "opencode") return;
    const state = activeStates[target.app];
    const enabled = (state?.configuredProviderIds ?? []).includes(target.id);
    setToggling(target.id);
    try {
      if (enabled) {
        await invoke("delete_provider_entry", { provider: target });
        toast.success(`Disabled ${target.name || "(unnamed)"}.`);
      } else {
        await invoke("activate_provider", {
          provider: target,
          providersForApp
        });
        toast.success(`Enabled ${target.name || "(unnamed)"}.`);
      }
      await refreshActive();
    } catch (err) {
      toast.error(String(err));
    } finally {
      setToggling(null);
    }
  };

  // Universal "Set as default" — promotes a provider to "In use".
  const setAsDefault = async (target: Provider) => {
    setSettingDefault(target.id);
    try {
      if (target.app === "opencode") {
        await invoke("set_opencode_default_provider", { provider: target });
      } else {
        await invoke("activate_provider", {
          provider: target,
          providersForApp
        });
      }
      toast.success(`${target.name || "(unnamed)"} is now in use.`);
      await refreshActive();
    } catch (err) {
      toast.error(String(err));
    } finally {
      setSettingDefault(null);
    }
  };

  // Official "Set as default" — clears Termory writes from the CLI's
  // live config so it falls back to its native auth flow.
  const setOfficialAsDefault = async () => {
    setSettingDefault("__official__");
    try {
      await invoke("deactivate_provider", {
        app,
        providersForApp
      });
      toast.success(`Official is now in use for ${CLI_APP_LABEL[app]}.`);
      await refreshActive();
    } catch (err) {
      toast.error(String(err));
    } finally {
      setSettingDefault(null);
    }
  };

  const testOne = async (target: Provider) => {
    setTesting(target.id);
    try {
      const result = await invoke<TestResult>("test_provider_api", { provider: target });
      setTestResults((cur) => ({ ...cur, [target.id]: result }));
    } catch (err) {
      setTestResults((cur) => ({
        ...cur,
        [target.id]: {
          ok: false,
          status: null,
          latencyMs: 0,
          message: String(err)
        }
      }));
    } finally {
      setTesting(null);
    }
  };

  return (
    <div className="flex-1 min-h-0 flex flex-col bg-background">
      <div className="flex items-center gap-3 px-6 bg-card border-b border-border">
        <div role="tablist" className="flex-1 flex overflow-x-auto">
          {CLI_APPS.map((id) => {
            const isActive = app === id;
            return (
              <button
                key={id}
                type="button"
                role="tab"
                aria-selected={isActive}
                onClick={() => setApp(id)}
                className={cn(
                  "relative inline-flex items-center gap-2 px-3.5 py-3 text-sm whitespace-nowrap shrink-0 transition-colors border-b-2 -mb-px",
                  isActive
                    ? "text-foreground border-primary"
                    : "text-muted-foreground border-transparent hover:text-foreground"
                )}
              >
                <BrandIcon source={CLI_APP_SOURCE_BADGE[id]} />
                <span>{CLI_APP_LABEL[id]}</span>
              </button>
            );
          })}
        </div>
        <Button
          type="button"
          size="icon"
          onClick={startNew}
          aria-label="Add provider"
          title="Add provider"
          className="rounded-full size-7 shrink-0"
        >
          <Plus className="size-4" />
        </Button>
      </div>

      <div className="flex-1 min-h-0 overflow-auto px-6 py-5">
        <div className="flex flex-col gap-2.5">
          <ProviderOfficialCard
            isInUse={activeState?.kind === "official"}
            settingDefault={settingDefault === "__official__"}
            onSetDefault={() => void setOfficialAsDefault()}
          />

          {customProviders.map((p) => {
            const configuredIds = activeState?.configuredProviderIds ?? [];
            const matchedId = activeState?.matchedProviderId ?? null;
            const isOpencode = p.app === "opencode";
            const isConfigured = isOpencode
              ? configuredIds.includes(p.id)
              : matchedId === p.id;
            const isInUse = matchedId === p.id;
            return (
              <ProviderCard
                key={p.id}
                provider={p}
                isConfigured={isConfigured}
                isInUse={isInUse}
                toggling={toggling === p.id}
                settingDefault={settingDefault === p.id}
                testing={testing === p.id}
                testResult={testResults[p.id]}
                onToggleEnabled={isOpencode ? () => void toggleEnabled(p) : undefined}
                onSetDefault={() => void setAsDefault(p)}
                onEdit={() => startEdit(p)}
                onDelete={() => deleteProvider(p.id)}
                onTest={() => void testOne(p)}
              />
            );
          })}

          {customProviders.length === 0 && (
            <EmptyState
              icon={<Plug size={32} />}
              title="No custom providers yet"
              description={`Add a third-party API platform for ${CLI_APP_LABEL[app]} and switch to it with one click.`}
              action={{ label: "+ Add provider", onClick: startNew }}
            />
          )}
        </div>
      </div>

      {editing && (
        <ProviderEditor
          provider={editing}
          isNew={editingIsNew}
          onSave={saveProvider}
          onClose={closeEditor}
        />
      )}
    </div>
  );
}
