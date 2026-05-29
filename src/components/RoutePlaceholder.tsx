import { Card, CardContent } from "@/components/ui/card";
import type { Route } from "@/types";

export function RoutePlaceholder({ route }: { route: Route }) {
  // Only `stats` is still a placeholder route — every other entry's
  // page is implemented and routed before this fallback is reached.
  // Keeping the records / search / config / settings labels stops a
  // future routing regression from rendering "Lands in a later phase"
  // for an actually-shipped page.
  const labels: Record<Route, { title: string; detail: string }> = {
    records: { title: "Records", detail: "" },
    search: { title: "Search", detail: "" },
    stats: {
      title: "Stats",
      detail:
        "Dashboards (sessions / day, tokens per tool, top projects) land here in a later phase."
    },
    config: { title: "Providers", detail: "" },
    settings: { title: "Settings", detail: "" }
  };
  const { title, detail } = labels[route];
  return (
    <div className="flex-1 min-h-0 flex items-center justify-center px-6 py-10 bg-background">
      <Card className="max-w-md w-full">
        <CardContent className="px-6 py-5 flex flex-col gap-2">
          <h2 className="text-lg font-semibold">{title}</h2>
          <p className="text-sm text-muted-foreground leading-relaxed">{detail}</p>
        </CardContent>
      </Card>
    </div>
  );
}
