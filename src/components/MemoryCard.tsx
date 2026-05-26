import { Folder } from "lucide-react";
import { cn } from "@/lib/utils";
import { formatDate, formatRelativeDate } from "@/lib/format";
import {
  memoryToolsOf,
  projectDisplayName,
  sourceDisplayName
} from "@/lib/session-utils";
import type { AppSession, SearchHit } from "@/types";
import { BrandIcon } from "./BrandIcon";
import { SnippetLine } from "./SnippetLine";

export function MemoryCard({
  item,
  selected,
  onClick,
  query,
  contentQuery,
  hit,
  showSource
}: {
  item: AppSession;
  selected: AppSession | null;
  onClick: () => void;
  query: string;
  contentQuery: string;
  hit: SearchHit | undefined;
  showSource: boolean;
}) {
  const showSnippet = !!hit && query.toLowerCase() === contentQuery.toLowerCase();
  const isActive = selected?.path === item.path && selected?.id === item.id;
  const tools = memoryToolsOf(item);
  return (
    <button
      onClick={onClick}
      title={item.snippet || undefined}
      className={cn(
        "w-full text-left rounded-md border px-3 py-2 transition-colors flex flex-col gap-1",
        isActive
          ? "border-primary bg-primary/5"
          : "border-border bg-card hover:bg-accent/40"
      )}
    >
      <div className="flex items-baseline justify-between gap-2">
        <h2 className="text-sm font-medium leading-snug line-clamp-2 flex-1 min-w-0">
          {item.title}
        </h2>
        <span
          className="text-xs text-muted-foreground shrink-0"
          title={formatDate(item.updated_at ?? item.started_at)}
        >
          {formatRelativeDate(item.updated_at ?? item.started_at)}
        </span>
      </div>
      <div className="flex items-center justify-between gap-2 text-xs text-muted-foreground">
        <span className="flex items-center gap-1 min-w-0" title={item.project}>
          <Folder size={12} className="shrink-0" />
          <span className="truncate">{projectDisplayName(item.project)}</span>
        </span>
        {showSource && (
          <span className="flex items-center gap-1 shrink-0">
            {tools.map((tool) => {
              const label = tool === "Other" ? "Memory" : sourceDisplayName(tool);
              return (
                <span key={tool} title={label} aria-label={label}>
                  <BrandIcon source={tool === "Other" ? "Memory" : tool} />
                </span>
              );
            })}
          </span>
        )}
      </div>
      {showSnippet && hit && (
        <SnippetLine
          snippet={hit.snippet}
          query={query}
          role={hit.role}
          matchCount={hit.match_count}
        />
      )}
    </button>
  );
}
