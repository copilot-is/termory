import { AlertTriangle, Check, Loader2, Pencil, Trash2, Zap } from "lucide-react";
import { Badge } from "@/components/ui/badge";
import { Button } from "@/components/ui/button";
import { Card, CardContent } from "@/components/ui/card";
import { cn } from "@/lib/utils";
import { maskKey } from "@/lib/provider-utils";
import type { Provider, TestResult } from "@/types";

export function ProviderCard({
  provider,
  isConfigured,
  isInUse,
  toggling,
  settingDefault,
  testing,
  testResult,
  onToggleEnabled,
  onSetDefault,
  onEdit,
  onDelete,
  onTest
}: {
  provider: Provider;
  // OpenCode: slot exists in opencode.json. Other CLIs: same as isInUse
  // (single-slot — Enabled ≡ In use, so the Enable concept doesn't
  // surface separately).
  isConfigured: boolean;
  // Universal: CLI is currently using this provider.
  isInUse: boolean;
  // OpenCode-only pending state for the Enable/Disable toggle.
  toggling: boolean;
  settingDefault: boolean;
  testing: boolean;
  testResult: TestResult | undefined;
  // OpenCode-only: toggle the slot in opencode.json. Undefined for
  // other CLIs (their Enabled state isn't separately controllable).
  onToggleEnabled?: () => void;
  onSetDefault: () => void;
  onEdit: () => void;
  onDelete: () => void;
  onTest: () => void;
}) {
  const isOpencode = provider.app === "opencode";
  return (
    <Card className={cn("py-3 gap-0 outline outline-1 outline-foreground/5", isInUse && "outline-primary/15 bg-primary/10")}>
      <CardContent className="px-4 flex flex-col gap-2.5">
        <div className="flex items-center justify-between gap-3 flex-wrap min-h-7">
          <div className="flex-1 min-w-0 flex flex-col gap-2">
            <div className="flex items-center gap-2">
              <h3 className="text-lg font-medium">
                {provider.name || "(unnamed)"}
              </h3>
              {isInUse && (
                <Badge className="uppercase text-[9px] tracking-wide px-1.5 py-0">In use</Badge>
              )}
            </div>
            {(provider.baseUrl || provider.apiKey || provider.model) && (
              <dl className="grid grid-cols-[max-content_1fr] gap-x-3.5 gap-y-1 text-xs">
                {provider.baseUrl && (
                  <>
                    <dt className="text-muted-foreground">Base URL</dt>
                    <dd className="font-mono break-all">{provider.baseUrl}</dd>
                  </>
                )}
                {provider.apiKey && (
                  <>
                    <dt className="text-muted-foreground">API key</dt>
                    <dd className="font-mono break-all">{maskKey(provider.apiKey)}</dd>
                  </>
                )}
                {provider.model && (
                  <>
                    <dt className="text-muted-foreground">Model</dt>
                    <dd className="font-mono break-all">{provider.model}</dd>
                  </>
                )}
              </dl>
            )}
          </div>
          <div className="inline-flex items-center gap-1.5 shrink-0">
            {!isInUse && (
              <Button
                type="button"
                variant="outline"
                size="sm"
                onClick={onSetDefault}
                disabled={settingDefault || (isOpencode && !isConfigured)}
                title={
                  isOpencode && !isConfigured
                    ? "Enable this provider first."
                    : undefined
                }
              >
                {settingDefault ? "Setting…" : "Set as default"}
              </Button>
            )}
            {onToggleEnabled && (
              <Button
                type="button"
                variant="ghost"
                size="sm"
                onClick={onToggleEnabled}
                disabled={toggling}
              >
                {toggling
                  ? isConfigured
                    ? "Disabling…"
                    : "Enabling…"
                  : isConfigured
                    ? "Disable"
                    : "Enable"}
              </Button>
            )}
            <Button
              type="button"
              variant="ghost"
              size="icon"
              className="size-7"
              onClick={onTest}
              disabled={testing}
              title="Test"
              aria-label="Test"
            >
              {testing ? <Loader2 className="size-4 animate-spin" /> : <Zap className="size-4" />}
            </Button>
            <Button
              type="button"
              variant="ghost"
              size="icon"
              className="size-7"
              onClick={onEdit}
              title="Edit"
              aria-label="Edit"
            >
              <Pencil className="size-4" />
            </Button>
            <Button
              type="button"
              variant="ghost"
              size="icon"
              className="size-7 text-destructive hover:text-destructive hover:bg-destructive/10"
              onClick={onDelete}
              title="Delete"
              aria-label="Delete"
            >
              <Trash2 className="size-4" />
            </Button>
          </div>
        </div>
        {testResult && (
          <div
            className={cn(
              "flex items-center gap-2 text-xs px-2.5 py-1.5 rounded-md",
              testResult.ok
                ? "bg-primary/10 text-primary"
                : "bg-destructive/10 text-destructive"
            )}
          >
            {testResult.ok ? <Check className="size-3.5" /> : <AlertTriangle className="size-3.5" />}
            <span>
              {testResult.status ? `HTTP ${testResult.status}` : "no response"} ·{" "}
              {testResult.latencyMs}ms · {testResult.message}
            </span>
          </div>
        )}
      </CardContent>
    </Card>
  );
}
