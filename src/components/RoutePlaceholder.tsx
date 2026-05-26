import { Card, CardContent } from "@/components/ui/card";
import type { Route } from "@/types";

export function RoutePlaceholder({ route }: { route: Route }) {
  const labels: Record<Route, { title: string; detail: string }> = {
    records: { title: "Records", detail: "" },
    search: { title: "Search", detail: "" },
    stats: {
      title: "Stats",
      detail:
        "Dashboards (sessions / day, tokens per tool, top projects) land here in a later phase."
    },
    config: {
      title: "Providers",
      detail:
        "Per-CLI provider profile editor — base URL / API key / model, with quick-switch. Lands in a later phase."
    },
    settings: {
      title: "Settings",
      detail:
        "App preferences (theme, scan paths, keyboard shortcuts). Lands in a later phase."
    }
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
