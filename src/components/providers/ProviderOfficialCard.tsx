import { Badge } from "@/components/ui/badge";
import { Button } from "@/components/ui/button";
import { Card, CardContent } from "@/components/ui/card";
import { BrandIcon } from "@/components/BrandIcon";
import { CLI_APP_SOURCE_BADGE } from "@/constants";
import { cn } from "@/lib/utils";
import type { CliApp } from "@/types";

const OFFICIAL_DESCRIPTION: Record<CliApp, string> = {
  claude: "Uses the official Claude Code configuration.",
  codex: "Uses the official Codex configuration.",
  gemini: "Uses the official Gemini CLI configuration.",
  opencode: "Uses the official OpenCode configuration."
};

export function ProviderOfficialCard({
  app,
  isInUse,
  settingDefault,
  onSetDefault
}: {
  app: CliApp;
  isInUse: boolean;
  settingDefault: boolean;
  onSetDefault: () => void;
}) {
  return (
    <Card className={cn("p-3 gap-0 outline outline-1 outline-foreground/5", isInUse && "outline-primary/15 bg-primary/10")}>
      <CardContent className="px-0 flex items-center gap-3 min-h-7">
        <span className="shrink-0 inline-flex items-center justify-center size-10 rounded-md bg-background shadow-sm [&_svg]:size-5">
          <BrandIcon source={CLI_APP_SOURCE_BADGE[app]} />
        </span>
        <div className="flex-1 min-w-0 flex flex-col gap-1">
          <div className="flex items-center gap-2">
            <h3 className="text-lg font-medium">Official</h3>
            {isInUse && (
              <Badge className="uppercase text-[9px] tracking-wide px-1.5 py-0">In use</Badge>
            )}
          </div>
          <p className="text-xs text-muted-foreground leading-snug">
            {OFFICIAL_DESCRIPTION[app]}
          </p>
        </div>
        {!isInUse && (
          <Button
            type="button"
            variant="outline"
            size="sm"
            onClick={onSetDefault}
            disabled={settingDefault}
          >
            {settingDefault ? "Setting…" : "Set as default"}
          </Button>
        )}
      </CardContent>
    </Card>
  );
}
