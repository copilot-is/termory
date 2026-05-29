import React from "react";
import { MessageSquare } from "lucide-react";
import { splitSnippet } from "@/lib/search-utils";

export function SnippetLine({
  snippet,
  query,
  role,
  matchCount,
  truncated
}: {
  snippet: string;
  query: string;
  role: string;
  matchCount: number;
  truncated?: boolean;
}) {
  const segments = React.useMemo(() => splitSnippet(snippet, query), [snippet, query]);
  const label = role ? role : "match";
  return (
    <div className="flex flex-col gap-0.5 mt-1 pt-1">
      <span className="flex items-center gap-1 text-[10.5px] uppercase tracking-wide text-muted-foreground">
        <MessageSquare size={11} />
        <span>{label}</span>
        {matchCount > 1 && (
          <span className="text-muted-foreground/70">
            ×{matchCount}
            {truncated && "+"}
          </span>
        )}
      </span>
      <span className="text-xs text-muted-foreground line-clamp-2">
        {segments.map((seg, index) =>
          seg.match ? (
            <mark key={index} className="bg-primary/20 text-foreground rounded px-0.5">
              {seg.text}
            </mark>
          ) : (
            <span key={index}>{seg.text}</span>
          )
        )}
      </span>
    </div>
  );
}
