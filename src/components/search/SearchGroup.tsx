import React from "react";
import type { AppSession, SearchHit } from "@/types";
import { formatFullNumber } from "@/lib/format";
import { sessionKey } from "@/lib/session-utils";
import { SearchResultCard } from "./SearchResultCard";

export function SearchGroup({
  title,
  icon,
  hits,
  query,
  onOpen
}: {
  title: string;
  icon: React.ReactNode;
  hits: SearchHit[];
  query: string;
  onOpen: (item: AppSession) => void;
}) {
  const limit = 50;
  const visible = hits.slice(0, limit);
  const truncated = hits.length - visible.length;
  return (
    <section className="flex flex-col gap-2">
      <header className="flex items-center gap-2 text-xs uppercase tracking-wide text-muted-foreground">
        <span className="text-muted-foreground/80">{icon}</span>
        <h3 className="text-xs font-semibold text-foreground">{title}</h3>
        <span className="text-muted-foreground tabular-nums">{hits.length}</span>
      </header>
      <div className="flex flex-col gap-1.5">
        {visible.map((hit) => (
          <SearchResultCard
            key={sessionKey(hit.session)}
            hit={hit}
            query={query}
            onOpen={() => onOpen(hit.session)}
          />
        ))}
      </div>
      {truncated > 0 && (
        <div className="text-xs text-muted-foreground pl-2">
          + {formatFullNumber(truncated)} more
        </div>
      )}
    </section>
  );
}
