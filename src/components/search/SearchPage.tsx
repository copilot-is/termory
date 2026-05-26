import React from "react";
import { BookOpen, Loader2, MessageSquare, Search, Sparkles } from "lucide-react";
import { Input } from "@/components/ui/input";
import { useSearchHits } from "@/hooks/useSearchHits";
import { formatFullNumber } from "@/lib/format";
import { isMemoryItem, isSkillItem } from "@/lib/session-utils";
import type { AppSession, SearchHit } from "@/types";
import { EmptyState } from "@/components/EmptyState";
import { SearchGroup } from "./SearchGroup";

export function SearchPage({
  sessions,
  onOpenItem,
  recentSearches,
  onCommitSearch,
  onClearRecent
}: {
  sessions: AppSession[];
  onOpenItem: (item: AppSession) => void;
  recentSearches: string[];
  onCommitSearch: (query: string) => void;
  onClearRecent: () => void;
}) {
  const [query, setQuery] = React.useState("");
  const { hits, loading, committedQuery, error } = useSearchHits(query);
  const inputRef = React.useRef<HTMLInputElement>(null);

  React.useEffect(() => {
    inputRef.current?.focus();
  }, []);

  const handleOpen = React.useCallback(
    (item: AppSession) => {
      onCommitSearch(committedQuery || query);
      onOpenItem(item);
    },
    [committedQuery, onCommitSearch, onOpenItem, query]
  );

  const groups = React.useMemo(() => {
    const sessionHits: SearchHit[] = [];
    const memoryHits: SearchHit[] = [];
    const skillHits: SearchHit[] = [];
    for (const hit of hits) {
      if (isMemoryItem(hit.session)) memoryHits.push(hit);
      else if (isSkillItem(hit.session)) skillHits.push(hit);
      else sessionHits.push(hit);
    }
    return { sessions: sessionHits, memories: memoryHits, skills: skillHits };
  }, [hits]);

  const trimmed = query.trim();
  const settled = committedQuery === trimmed && trimmed.length >= 2;
  const noResults = settled && !loading && hits.length === 0;

  return (
    <div className="flex-1 min-h-0 flex flex-col bg-background">
      <div className="flex flex-col gap-2 px-6 py-3 bg-card border-b border-border">
        <div className="relative flex items-center">
          <Search className="absolute left-3 size-4 text-muted-foreground pointer-events-none" />
          <Input
            ref={inputRef}
            type="search"
            placeholder="Search across sessions, memories, skills…"
            value={query}
            onChange={(event) => setQuery(event.target.value)}
            autoFocus
            autoComplete="off"
            autoCorrect="off"
            autoCapitalize="off"
            spellCheck={false}
            className="pl-9 pr-9"
          />
          {loading && (
            <Loader2 className="absolute right-3 size-4 animate-spin text-muted-foreground" />
          )}
        </div>
        {settled && hits.length > 0 && (
          <div className="flex items-center gap-2 text-xs text-muted-foreground">
            <span>
              {formatFullNumber(hits.length)} {hits.length === 1 ? "match" : "matches"}
            </span>
            <span className="text-border">·</span>
            <span>Sessions {groups.sessions.length}</span>
            <span className="text-border">·</span>
            <span>Memories {groups.memories.length}</span>
            <span className="text-border">·</span>
            <span>Skills {groups.skills.length}</span>
          </div>
        )}
      </div>
      <div className="flex-1 min-h-0 overflow-auto px-6 py-5 flex flex-col gap-5">
        {error && (
          <div className="rounded-md border border-destructive/30 bg-destructive/10 text-destructive text-sm px-3 py-2">
            {error}
          </div>
        )}
        {trimmed.length < 2 && !loading && (
          <div className="flex flex-col items-center justify-center text-center gap-3 py-12 text-muted-foreground">
            <Search className="size-7" />
            <p className="text-sm">Search inside every session, memory, and skill Termory scans.</p>
            <p className="flex items-center gap-1 text-xs">
              <span>Press</span>
              <kbd className="inline-flex h-5 items-center justify-center rounded border border-border bg-muted px-1.5 text-[10px] font-medium font-mono">⌘</kbd>
              <kbd className="inline-flex h-5 items-center justify-center rounded border border-border bg-muted px-1.5 text-[10px] font-medium font-mono">K</kbd>
              <span>to summon search from anywhere.</span>
            </p>
            <p className="text-xs">{formatFullNumber(sessions.length)} records indexed.</p>
            {recentSearches.length > 0 && (
              <div className="w-full max-w-md mt-4 flex flex-col gap-2 items-center">
                <div className="flex items-center gap-3 text-xs">
                  <span>Recent</span>
                  <button
                    type="button"
                    onClick={onClearRecent}
                    className="text-muted-foreground hover:text-foreground"
                  >
                    Clear
                  </button>
                </div>
                <div className="flex flex-wrap justify-center gap-1.5">
                  {recentSearches.map((entry) => (
                    <button
                      key={entry}
                      type="button"
                      onClick={() => setQuery(entry)}
                      className="inline-flex items-center gap-1 rounded-full border border-border bg-muted px-2.5 py-1 text-xs hover:bg-accent"
                    >
                      {entry}
                    </button>
                  ))}
                </div>
              </div>
            )}
          </div>
        )}
        {noResults && <EmptyState icon={<Search />} title={`No matches for "${trimmed}"`} />}
        {groups.sessions.length > 0 && (
          <SearchGroup
            title="Sessions"
            icon={<MessageSquare size={14} />}
            hits={groups.sessions}
            query={committedQuery}
            onOpen={handleOpen}
          />
        )}
        {groups.memories.length > 0 && (
          <SearchGroup
            title="Memories"
            icon={<BookOpen size={14} />}
            hits={groups.memories}
            query={committedQuery}
            onOpen={handleOpen}
          />
        )}
        {groups.skills.length > 0 && (
          <SearchGroup
            title="Skills"
            icon={<Sparkles size={14} />}
            hits={groups.skills}
            query={committedQuery}
            onOpen={handleOpen}
          />
        )}
      </div>
    </div>
  );
}
