import React from "react";
import {
  BarChart3,
  History,
  Plug,
  Search,
  Settings as SettingsIcon
} from "lucide-react";
import {
  Tooltip,
  TooltipContent,
  TooltipTrigger
} from "@/components/ui/tooltip";
import { cn } from "@/lib/utils";
import type { Route } from "@/types";

export function ActivityRail({
  route,
  onChange
}: {
  route: Route;
  onChange: (next: Route) => void;
}) {
  const items: { id: Route; icon: React.ReactNode; label: string }[] = [
    { id: "records", icon: <History size={20} />, label: "Records" },
    { id: "search", icon: <Search size={20} />, label: "Search" },
    { id: "stats", icon: <BarChart3 size={20} />, label: "Stats" },
    { id: "config", icon: <Plug size={20} />, label: "Providers" },
    { id: "settings", icon: <SettingsIcon size={20} />, label: "Settings" }
  ];
  return (
    <nav
      aria-label="Primary"
      className="flex flex-col items-center gap-1.5 px-1.5 py-3 bg-sidebar border-r border-sidebar-border shrink-0"
    >
      {items.map((item) => {
        const isActive = route === item.id;
        return (
          <Tooltip key={item.id}>
            <TooltipTrigger asChild>
              <button
                type="button"
                onClick={() => onChange(item.id)}
                aria-label={item.label}
                aria-current={isActive ? "page" : undefined}
                className={cn(
                  "inline-flex items-center justify-center rounded-md size-9 transition-colors",
                  isActive
                    ? "bg-sidebar-accent text-sidebar-accent-foreground"
                    : "text-muted-foreground hover:bg-sidebar-accent/60 hover:text-sidebar-accent-foreground"
                )}
              >
                {item.icon}
              </button>
            </TooltipTrigger>
            <TooltipContent side="right">{item.label}</TooltipContent>
          </Tooltip>
        );
      })}
    </nav>
  );
}
