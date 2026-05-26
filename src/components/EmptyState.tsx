import React from "react";
import { Button } from "@/components/ui/button";

export function EmptyState({
  icon,
  title,
  description,
  action
}: {
  icon: React.ReactNode;
  title: string;
  description?: React.ReactNode;
  action?: { label: string; onClick: () => void };
}) {
  return (
    <div className="flex-1 min-h-[200px] flex flex-col items-center justify-center text-center gap-2 px-6 py-10 text-muted-foreground">
      <span className="text-muted-foreground/60">{icon}</span>
      <span className="text-sm font-medium text-foreground">{title}</span>
      {description && (
        <span className="text-xs max-w-sm leading-relaxed">{description}</span>
      )}
      {action && (
        <Button type="button" variant="outline" size="sm" className="mt-2" onClick={action.onClick}>
          {action.label}
        </Button>
      )}
    </div>
  );
}
