import React from "react";
import { invoke } from "@tauri-apps/api/core";
import { listen } from "@tauri-apps/api/event";
import { revealItemInDir } from "@tauri-apps/plugin-opener";
import {
  BookOpen,
  ChevronDown,
  ChevronRight,
  Clock,
  ExternalLink,
  File,
  FileJson,
  Folder,
  Loader2,
  MessageSquare,
  Sparkles
} from "lucide-react";
import { Button } from "@/components/ui/button";
import { cn } from "@/lib/utils";
import type {
  AppSession,
  CliApp,
  MemoryTool,
  Provider,
  Route,
  SearchHit,
  SessionDetail
} from "@/types";
import { formatDate, formatFullNumber, formatRelativeDate } from "@/lib/format";
import {
  basename,
  isMemoryItem,
  isSessionItem,
  isSkillItem,
  memoryToolsOf,
  projectDisplayName,
  readRouteFromHash,
  resumeCommandFor,
  roleClass,
  sessionKey,
  sourceDisplayName
} from "@/lib/session-utils";
import { isProviderList } from "@/lib/provider-utils";
import { addSetValue, toggleSetValue } from "@/lib/set-utils";
import { usePersistentState } from "@/hooks/usePersistentState";
import { ActivityRail } from "@/components/ActivityRail";
import { BrandIcon } from "@/components/BrandIcon";
import { CommandPalette } from "@/components/CommandPalette";
import { CopyMenu } from "@/components/CopyMenu";
import { EmptyState } from "@/components/EmptyState";
import { FreshnessFooter } from "@/components/FreshnessFooter";
import { MemoryCard } from "@/components/MemoryCard";
import { MessageBody } from "@/components/MessageBody";
import { RoutePlaceholder } from "@/components/RoutePlaceholder";
import { SnippetLine } from "@/components/SnippetLine";
import { SearchPage } from "@/components/search/SearchPage";
import { ProvidersPage } from "@/components/providers/ProvidersPage";

export function App() {
  const [sessions, setSessions] = React.useState<AppSession[]>([]);
  const [selected, setSelected] = React.useState<AppSession | null>(null);
  const [detail, setDetail] = React.useState<SessionDetail | null>(null);
  const [query, setQuery] = React.useState("");
  const [source, setSource] = React.useState("All");
  const [project, setProject] = React.useState<string | null>(null);
  const [expandedSources, setExpandedSources] = React.useState<Set<string>>(() => new Set());
  const [loading, setLoading] = React.useState(true);
  const [detailLoading, setDetailLoading] = React.useState(false);
  const [error, setError] = React.useState<string | null>(null);
  const [contentHits, setContentHits] = React.useState<Map<string, SearchHit>>(() => new Map());
  const [, setSearchingContent] = React.useState(false);
  const [contentQuery, setContentQuery] = React.useState("");
  const [pane, setPane] = usePersistentState<"sessions" | "memory" | "skills">(
    "default_pane",
    "sessions",
    (raw): raw is "sessions" | "memory" | "skills" =>
      raw === "sessions" || raw === "memory" || raw === "skills"
  );
  const [recentSearches, setRecentSearches] = usePersistentState<string[]>(
    "recent_searches",
    [],
    (raw): raw is string[] =>
      Array.isArray(raw) && raw.every((entry) => typeof entry === "string")
  );
  const [providers, setProviders] = usePersistentState<Provider[]>(
    "providers",
    [],
    isProviderList
  );
  const [providersApp, setProvidersApp] = usePersistentState<CliApp>(
    "providers_app",
    "claude",
    (raw): raw is CliApp =>
      raw === "claude" || raw === "codex" || raw === "gemini" || raw === "opencode"
  );

  const addRecentSearch = React.useCallback(
    (raw: string) => {
      const trimmed = raw.trim();
      if (trimmed.length < 2) return;
      setRecentSearches((current) => {
        const filtered = current.filter((entry) => entry !== trimmed);
        return [trimmed, ...filtered].slice(0, 5);
      });
    },
    [setRecentSearches]
  );

  const clearRecentSearches = React.useCallback(() => {
    setRecentSearches([]);
  }, [setRecentSearches]);
  const [route, setRoute] = React.useState<Route>(() => readRouteFromHash());
  const [lastRefreshedAt, setLastRefreshedAt] = React.useState<number | null>(null);

  // Sync route ↔ URL hash so refresh / back-forward / deeplink work.
  React.useEffect(() => {
    const wanted = `#${route}`;
    if (window.location.hash !== wanted) {
      window.history.replaceState(null, "", wanted);
    }
  }, [route]);
  React.useEffect(() => {
    const onHashChange = () => setRoute(readRouteFromHash());
    window.addEventListener("hashchange", onHashChange);
    return () => window.removeEventListener("hashchange", onHashChange);
  }, []);

  // Shared "jump to this record" action — used by both the Search page
  // and the Cmd-K palette. Resets the Records sidebar filters so the
  // target item is visible in its pane and switches the pane tab to
  // match the item type before flipping the route.
  const openItem = React.useCallback(
    (item: AppSession) => {
      setSource("All");
      setProject(null);
      if (isMemoryItem(item)) setPane("memory");
      else if (isSkillItem(item)) setPane("skills");
      else setPane("sessions");
      setSelected(item);
      setRoute("records");
    },
    [setPane]
  );

  const applyScanResult = React.useCallback((result: AppSession[]) => {
    setSessions(result);
    setSelected((current) => {
      if (!current) return result[0] ?? null;
      return (
        result.find(
          (session) =>
            session.source === current.source &&
            session.path === current.path &&
            session.id === current.id
        ) ??
        result[0] ??
        null
      );
    });
    setLastRefreshedAt(Date.now());
  }, []);

  const refresh = React.useCallback(async () => {
    setLoading(true);
    setError(null);
    try {
      const result = await invoke<AppSession[]>("scan_all_sessions");
      applyScanResult(result);
    } catch (err) {
      setError(String(err));
    } finally {
      setLoading(false);
    }
  }, [applyScanResult]);

  React.useEffect(() => {
    const unlisten = listen<AppSession[]>("termory:sources-changed", (event) => {
      setError(null);
      applyScanResult(event.payload);
    });
    return () => {
      void unlisten.then((fn) => fn()).catch(() => {});
    };
  }, [applyScanResult]);

  React.useEffect(() => {
    refresh();
  }, [refresh]);

  React.useEffect(() => {
    if (!selected) {
      setDetail(null);
      return;
    }
    let cancelled = false;
    setDetailLoading(true);
    invoke<SessionDetail>("load_session", {
      source: selected.source,
      path: selected.path,
      id: selected.id
    })
      .then((result) => {
        if (!cancelled) setDetail(result);
      })
      .catch((err) => {
        if (!cancelled) setError(String(err));
      })
      .finally(() => {
        if (!cancelled) setDetailLoading(false);
      });
    return () => {
      cancelled = true;
    };
  }, [selected]);

  React.useEffect(() => {
    const trimmed = query.trim();
    if (trimmed.length < 2) {
      setContentHits(new Map());
      setContentQuery("");
      setSearchingContent(false);
      return;
    }
    let cancelled = false;
    const handle = window.setTimeout(() => {
      setSearchingContent(true);
      invoke<SearchHit[]>("search_all_sessions", { query: trimmed })
        .then((hits) => {
          if (cancelled) return;
          const map = new Map<string, SearchHit>();
          for (const hit of hits) map.set(sessionKey(hit.session), hit);
          setContentHits(map);
          setContentQuery(trimmed);
        })
        .catch((err) => {
          if (!cancelled) setError(String(err));
        })
        .finally(() => {
          if (!cancelled) setSearchingContent(false);
        });
    }, 300);
    return () => {
      cancelled = true;
      window.clearTimeout(handle);
    };
  }, [query]);

  const sessionItems = React.useMemo(
    () => sessions.filter((item) => !isMemoryItem(item) && !isSkillItem(item)),
    [sessions]
  );
  const memoryItems = React.useMemo(() => sessions.filter(isMemoryItem), [sessions]);
  const skillItems = React.useMemo(() => sessions.filter(isSkillItem), [sessions]);

  const filtered = React.useMemo(() => {
    const needle = query.trim().toLowerCase();
    return sessionItems.filter((session) => {
      if (source !== "All" && session.source !== source) return false;
      if (project && session.project !== project) return false;
      if (!needle) return true;
      const metaMatch = [
        session.title,
        session.project,
        session.source,
        session.id,
        session.path
      ]
        .join("\n")
        .toLowerCase()
        .includes(needle);
      if (metaMatch) return true;
      if (needle === contentQuery.toLowerCase() && contentHits.has(sessionKey(session))) {
        return true;
      }
      return false;
    });
  }, [sessionItems, query, source, project, contentHits, contentQuery]);

  const filteredMemories = React.useMemo(() => {
    const needle = query.trim().toLowerCase();
    return memoryItems.filter((memory) => {
      if (source !== "All") {
        const tools = memoryToolsOf(memory);
        if (!tools.includes(source as MemoryTool)) return false;
      }
      if (project && memory.project !== project) return false;
      if (!needle) return true;
      const metaMatch = [memory.title, memory.project, memory.id, memory.path]
        .join("\n")
        .toLowerCase()
        .includes(needle);
      if (metaMatch) return true;
      if (needle === contentQuery.toLowerCase() && contentHits.has(sessionKey(memory))) {
        return true;
      }
      return false;
    });
  }, [memoryItems, query, source, project, contentHits, contentQuery]);

  const filteredSkills = React.useMemo(() => {
    const needle = query.trim().toLowerCase();
    return skillItems.filter((skill) => {
      if (source !== "All") {
        const tools = memoryToolsOf(skill);
        if (!tools.includes(source as MemoryTool)) return false;
      }
      if (project && skill.project !== project) return false;
      if (!needle) return true;
      const metaMatch = [skill.title, skill.project, skill.id, skill.path]
        .join("\n")
        .toLowerCase()
        .includes(needle);
      if (metaMatch) return true;
      if (needle === contentQuery.toLowerCase() && contentHits.has(sessionKey(skill))) {
        return true;
      }
      return false;
    });
  }, [skillItems, query, source, project, contentHits, contentQuery]);

  const hasActiveFilters =
    source !== "All" || project !== null || query.trim().length > 0;

  const clearFilters = React.useCallback(() => {
    setSource("All");
    setProject(null);
    setQuery("");
  }, []);

  const sourceGroups = React.useMemo(() => {
    const sources: string[] = ["All", "Codex", "Claude", "Gemini", "OpenCode"];
    return sources.map((item) => {
      const sourceSessions =
        item === "All"
          ? sessionItems
          : sessionItems.filter((session) => session.source === item);
      const projects = Array.from(
        sourceSessions
          .filter((session) => item !== "All" && session.project)
          .reduce((map, session) => {
            map.set(session.project, (map.get(session.project) ?? 0) + 1);
            return map;
          }, new Map<string, number>())
      ).sort(([left], [right]) => left.localeCompare(right));

      return { source: item, count: sourceSessions.length, projects };
    });
  }, [sessionItems]);

  return (
    <div className="grid grid-rows-[1fr_auto] w-full h-screen text-foreground bg-background">
      <div className="grid grid-cols-[62px_1fr] min-h-0 min-w-0">
        <ActivityRail route={route} onChange={setRoute} />
        <div className="min-w-0 min-h-0 flex flex-col">
          {route === "search" && (
            <SearchPage
              sessions={sessions}
              onOpenItem={openItem}
              recentSearches={recentSearches}
              onCommitSearch={addRecentSearch}
              onClearRecent={clearRecentSearches}
            />
          )}
          {route === "config" && (
            <ProvidersPage
              providers={providers}
              setProviders={setProviders}
              app={providersApp}
              setApp={setProvidersApp}
            />
          )}
          {route !== "records" && route !== "search" && route !== "config" && (
            <RoutePlaceholder route={route} />
          )}
          {route === "records" && (
            <main className="flex-1 min-h-0 grid grid-cols-[220px_minmax(280px,360px)_1fr]">
              <aside className="flex flex-col min-h-0 bg-sidebar border-r border-sidebar-border">
                <div className="flex-1 min-h-0 overflow-auto px-2 py-2">
                  <div className="flex flex-col gap-0.5">
                    {sourceGroups.map((group) => {
                      const groupActive = source === group.source && !project;
                      return (
                        <div key={group.source} className="flex flex-col gap-0.5">
                          <button
                            aria-expanded={
                              group.projects.length > 0
                                ? expandedSources.has(group.source)
                                : undefined
                            }
                            onClick={() => {
                              setSource(group.source);
                              setProject(null);
                            }}
                            className={cn(
                              "w-full flex items-center justify-between gap-2 px-2 py-1.5 rounded-md text-sm transition-colors",
                              groupActive
                                ? "bg-sidebar-accent text-sidebar-accent-foreground"
                                : "text-sidebar-foreground hover:bg-sidebar-accent/60"
                            )}
                          >
                            <span className="flex items-center gap-2 min-w-0">
                              <span
                                role={group.projects.length > 0 ? "button" : undefined}
                                aria-label={
                                  group.projects.length > 0
                                    ? `${
                                        expandedSources.has(group.source)
                                          ? "Collapse"
                                          : "Expand"
                                      } ${group.source} projects`
                                    : undefined
                                }
                                tabIndex={group.projects.length > 0 ? 0 : undefined}
                                onClick={(event) => {
                                  if (group.projects.length === 0) return;
                                  event.stopPropagation();
                                  setExpandedSources((current) =>
                                    toggleSetValue(current, group.source)
                                  );
                                }}
                                onKeyDown={(event) => {
                                  if (group.projects.length === 0) return;
                                  if (event.key !== "Enter" && event.key !== " ") return;
                                  event.preventDefault();
                                  event.stopPropagation();
                                  setExpandedSources((current) =>
                                    toggleSetValue(current, group.source)
                                  );
                                }}
                                className={cn(
                                  "inline-flex items-center justify-center size-5 shrink-0 rounded relative",
                                  group.projects.length > 0 &&
                                    "hover:bg-sidebar-accent/40 cursor-pointer"
                                )}
                              >
                                {group.projects.length > 0 ? (
                                  <>
                                    <BrandIcon source={group.source} />
                                    <span className="absolute inset-0 inline-flex items-center justify-center opacity-0 hover:opacity-100 bg-sidebar-accent rounded transition-opacity">
                                      {expandedSources.has(group.source) ? (
                                        <ChevronDown size={14} />
                                      ) : (
                                        <ChevronRight size={14} />
                                      )}
                                    </span>
                                  </>
                                ) : (
                                  <BrandIcon source={group.source} />
                                )}
                              </span>
                              <span className="truncate">
                                {sourceDisplayName(group.source)}
                              </span>
                            </span>
                            <span className="text-xs text-muted-foreground tabular-nums shrink-0">
                              {group.count}
                            </span>
                          </button>

                          {group.projects.length > 0 &&
                            expandedSources.has(group.source) && (
                              <div className="pl-6 flex flex-col gap-0.5">
                                {group.projects.map(([projectName, count]) => {
                                  const projActive =
                                    source === group.source && project === projectName;
                                  return (
                                    <button
                                      key={`${group.source}:${projectName}`}
                                      title={`${projectName} - ${formatFullNumber(count)} Sessions`}
                                      onClick={() => {
                                        setSource(group.source);
                                        setProject(projectName);
                                        setExpandedSources((current) =>
                                          addSetValue(current, group.source)
                                        );
                                      }}
                                      className={cn(
                                        "w-full flex items-center justify-between gap-2 px-2 py-1 rounded-md text-xs transition-colors",
                                        projActive
                                          ? "bg-sidebar-accent text-sidebar-accent-foreground"
                                          : "text-muted-foreground hover:bg-sidebar-accent/60 hover:text-sidebar-foreground"
                                      )}
                                    >
                                      <span className="flex items-center gap-1.5 min-w-0">
                                        <Folder size={12} className="shrink-0" />
                                        <span className="truncate">
                                          {projectDisplayName(projectName)}
                                        </span>
                                      </span>
                                      <span className="tabular-nums shrink-0">{count}</span>
                                    </button>
                                  );
                                })}
                              </div>
                            )}
                        </div>
                      );
                    })}
                  </div>
                </div>
              </aside>

              <section className="flex flex-col min-h-0 bg-background border-r border-border">
                <div
                  role="tablist"
                  className="flex items-center gap-1 px-3 pt-3 pb-2 bg-card border-b border-border"
                >
                  {(
                    [
                      { id: "sessions", label: "Sessions", count: filtered.length },
                      { id: "memory", label: "Memories", count: filteredMemories.length },
                      { id: "skills", label: "Skills", count: filteredSkills.length }
                    ] as const
                  ).map((tab) => {
                    const isActive = pane === tab.id;
                    return (
                      <button
                        key={tab.id}
                        type="button"
                        role="tab"
                        aria-selected={isActive}
                        onClick={() => setPane(tab.id)}
                        className={cn(
                          "inline-flex items-center gap-1.5 px-2.5 py-1 rounded-md text-xs font-medium transition-colors",
                          isActive
                            ? "bg-accent text-accent-foreground"
                            : "text-muted-foreground hover:text-foreground"
                        )}
                      >
                        <span>{tab.label}</span>
                        <span
                          className={cn(
                            "tabular-nums",
                            isActive ? "text-foreground" : "text-muted-foreground/70"
                          )}
                        >
                          {tab.count}
                        </span>
                      </button>
                    );
                  })}
                </div>

                {error && (
                  <div className="m-3 rounded-md border border-destructive/30 bg-destructive/10 text-destructive text-sm px-3 py-2">
                    {error}
                  </div>
                )}

                {pane === "sessions" && (
                  <div className="flex-1 min-h-0 overflow-auto px-3 py-2 flex flex-col gap-1.5">
                    {loading && sessionItems.length === 0 && (
                      <EmptyState
                        icon={<Loader2 className="animate-spin" />}
                        title="Scanning local history"
                      />
                    )}
                    {!loading && filtered.length === 0 && sessionItems.length === 0 && (
                      <EmptyState
                        icon={<FileJson size={32} />}
                        title="No sessions yet"
                        description="Termory scans Codex, Claude Code, Gemini CLI, and OpenCode for chat history. None of those tools have recorded sessions here yet."
                      />
                    )}
                    {!loading && filtered.length === 0 && sessionItems.length > 0 && (
                      <EmptyState
                        icon={<FileJson size={32} />}
                        title="No sessions match"
                        description={
                          hasActiveFilters
                            ? "Try a different source, project, or query."
                            : "Nothing matches your current view."
                        }
                        action={
                          hasActiveFilters
                            ? { label: "Clear filters", onClick: clearFilters }
                            : undefined
                        }
                      />
                    )}
                    {(!loading || sessionItems.length > 0) &&
                      filtered.map((session) => {
                        const hit = contentHits.get(sessionKey(session));
                        const showSnippet =
                          !!hit && query.trim().toLowerCase() === contentQuery.toLowerCase();
                        const isActive =
                          selected?.path === session.path && selected?.id === session.id;
                        return (
                          <button
                            key={sessionKey(session)}
                            onClick={() => setSelected(session)}
                            title={session.snippet || undefined}
                            className={cn(
                              "w-full text-left rounded-md border px-3 py-2 transition-colors flex flex-col gap-1",
                              isActive
                                ? "border-primary bg-primary/5"
                                : "border-border bg-card hover:bg-accent/40"
                            )}
                          >
                            <div className="flex items-baseline justify-between gap-2">
                              <h2 className="text-sm font-medium leading-snug line-clamp-2 flex-1 min-w-0">
                                {session.title}
                              </h2>
                              <span
                                className="text-xs text-muted-foreground shrink-0"
                                title={formatDate(session.updated_at ?? session.started_at)}
                              >
                                {formatRelativeDate(session.updated_at ?? session.started_at)}
                              </span>
                            </div>
                            <div className="flex items-center justify-between gap-2 text-xs text-muted-foreground">
                              <span
                                className="flex items-center gap-1 min-w-0"
                                title={session.project}
                              >
                                <Folder size={12} className="shrink-0" />
                                <span className="truncate">
                                  {projectDisplayName(session.project)}
                                </span>
                              </span>
                              <span className="flex items-center gap-2 shrink-0">
                                <span
                                  className="flex items-center gap-1"
                                  title={`${session.message_count} messages`}
                                >
                                  <MessageSquare size={11} />
                                  <span className="tabular-nums">{session.message_count}</span>
                                </span>
                                {source === "All" && (
                                  <span
                                    title={sourceDisplayName(session.source)}
                                    aria-label={sourceDisplayName(session.source)}
                                  >
                                    <BrandIcon source={session.source} />
                                  </span>
                                )}
                              </span>
                            </div>
                            {showSnippet && hit && (
                              <SnippetLine
                                snippet={hit.snippet}
                                query={query.trim()}
                                role={hit.role}
                                matchCount={hit.match_count}
                              />
                            )}
                          </button>
                        );
                      })}
                  </div>
                )}

                {pane === "memory" && (
                  <div className="flex-1 min-h-0 overflow-auto px-3 py-2 flex flex-col gap-1.5">
                    {loading && memoryItems.length === 0 && (
                      <EmptyState
                        icon={<Loader2 className="animate-spin" />}
                        title="Scanning memory"
                      />
                    )}
                    {!loading && filteredMemories.length === 0 && memoryItems.length === 0 && (
                      <EmptyState
                        icon={<BookOpen size={32} />}
                        title="No memory files yet"
                        description="Termory looks for AGENTS.md, CLAUDE.md, GEMINI.md, and per-project memory folders in the current working directory and your home folder."
                      />
                    )}
                    {!loading && filteredMemories.length === 0 && memoryItems.length > 0 && (
                      <EmptyState
                        icon={<BookOpen size={32} />}
                        title="No memory matches"
                        description={
                          hasActiveFilters
                            ? "Try a different source or query."
                            : "Nothing matches your current view."
                        }
                        action={
                          hasActiveFilters
                            ? { label: "Clear filters", onClick: clearFilters }
                            : undefined
                        }
                      />
                    )}
                    {filteredMemories.map((item) => (
                      <MemoryCard
                        key={sessionKey(item)}
                        item={item}
                        selected={selected}
                        onClick={() => setSelected(item)}
                        query={query.trim()}
                        contentQuery={contentQuery}
                        hit={contentHits.get(sessionKey(item))}
                        showSource={source === "All"}
                      />
                    ))}
                  </div>
                )}

                {pane === "skills" && (
                  <div className="flex-1 min-h-0 overflow-auto px-3 py-2 flex flex-col gap-1.5">
                    {loading && skillItems.length === 0 && (
                      <EmptyState
                        icon={<Loader2 className="animate-spin" />}
                        title="Scanning skills"
                      />
                    )}
                    {!loading && filteredSkills.length === 0 && skillItems.length === 0 && (
                      <EmptyState
                        icon={<Sparkles size={32} />}
                        title="No skills yet"
                        description="Termory scans ~/.claude/skills, ~/.codex/skills, ~/.gemini/skills, and ~/.agents/skills, plus project-local .agents/skills folders."
                      />
                    )}
                    {!loading && filteredSkills.length === 0 && skillItems.length > 0 && (
                      <EmptyState
                        icon={<Sparkles size={32} />}
                        title="No skill matches"
                        description={
                          hasActiveFilters
                            ? "Try a different source or query."
                            : "Nothing matches your current view."
                        }
                        action={
                          hasActiveFilters
                            ? { label: "Clear filters", onClick: clearFilters }
                            : undefined
                        }
                      />
                    )}
                    {filteredSkills.map((item) => (
                      <MemoryCard
                        key={sessionKey(item)}
                        item={item}
                        selected={selected}
                        onClick={() => setSelected(item)}
                        query={query.trim()}
                        contentQuery={contentQuery}
                        hit={contentHits.get(sessionKey(item))}
                        showSource={source === "All"}
                      />
                    ))}
                  </div>
                )}
              </section>

              <section className="flex flex-col min-h-0 bg-background">
                {!selected && sessions.length === 0 && !loading && (
                  <EmptyState
                    icon={<Sparkles size={32} />}
                    title="Nothing to view yet"
                    description="Once Termory finds local history, sessions, memories, and skills will show up here."
                  />
                )}
                {!selected && sessions.length > 0 && (
                  <EmptyState icon={<Sparkles />} title="Select a record" />
                )}
                {!selected && sessions.length === 0 && loading && (
                  <EmptyState
                    icon={<Loader2 className="animate-spin" />}
                    title="Scanning…"
                  />
                )}
                {selected && (
                  <>
                    <header className="flex flex-col gap-2 px-5 py-3 bg-card border-b border-border">
                      <h2
                        className="text-base font-semibold leading-snug"
                        title={selected.title}
                      >
                        {selected.title || "(untitled)"}
                      </h2>

                      <div className="flex items-center gap-2 text-xs text-muted-foreground flex-wrap">
                        <span
                          className="inline-flex items-center gap-1"
                          title={selected.updated_at ?? selected.started_at ?? ""}
                        >
                          <Clock size={13} />
                          {formatDate(selected.updated_at ?? selected.started_at)}
                        </span>
                        {isSessionItem(selected) && (
                          <>
                            <span className="text-border">·</span>
                            <span
                              className="inline-flex items-center gap-1"
                              title={`${selected.message_count} messages`}
                            >
                              <MessageSquare size={13} />
                              {selected.message_count}
                            </span>
                          </>
                        )}
                        <span className="text-border">·</span>
                        <span
                          className="inline-flex items-center gap-1 min-w-0"
                          title={selected.project}
                        >
                          <Folder size={13} />
                          <span className="truncate">
                            {projectDisplayName(selected.project)}
                          </span>
                        </span>
                      </div>

                      <div className="flex items-center justify-between gap-2">
                        <div
                          className="inline-flex items-center gap-1.5 min-w-0 text-xs font-mono text-muted-foreground"
                          title={selected.path}
                        >
                          <File size={13} className="shrink-0" />
                          <span className="truncate">{selected.path}</span>
                        </div>
                        <div className="inline-flex items-center gap-1 shrink-0">
                          <Button
                            variant="ghost"
                            size="icon"
                            className="size-7"
                            onClick={() => revealItemInDir(selected.path)}
                            title="Open in Finder"
                            aria-label="Open in Finder"
                          >
                            <ExternalLink size={14} />
                          </Button>
                          <CopyMenu
                            items={[
                              ...(isSessionItem(selected) &&
                              resumeCommandFor(selected.source, selected.id)
                                ? [
                                    {
                                      label: "Copy resume command",
                                      value: resumeCommandFor(selected.source, selected.id)!
                                    }
                                  ]
                                : []),
                              { label: "Copy path", value: selected.path },
                              { label: "Copy filename", value: basename(selected.path) },
                              ...(isSessionItem(selected)
                                ? [{ label: "Copy ID", value: selected.id }]
                                : [])
                            ]}
                          />
                        </div>
                      </div>
                    </header>

                    <div className="flex-1 flex flex-col gap-5 overflow-auto p-4">
                      {detailLoading && (
                        <div className="flex items-center justify-center min-h-[120px] text-muted-foreground">
                          <Loader2 className="animate-spin" />
                        </div>
                      )}
                      {!detailLoading &&
                        isSessionItem(selected) &&
                        detail?.messages.map((message, index) => (
                          <article
                            key={`${message.timestamp ?? "msg"}:${index}`}
                            data-role={roleClass(message.role)}
                          >
                            <header className="flex items-center gap-2 mb-1">
                              <span
                                aria-hidden="true"
                                data-role={roleClass(message.role)}
                                className="w-[3px] h-[0.95em] rounded-sm shrink-0 bg-muted-foreground/60 data-[role=user]:bg-teal-500 data-[role=assistant]:bg-blue-400 data-[role=tool]:bg-amber-500"
                              />
                              <span className="text-xs font-medium text-muted-foreground lowercase tabular-nums">
                                {message.role || "event"}
                              </span>
                            </header>
                            <MessageBody text={message.text} className="pl-[11px]" />
                          </article>
                        ))}
                      {!detailLoading &&
                      !isSessionItem(selected) &&
                      detail?.messages.length ? (
                        <div className="rounded-lg border border-border bg-card text-card-foreground px-5 py-4">
                          <MessageBody
                            text={detail.messages.map((m) => m.text).join("\n\n")}
                          />
                        </div>
                      ) : null}
                    </div>
                  </>
                )}
              </section>
            </main>
          )}
        </div>
      </div>
      <FreshnessFooter
        syncing={loading}
        lastSyncedAt={lastRefreshedAt}
        error={error}
      />
      <CommandPalette
        sessions={sessions}
        onOpenItem={openItem}
        recentSearches={recentSearches}
        onCommitSearch={addRecentSearch}
        onClearRecent={clearRecentSearches}
      />
    </div>
  );
}
