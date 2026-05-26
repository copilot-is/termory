import { Badge } from "@/components/ui/badge";
import { Button } from "@/components/ui/button";
import { Card, CardContent } from "@/components/ui/card";
import { cn } from "@/lib/utils";

export function ProviderOfficialCard({
  isInUse,
  settingDefault,
  onSetDefault
}: {
  isInUse: boolean;
  settingDefault: boolean;
  onSetDefault: () => void;
}) {
  return (
    <Card className={cn("py-3 gap-0", isInUse && "border-primary bg-primary/5")}>
      <CardContent className="px-4 flex items-center justify-between gap-3 min-h-7">
        <div className="flex items-center gap-2">
          <h3 className="text-[14.5px] font-medium leading-none">Official</h3>
          {isInUse && (
            <Badge className="uppercase text-[10.5px] tracking-wide">In use</Badge>
          )}
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
