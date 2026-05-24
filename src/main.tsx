import React from "react";
import ReactDOM from "react-dom/client";
import { invoke } from "@tauri-apps/api/core";
import { listen } from "@tauri-apps/api/event";
import { revealItemInDir } from "@tauri-apps/plugin-opener";
import {
  AlertTriangle,
  BarChart3,
  Bot,
  BookOpen,
  Check,
  ChevronDown,
  ChevronRight,
  Clock,
  Copy,
  Eye,
  EyeOff,
  File,
  Folder,
  ExternalLink,
  FileJson,
  History,
  Loader2,
  MessageSquare,
  Plug,
  RefreshCw,
  Search,
  Settings as SettingsIcon,
  Sparkles
} from "lucide-react";
import ReactMarkdown from "react-markdown";
import remarkGfm from "remark-gfm";
import { getConfig, setConfig } from "./config";
import "./styles.css";

/// React state that mirrors a key in the tauri-plugin-store backing
/// file. Behavior:
/// * mount → returns `initial` immediately; kicks off an async load
/// * load resolves → if a value was persisted, swaps in (without
///   re-persisting it)
/// * any later setValue → writes through to the store
///
/// `validate` lets callers reject corrupt persisted data (e.g. an
/// out-of-range enum) and fall back to `initial`. Returns the same
/// `[value, setValue]` tuple as `React.useState` so callers can
/// drop-in replace.
function usePersistentState<T>(
  key: string,
  initial: T,
  validate?: (raw: unknown) => raw is T
): [T, React.Dispatch<React.SetStateAction<T>>] {
  const [value, setValue] = React.useState<T>(initial);
  const loaded = React.useRef(false);
  const skipNextPersist = React.useRef(false);

  React.useEffect(() => {
    let cancelled = false;
    getConfig<unknown>(key)
      .then((stored) => {
        if (cancelled) return;
        if (stored == null) return;
        if (validate && !validate(stored)) return;
        skipNextPersist.current = true;
        setValue(stored as T);
      })
      .catch(() => {})
      .finally(() => {
        if (!cancelled) loaded.current = true;
      });
    return () => {
      cancelled = true;
    };
  }, [key]);

  React.useEffect(() => {
    if (!loaded.current) return;
    if (skipNextPersist.current) {
      skipNextPersist.current = false;
      return;
    }
    void setConfig(key, value);
  }, [key, value]);

  return [value, setValue];
}

const messageRemarkPlugins = [remarkGfm];

const MessageBody = React.memo(function MessageBody({
  text
}: {
  text: string;
}) {
  return (
    <div className="messageBody">
      <ReactMarkdown remarkPlugins={messageRemarkPlugins}>
        {text}
      </ReactMarkdown>
    </div>
  );
});

type AppSession = {
  id: string;
  source: string;
  title: string;
  project: string;
  path: string;
  started_at?: string | null;
  updated_at?: string | null;
  message_count: number;
  preview: string;
  message_previews: SessionMessage[];
};

type SessionMessage = {
  role: string;
  text: string;
  timestamp?: string | null;
  kind: string;
};

type SessionDetail = {
  session: AppSession;
  messages: SessionMessage[];
};

type SearchHit = {
  session: AppSession;
  snippet: string;
  role: string;
  match_count: number;
};

function sessionKey(session: { source: string; path: string; id: string }) {
  return `${session.source}:${session.path}:${session.id}`;
}

const sources = ["All", "Codex", "Claude", "Gemini", "OpenCode"];

/// Pretty label for a tool/source identifier. Internal source values
/// stay short ("Claude", "Gemini") so they line up with badge CSS
/// classes (`.badge.claude`, `.badge.gemini`) and the filter logic;
/// the display layer always goes through this helper so Records and
/// Providers show the same official tool name ("Claude Code",
/// "Gemini CLI").
function sourceDisplayName(source: string): string {
  switch (source) {
    case "Claude":
      return "Claude Code";
    case "Gemini":
      return "Gemini CLI";
    default:
      return source;
  }
}

const MEMORY_SOURCE = "Memory";
const SKILL_SOURCE = "Skill";

type MemoryTool = "Claude" | "Codex" | "Gemini" | "OpenCode" | "Other";

function isMemoryItem(session: AppSession) {
  return session.source === MEMORY_SOURCE;
}

function isSkillItem(session: AppSession) {
  return session.source === SKILL_SOURCE;
}

function isSessionItem(session: AppSession) {
  return !isMemoryItem(session) && !isSkillItem(session);
}

function typeLabelOf(session: AppSession): "Session" | "Memory" | "Skill" {
  if (isMemoryItem(session)) return "Memory";
  if (isSkillItem(session)) return "Skill";
  return "Session";
}

function memoryToolsOf(session: AppSession): MemoryTool[] {
  const set = new Set<MemoryTool>();
  for (const raw of (session.preview ?? "").split(",")) {
    const tag = raw.trim().toLowerCase();
    if (tag === "claude") set.add("Claude");
    else if (tag === "codex") set.add("Codex");
    else if (tag === "gemini") set.add("Gemini");
    else if (tag === "opencode") set.add("OpenCode");
  }
  if (set.size === 0) return ["Other"];
  return MEMORY_TOOL_ORDER.filter((tool) => set.has(tool));
}

const MEMORY_TOOL_ORDER: MemoryTool[] = ["Claude", "Codex", "Gemini", "OpenCode", "Other"];

type CliApp = "claude" | "codex" | "gemini" | "opencode";
const CLI_APPS: CliApp[] = ["claude", "codex", "gemini", "opencode"];

type ProviderKind = "official" | "custom";

type Provider = {
  id: string;
  app: CliApp;
  kind: ProviderKind;
  name: string;
  // All string fields below are optional in storage — config.ts strips
  // ""/null/undefined when writing providers.json, so a freshly-loaded
  // Provider may omit any of them. React inputs that bind to these
  // must use `?? ""` to stay controlled.
  baseUrl?: string;
  apiKey?: string;
  model?: string;
  // Claude-only: per-size routing. When set, Claude Code's `/model`
  // menu (Sonnet/Opus/Haiku) maps to these model ids instead of the
  // Anthropic-native ones — matters when the provider doesn't speak
  // Anthropic model id (e.g. routes Claude requests to gpt-5).
  claudeHaikuModel?: string;
  claudeSonnetModel?: string;
  claudeOpusModel?: string;
};

type ActiveKind = "official" | "custom" | "unmanaged";

type LiveSnapshot = {
  baseUrl?: string | null;
  apiKeyMasked?: string | null;
  model?: string | null;
};

type ActiveState = {
  app: CliApp;
  kind: ActiveKind;
  matchedProviderId?: string | null;
  liveSnapshot?: LiveSnapshot | null;
  livePath: string;
};

type TestResult = {
  ok: boolean;
  status?: number | null;
  latencyMs: number;
  message: string;
};

function isProviderList(raw: unknown): raw is Provider[] {
  if (!Array.isArray(raw)) return false;
  for (const item of raw) {
    if (!item || typeof item !== "object") return false;
    const p = item as Record<string, unknown>;
    if (typeof p.id !== "string") return false;
    if (typeof p.name !== "string") return false;
    if (p.app !== "claude" && p.app !== "codex" && p.app !== "gemini" && p.app !== "opencode") {
      return false;
    }
    if (p.kind !== "official" && p.kind !== "custom") return false;
  }
  return true;
}

type Route = "records" | "search" | "stats" | "config" | "settings";
const ROUTES: Route[] = ["records", "search", "stats", "config", "settings"];

function isRoute(value: string): value is Route {
  return (ROUTES as string[]).includes(value);
}

function readRouteFromHash(): Route {
  const raw = window.location.hash.replace(/^#/, "");
  return isRoute(raw) ? raw : "records";
}

function App() {
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
  const [searchingContent, setSearchingContent] = React.useState(false);
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
  const openItem = React.useCallback((item: AppSession) => {
    setSource("All");
    setProject(null);
    if (isMemoryItem(item)) setPane("memory");
    else if (isSkillItem(item)) setPane("skills");
    else setPane("sessions");
    setSelected(item);
    setRoute("records");
  }, []);

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
        ) ?? result[0] ?? null
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

  // Subscribe to background filesystem-watcher pushes. The Rust side
  // detects changes in any source dir, debounces, runs a fresh
  // `scan_sessions`, and emits the result here. We swap state in
  // silently (no "Syncing…" middle state — these events aren't
  // user-initiated, so the just-synced pulse alone is enough feedback).
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
  const memoryItems = React.useMemo(
    () => sessions.filter(isMemoryItem),
    [sessions]
  );
  const skillItems = React.useMemo(
    () => sessions.filter(isSkillItem),
    [sessions]
  );

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
    return sources.map((item) => {
      const sourceSessions = item === "All" ? sessionItems : sessionItems.filter((session) => session.source === item);
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
    <div className="appShell">
      <div className="appBody">
      <ActivityRail route={route} onChange={setRoute} />
      <div className="routeContent">
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
    <main className="app">
      <aside className="sidebar">
        <div className="sourceList">
          <div className="sourceListContent">
            {sourceGroups.map((group) => (
              <div key={group.source} className="sourceGroup">
              <button
                className={source === group.source && !project ? "sourceButton active" : "sourceButton"}
                aria-expanded={group.projects.length > 0 ? expandedSources.has(group.source) : undefined}
                onClick={() => {
                  setSource(group.source);
                  setProject(null);
                }}
              >
                <span className="sourceLabel">
                  <span
                    className={group.projects.length > 0 ? "sourceIconSlot hasProjects" : "sourceIconSlot"}
                    role={group.projects.length > 0 ? "button" : undefined}
                    aria-label={group.projects.length > 0 ? `${expandedSources.has(group.source) ? "Collapse" : "Expand"} ${group.source} projects` : undefined}
                    tabIndex={group.projects.length > 0 ? 0 : undefined}
                    onClick={(event) => {
                      if (group.projects.length === 0) return;
                      event.stopPropagation();
                      setExpandedSources((current) => toggleSetValue(current, group.source));
                    }}
                    onKeyDown={(event) => {
                      if (group.projects.length === 0) return;
                      if (event.key !== "Enter" && event.key !== " ") return;
                      event.preventDefault();
                      event.stopPropagation();
                      setExpandedSources((current) => toggleSetValue(current, group.source));
                    }}
                  >
                    <BrandIcon source={group.source} />
                    {expandedSources.has(group.source) ? (
                      <ChevronDown className="toggleIcon" size={15} />
                    ) : (
                      <ChevronRight className="toggleIcon" size={15} />
                    )}
                  </span>
                  <span>{sourceDisplayName(group.source)}</span>
                </span>
                <b>{group.count}</b>
              </button>

              {group.projects.length > 0 && expandedSources.has(group.source) && (
                <div className="projectList">
                  {group.projects.map(([projectName, count]) => (
                    <button
                      key={`${group.source}:${projectName}`}
                      className={source === group.source && project === projectName ? "projectButton active" : "projectButton"}
                      title={`${projectName} - ${formatFullNumber(count)} Sessions`}
                      onClick={() => {
                        setSource(group.source);
                        setProject(projectName);
                        setExpandedSources((current) => addSetValue(current, group.source));
                      }}
                    >
                      <span className="projectLabel">
                        <Folder size={12} />
                        <span>{projectDisplayName(projectName)}</span>
                      </span>
                      <b>{count}</b>
                    </button>
                  ))}
                </div>
              )}
              </div>
            ))}
          </div>
        </div>

      </aside>

      <section className="sessionsPane">

        <div className="paneTabs" role="tablist">
          <button
            type="button"
            role="tab"
            aria-selected={pane === "sessions"}
            className={pane === "sessions" ? "paneTab active" : "paneTab"}
            onClick={() => setPane("sessions")}
          >
            Sessions <b>{filtered.length}</b>
          </button>
          <button
            type="button"
            role="tab"
            aria-selected={pane === "memory"}
            className={pane === "memory" ? "paneTab active" : "paneTab"}
            onClick={() => setPane("memory")}
          >
            Memories <b>{filteredMemories.length}</b>
          </button>
          <button
            type="button"
            role="tab"
            aria-selected={pane === "skills"}
            className={pane === "skills" ? "paneTab active" : "paneTab"}
            onClick={() => setPane("skills")}
          >
            Skills <b>{filteredSkills.length}</b>
          </button>
        </div>

        {error && <div className="error">{error}</div>}

        {pane === "sessions" && (
          <div className="sessionList">
            {loading && sessionItems.length === 0 && <EmptyState icon={<Loader2 className="spin" />} title="Scanning local history" />}
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
                  hasActiveFilters ? { label: "Clear filters", onClick: clearFilters } : undefined
                }
              />
            )}
            {(!loading || sessionItems.length > 0) &&
              filtered.map((session) => {
                const hit = contentHits.get(sessionKey(session));
                const showSnippet =
                  !!hit && query.trim().toLowerCase() === contentQuery.toLowerCase();
                return (
                  <button
                    key={sessionKey(session)}
                    className={selected?.path === session.path && selected?.id === session.id ? "sessionCard active" : "sessionCard"}
                    onClick={() => setSelected(session)}
                  >
                    <div className="sessionHeader">
                      <span className={`badge ${session.source.toLowerCase()}`}>{sourceDisplayName(session.source)}</span>
                      <span className="date">{formatDate(session.updated_at ?? session.started_at)}</span>
                    </div>
                    <h2>{session.title}</h2>
                    <div className="sessionMeta">
                      <span className="sessionMetaProject" title={session.project}>
                        <Folder size={12} />
                        <span>{projectDisplayName(session.project)}</span>
                      </span>
                      <span>{session.message_count} messages</span>
                    </div>
                    {showSnippet && (
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
          <div className="sessionList memoryList">
            {loading && memoryItems.length === 0 && (
              <EmptyState icon={<Loader2 className="spin" />} title="Scanning memory" />
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
                  hasActiveFilters ? { label: "Clear filters", onClick: clearFilters } : undefined
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
              />
            ))}
          </div>
        )}

        {pane === "skills" && (
          <div className="sessionList memoryList">
            {loading && skillItems.length === 0 && (
              <EmptyState icon={<Loader2 className="spin" />} title="Scanning skills" />
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
                  hasActiveFilters ? { label: "Clear filters", onClick: clearFilters } : undefined
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
              />
            ))}
          </div>
        )}
      </section>

      <section className="detailPane">
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
          <EmptyState icon={<Loader2 className="spin" />} title="Scanning…" />
        )}
        {selected && (
          <>
            <header className="detailHeader">
              <h2 className="detailTitle" title={selected.title}>{selected.title || "(untitled)"}</h2>

              <div className="detailMeta">
                <span className="detailMetaItem" title={selected.updated_at ?? selected.started_at ?? ""}>
                  <Clock size={13} />
                  {formatDate(selected.updated_at ?? selected.started_at)}
                </span>
                {isSessionItem(selected) && (
                  <>
                    <span className="detailMetaSep">·</span>
                    <span className="detailMetaItem" title={`${selected.message_count} messages`}>
                      <MessageSquare size={13} />
                      {selected.message_count}
                    </span>
                  </>
                )}
                <span className="detailMetaSep">·</span>
                <span className="detailMetaItem detailMetaProject" title={selected.project}>
                  <Folder size={13} />
                  <span>{projectDisplayName(selected.project)}</span>
                </span>
              </div>

              <div className="detailFileRow">
                <div className="detailPath" title={selected.path}>
                  <File size={13} />
                  <span>{selected.path}</span>
                </div>
                <div className="detailActions">
                  <button
                    className="bareIcon"
                    onClick={() => revealItemInDir(selected.path)}
                    title="Open in Finder"
                    aria-label="Open in Finder"
                  >
                    <ExternalLink size={14} />
                  </button>
                  <CopyMenu
                    items={[
                      ...(isSessionItem(selected) && resumeCommandFor(selected.source, selected.id)
                        ? [{
                            label: "Copy resume command",
                            value: resumeCommandFor(selected.source, selected.id)!
                          }]
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

            <div className="messageList">
              {detailLoading && (
                <div className="emptyState">
                  <Loader2 className="spin" />
                </div>
              )}
              {!detailLoading && isSessionItem(selected) &&
                detail?.messages.map((message, index) => (
                  <article key={`${message.timestamp ?? "msg"}:${index}`} className={`message ${roleClass(message.role)}`}>
                    <div className="messageTop">
                      <span className="messageRole">{message.role || "event"}</span>
                      <time>{formatDate(message.timestamp)}</time>
                    </div>
                    <MessageBody text={message.text} />
                  </article>
                ))}
              {!detailLoading && !isSessionItem(selected) && detail?.messages.length ? (
                // Memory / Skill files are single-document .md previews;
                // drop the message-card chrome (role label, timestamp,
                // padded wrapper) and render the body as a single
                // continuous document.
                <div className="docBody">
                  <MessageBody text={detail.messages.map((m) => m.text).join("\n\n")} />
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

function ActivityRail({
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
    <nav className="activityRail" aria-label="Primary">
      {items.map((item) => (
        <button
          key={item.id}
          type="button"
          className={route === item.id ? "railItem active" : "railItem"}
          onClick={() => onChange(item.id)}
          title={item.label}
          aria-label={item.label}
          aria-current={route === item.id ? "page" : undefined}
        >
          {item.icon}
        </button>
      ))}
    </nav>
  );
}

function FreshnessFooter({
  syncing,
  lastSyncedAt,
  error
}: {
  syncing: boolean;
  lastSyncedAt: number | null;
  error: string | null;
}) {
  // Bump every 30s so "Synced 2m ago" stays accurate without
  // re-rendering the rest of the app. tick is intentionally unused —
  // its only job is to invalidate the rendered label.
  const [, setTick] = React.useState(0);
  React.useEffect(() => {
    const id = window.setInterval(() => setTick((t) => t + 1), 30_000);
    return () => window.clearInterval(id);
  }, []);

  // Brief "just synced" pulse after a successful sync — gives the user
  // a passive cue that the background actually did something. After
  // ~1.8s the footer falls back to the idle "Synced 2m ago" state.
  // Triggers on any `lastSyncedAt` advance, so both launch-time scans
  // and watcher-driven re-scans get the cue.
  const justSyncedWindow = 1800;
  const [justSynced, setJustSynced] = React.useState(false);
  const prevSyncedAt = React.useRef(lastSyncedAt);
  React.useEffect(() => {
    if (
      lastSyncedAt != null &&
      prevSyncedAt.current !== lastSyncedAt &&
      !error
    ) {
      setJustSynced(true);
      const timer = window.setTimeout(() => setJustSynced(false), justSyncedWindow);
      prevSyncedAt.current = lastSyncedAt;
      return () => window.clearTimeout(timer);
    }
    prevSyncedAt.current = lastSyncedAt;
  }, [lastSyncedAt, error]);

  // State machine — chooses icon + label + className. error > syncing
  // > justSynced > idle.
  let state: "idle" | "syncing" | "done" | "error" = "idle";
  let icon: React.ReactNode = null;
  let label = "";
  let tooltip: string | undefined;
  if (error) {
    state = "error";
    icon = <AlertTriangle size={12} strokeWidth={2.25} />;
    label = "Sync failed";
    tooltip = error;
  } else if (syncing) {
    state = "syncing";
    icon = <RefreshCw size={12} strokeWidth={2.25} className="spin" />;
    label = "Syncing…";
  } else if (justSynced) {
    state = "done";
    icon = <Check size={12} strokeWidth={2.25} />;
    label = "Synced just now";
  } else if (lastSyncedAt != null) {
    state = "idle";
    icon = <Check size={12} strokeWidth={2.25} />;
    label = `Synced ${formatTimeAgo(lastSyncedAt)}`;
    tooltip = new Date(lastSyncedAt).toLocaleString();
  }

  return (
    <footer
      className={`freshnessFooter freshnessFooter-${state}`}
      aria-label={label || "Freshness status"}
      title={tooltip}
    >
      <span className="freshnessGlyph">{icon}</span>
      <span className="freshnessLabel">{label}</span>
    </footer>
  );
}

function CopyMenu({ items }: { items: { label: string; value: string }[] }) {
  const [open, setOpen] = React.useState(false);
  const [copied, setCopied] = React.useState<string | null>(null);
  const wrapperRef = React.useRef<HTMLDivElement>(null);

  // Close on outside click. Bound only while the menu is open so we
  // don't pay listener overhead the rest of the time.
  React.useEffect(() => {
    if (!open) return;
    const onDown = (event: MouseEvent) => {
      if (wrapperRef.current && !wrapperRef.current.contains(event.target as Node)) {
        setOpen(false);
      }
    };
    document.addEventListener("mousedown", onDown);
    return () => document.removeEventListener("mousedown", onDown);
  }, [open]);

  const handleCopy = async (label: string, value: string) => {
    await copyToClipboard(value);
    setCopied(label);
    window.setTimeout(() => setCopied(null), 1200);
    setOpen(false);
  };

  return (
    <div className="copyMenu" ref={wrapperRef}>
      <button
        className="bareIcon copyMenuTrigger"
        onClick={() => setOpen((value) => !value)}
        title="Copy…"
        aria-label="Copy…"
        aria-haspopup="menu"
        aria-expanded={open}
      >
        <Copy size={14} />
      </button>
      {open && (
        <div className="copyMenuList" role="menu">
          {items.map((item) => (
            <button
              key={item.label}
              className="copyMenuItem"
              role="menuitem"
              onClick={() => void handleCopy(item.label, item.value)}
            >
              <span>{item.label}</span>
              {copied === item.label && <Check size={12} className="copyMenuCheck" />}
            </button>
          ))}
        </div>
      )}
    </div>
  );
}

/// Render a session's messages, weaving in a thin time separator
/// whenever the timestamp jumps more than `TIME_GAP_MS` from the prior
/// message (plus an anchor separator before the very first message).
/// Per-message timestamps + role labels are intentionally not drawn —
/// role is encoded via a left color stripe in CSS, and dense streams
/// would be noisy with one timestamp per row.
const TIME_GAP_MS = 5 * 60 * 1000;

function renderMessages(messages: SessionMessage[]): React.ReactNode {
  const nodes: React.ReactNode[] = [];
  for (let i = 0; i < messages.length; i++) {
    const message = messages[i];
    const prev = i > 0 ? messages[i - 1] : null;
    if (shouldInsertTimeSeparator(prev, message)) {
      nodes.push(
        <TimeSeparator
          key={`sep-${i}`}
          timestamp={message.timestamp ?? undefined}
        />
      );
    }
    nodes.push(
      <article
        key={`msg-${i}`}
        className={`message ${roleClass(message.role)}`}
      >
        <MessageBody text={message.text} />
      </article>
    );
  }
  return nodes;
}

function shouldInsertTimeSeparator(
  prev: SessionMessage | null,
  current: SessionMessage
): boolean {
  if (!prev) {
    // Anchor the start of the conversation when the first message has
    // a timestamp; skip otherwise (rare, e.g. records with no time).
    return !!current.timestamp;
  }
  if (prev.timestamp && current.timestamp) {
    const gap =
      new Date(current.timestamp).getTime() - new Date(prev.timestamp).getTime();
    if (gap > TIME_GAP_MS) return true;
  }
  return false;
}

function TimeSeparator({ timestamp }: { timestamp?: string }) {
  const label = React.useMemo(() => {
    if (!timestamp) return "";
    const date = new Date(timestamp);
    if (isNaN(date.getTime())) return "";
    // HH:MM in the user's locale — short, parses at a glance.
    return date.toLocaleTimeString([], { hour: "2-digit", minute: "2-digit" });
  }, [timestamp]);
  return (
    <div className="timeSeparator" aria-hidden={!label} title={timestamp}>
      <span>{label}</span>
    </div>
  );
}

/// Per-platform "resume this session" shell command. Returns `null`
/// for sources whose CLI doesn't expose a direct resume-by-id flag.
///
/// Verified shapes:
///   * Claude  → `claude --resume <id>`  (per user spec)
///   * Codex   → `codex resume <id>`     (codex-rs/exec/src/cli.rs:174-181 ResumeArgsRaw,
///                                        SESSION_ID positional)
///
/// Skipped:
///   * Gemini   — `/resume` is an in-TUI slash command; there's no CLI
///     flag to resume a specific sessionId from shell.
///   * OpenCode — `--session` is a `run` subcommand option; the exact
///     external invocation depends on flags we don't know here.
function resumeCommandFor(source: string, id: string): string | null {
  switch (source) {
    case "Claude":
      return `claude --resume ${id}`;
    case "Codex":
      return `codex resume ${id}`;
    default:
      return null;
  }
}

function basename(path: string): string {
  // Cross-platform basename: split on / or \, drop trailing empty
  // segments, return the last piece. Falls back to the whole path if
  // nothing splits.
  const parts = path.split(/[\\/]+/).filter(Boolean);
  return parts.length > 0 ? parts[parts.length - 1] : path;
}

async function copyToClipboard(text: string): Promise<void> {
  // navigator.clipboard is available in the Tauri webview; failure
  // is silently swallowed since there's no useful recovery (clipboard
  // permissions in webviews don't get prompted, they're granted).
  try {
    await navigator.clipboard.writeText(text);
  } catch (err) {
    console.warn("clipboard write failed", err);
  }
}

function formatTimeAgo(timestamp: number): string {
  const sec = Math.floor((Date.now() - timestamp) / 1000);
  if (sec < 5) return "just now";
  if (sec < 60) return `${sec}s ago`;
  const min = Math.floor(sec / 60);
  if (min < 60) return `${min}m ago`;
  const hr = Math.floor(min / 60);
  if (hr < 24) return `${hr}h ago`;
  const day = Math.floor(hr / 24);
  return `${day}d ago`;
}

const CLI_APP_LABEL: Record<CliApp, string> = {
  claude: "Claude Code",
  codex: "Codex",
  gemini: "Gemini CLI",
  opencode: "OpenCode"
};

const CLI_APP_SOURCE_BADGE: Record<CliApp, string> = {
  claude: "Claude",
  codex: "Codex",
  gemini: "Gemini",
  opencode: "OpenCode"
};

function newProviderId(): string {
  if (typeof crypto !== "undefined" && "randomUUID" in crypto) return crypto.randomUUID();
  return `${Date.now().toString(36)}-${Math.random().toString(36).slice(2, 10)}`;
}

function blankProvider(app: CliApp): Provider {
  const base: Provider = {
    id: newProviderId(),
    app,
    kind: "custom",
    name: "",
    baseUrl: "",
    apiKey: "",
    model: ""
  };
  if (app === "claude") {
    base.baseUrl = "https://api.anthropic.com";
  } else if (app === "codex") {
    base.baseUrl = "https://api.openai.com/v1";
  } else if (app === "gemini") {
    base.baseUrl = "https://generativelanguage.googleapis.com";
  } else if (app === "opencode") {
    base.baseUrl = "https://api.anthropic.com";
  }
  return base;
}

function maskKey(key: string): string {
  if (!key) return "";
  if (key.length <= 8) return "•".repeat(key.length);
  return `${key.slice(0, 4)}${"•".repeat(key.length - 8)}${key.slice(-4)}`;
}

const ACTIVE_STATE_REFRESH_EVENT = "termory:providers-refresh";

function ProvidersPage({
  providers,
  setProviders,
  app,
  setApp
}: {
  providers: Provider[];
  setProviders: React.Dispatch<React.SetStateAction<Provider[]>>;
  app: CliApp;
  setApp: (next: CliApp) => void;
}) {
  const [editing, setEditing] = React.useState<Provider | null>(null);
  const [editingIsNew, setEditingIsNew] = React.useState(false);
  const [activeStates, setActiveStates] = React.useState<Record<CliApp, ActiveState | null>>({
    claude: null,
    codex: null,
    gemini: null,
    opencode: null
  });
  const [activating, setActivating] = React.useState<string | null>(null);
  const [testing, setTesting] = React.useState<string | null>(null);
  const [testResults, setTestResults] = React.useState<Record<string, TestResult>>({});
  const [feedback, setFeedback] = React.useState<{
    kind: "ok" | "error";
    message: string;
  } | null>(null);

  const refreshActive = React.useCallback(async () => {
    try {
      const states = await invoke<ActiveState[]>("provider_active_states", { providers });
      const next: Record<CliApp, ActiveState | null> = {
        claude: null,
        codex: null,
        gemini: null,
        opencode: null
      };
      for (const s of states) next[s.app] = s;
      setActiveStates(next);
    } catch (err) {
      setFeedback({ kind: "error", message: `read live state failed: ${String(err)}` });
    }
  }, [providers]);

  React.useEffect(() => {
    void refreshActive();
  }, [refreshActive]);

  // Auto refresh when the Rust watcher detects any change in the
  // CLI dirs (live config files live inside those dirs). Reuse the
  // existing `termory:sources-changed` event — payload is ignored.
  React.useEffect(() => {
    const unlistenPromise = listen("termory:sources-changed", () => {
      void refreshActive();
    });
    const peerHandler = () => void refreshActive();
    window.addEventListener(ACTIVE_STATE_REFRESH_EVENT, peerHandler);
    return () => {
      void unlistenPromise.then((fn) => fn()).catch(() => {});
      window.removeEventListener(ACTIVE_STATE_REFRESH_EVENT, peerHandler);
    };
  }, [refreshActive]);

  // Re-fetch when the user switches tabs (cheap; backend reads 4 files).
  React.useEffect(() => {
    setFeedback(null);
  }, [app]);

  const providersForApp = React.useMemo(
    () => providers.filter((p) => p.app === app),
    [providers, app]
  );
  const customProviders = React.useMemo(
    () => providersForApp.filter((p) => p.kind === "custom"),
    [providersForApp]
  );
  const activeState = activeStates[app];

  const startNew = () => {
    setEditing(blankProvider(app));
    setEditingIsNew(true);
  };
  const startEdit = (p: Provider) => {
    setEditing({ ...p });
    setEditingIsNew(false);
  };
  const closeEditor = () => {
    setEditing(null);
    setEditingIsNew(false);
  };

  const saveProvider = (next: Provider) => {
    setProviders((cur) => {
      const idx = cur.findIndex((p) => p.id === next.id);
      if (idx === -1) return [...cur, next];
      const copy = cur.slice();
      copy[idx] = next;
      return copy;
    });
    closeEditor();
  };

  const deleteProvider = (id: string) => {
    if (!window.confirm("Delete this provider?")) return;
    setProviders((cur) => cur.filter((p) => p.id !== id));
  };

  const activateOne = async (target: Provider) => {
    setActivating(target.id);
    setFeedback(null);
    try {
      await invoke("activate_provider", {
        provider: target,
        providersForApp
      });
      setFeedback({
        kind: "ok",
        message: `Activated ${target.name || "(unnamed)"}.`
      });
      await refreshActive();
    } catch (err) {
      setFeedback({ kind: "error", message: String(err) });
    } finally {
      setActivating(null);
    }
  };

  const activateOfficial = async () => {
    setActivating("__official__");
    setFeedback(null);
    try {
      await invoke("deactivate_provider", {
        app,
        providersForApp
      });
      setFeedback({ kind: "ok", message: `Restored ${CLI_APP_LABEL[app]} to native login.` });
      await refreshActive();
    } catch (err) {
      setFeedback({ kind: "error", message: String(err) });
    } finally {
      setActivating(null);
    }
  };

  const testOne = async (target: Provider) => {
    setTesting(target.id);
    try {
      const result = await invoke<TestResult>("test_provider_api", { provider: target });
      setTestResults((cur) => ({ ...cur, [target.id]: result }));
    } catch (err) {
      setTestResults((cur) => ({
        ...cur,
        [target.id]: {
          ok: false,
          status: null,
          latencyMs: 0,
          message: String(err)
        }
      }));
    } finally {
      setTesting(null);
    }
  };

  return (
    <div className="providersPage">
      <div className="providersTabs" role="tablist">
        {CLI_APPS.map((id) => (
          <button
            key={id}
            type="button"
            role="tab"
            aria-selected={app === id}
            className={app === id ? "providersTab active" : "providersTab"}
            onClick={() => setApp(id)}
          >
            <BrandIcon source={CLI_APP_SOURCE_BADGE[id]} />
            <span>{CLI_APP_LABEL[id]}</span>
          </button>
        ))}
      </div>

      <div className="providersBody">
        {feedback && (
          <div className={`providersFeedback ${feedback.kind}`}>
            {feedback.kind === "ok" ? <Check size={14} /> : <AlertTriangle size={14} />}
            <span>{feedback.message}</span>
          </div>
        )}

        {app !== "opencode" && (
          <section className="providersSection">
            <header className="providersSectionHeader">
              <h3>Official</h3>
              <span className="providersSectionHint">
                Use {CLI_APP_LABEL[app]}'s native login. Activating clears Termory-injected fields.
              </span>
            </header>
            <ProviderOfficialCard
              app={app}
              isActive={activeState?.kind === "official"}
              activating={activating === "__official__"}
              onActivate={() => void activateOfficial()}
            />
          </section>
        )}

        <section className="providersSection">
          <header className="providersSectionHeader">
            <h3>API platforms</h3>
            <span className="providersSectionHint">
              {customProviderHint(app)}
            </span>
            <button type="button" className="providersPrimary" onClick={startNew}>
              + Add provider
            </button>
          </header>

          {customProviders.length === 0 && (
            <EmptyState
              icon={<Plug size={32} />}
              title="No custom providers yet"
              description={`Add a third-party API platform for ${CLI_APP_LABEL[app]} and switch to it with one click.`}
              action={{ label: "+ Add provider", onClick: startNew }}
            />
          )}

          <div className="providersList">
            {customProviders.map((p) => (
              <ProviderCard
                key={p.id}
                provider={p}
                isActive={activeState?.matchedProviderId === p.id}
                activating={activating === p.id}
                testing={testing === p.id}
                testResult={testResults[p.id]}
                onActivate={() => void activateOne(p)}
                onEdit={() => startEdit(p)}
                onDelete={() => deleteProvider(p.id)}
                onTest={() => void testOne(p)}
              />
            ))}
          </div>
        </section>
      </div>

      {editing && (
        <ProviderEditor
          provider={editing}
          isNew={editingIsNew}
          onSave={saveProvider}
          onClose={closeEditor}
        />
      )}
    </div>
  );
}

function ProviderOfficialCard({
  app,
  isActive,
  activating,
  onActivate
}: {
  app: CliApp;
  isActive: boolean;
  activating: boolean;
  onActivate: () => void;
}) {
  const subtitle = {
    claude: "Anthropic OAuth via `claude login`",
    codex: "ChatGPT login via `codex login`",
    gemini: "Google OAuth via `gemini auth`",
    opencode: "Run `/connect` to add an AI provider and start coding"
  }[app];
  const authBadge = {
    claude: "OAuth",
    codex: "OAuth",
    gemini: "OAuth",
    opencode: "API key"
  }[app];
  return (
    <div className={isActive ? "providerCard active" : "providerCard"}>
      <div className="providerCardHeader">
        <div className="providerCardTitle">
          <h3>
            {CLI_APP_LABEL[app]} <span className="providerCardAuthBadge">({authBadge})</span>
          </h3>
          {isActive && <span className="providerActiveBadge">Active</span>}
        </div>
        <button
          type="button"
          className="providersSecondary"
          onClick={onActivate}
          disabled={activating || isActive}
        >
          {activating ? "Activating…" : isActive ? "Active" : "Activate"}
        </button>
      </div>
      <p className="providerCardSubtitle">{subtitle}</p>
    </div>
  );
}

function ProviderCard({
  provider,
  isActive,
  activating,
  testing,
  testResult,
  onActivate,
  onEdit,
  onDelete,
  onTest
}: {
  provider: Provider;
  isActive: boolean;
  activating: boolean;
  testing: boolean;
  testResult: TestResult | undefined;
  onActivate: () => void;
  onEdit: () => void;
  onDelete: () => void;
  onTest: () => void;
}) {
  return (
    <div className={isActive ? "providerCard active" : "providerCard"}>
      <div className="providerCardHeader">
        <div className="providerCardTitle">
          <h3>{provider.name || "(unnamed)"}</h3>
          {isActive && <span className="providerActiveBadge">Active</span>}
        </div>
        <div className="providerCardActions">
          <button
            type="button"
            className="providersSecondary"
            onClick={onActivate}
            disabled={activating}
          >
            {activating ? "Activating…" : isActive ? "Re-apply" : "Activate"}
          </button>
          <button type="button" className="providersGhost" onClick={onTest} disabled={testing}>
            {testing ? "Testing…" : "Test"}
          </button>
          <button type="button" className="providersGhost" onClick={onEdit}>
            Edit
          </button>
          <button type="button" className="providersGhost danger" onClick={onDelete}>
            Delete
          </button>
        </div>
      </div>
      <dl className="providerCardFields">
        {provider.baseUrl && (
          <div>
            <dt>Base URL</dt>
            <dd className="mono">{provider.baseUrl}</dd>
          </div>
        )}
        {provider.apiKey && (
          <div>
            <dt>API key</dt>
            <dd className="mono">{maskKey(provider.apiKey)}</dd>
          </div>
        )}
        {provider.model && (
          <div>
            <dt>Model</dt>
            <dd className="mono">{provider.model}</dd>
          </div>
        )}
      </dl>
      {testResult && (
        <div className={`providerTestResult ${testResult.ok ? "ok" : "fail"}`}>
          {testResult.ok ? <Check size={13} /> : <AlertTriangle size={13} />}
          <span>
            {testResult.status ? `HTTP ${testResult.status}` : "no response"} ·{" "}
            {testResult.latencyMs}ms · {testResult.message}
          </span>
        </div>
      )}
    </div>
  );
}

function ProviderEditor({
  provider,
  isNew,
  onSave,
  onClose
}: {
  provider: Provider;
  isNew: boolean;
  onSave: (p: Provider) => void;
  onClose: () => void;
}) {
  const [draft, setDraft] = React.useState<Provider>(provider);
  const [revealKey, setRevealKey] = React.useState(false);
  const firstFieldRef = React.useRef<HTMLInputElement>(null);
  const [modelOptions, setModelOptions] = React.useState<string[]>([]);
  const [fetchingModels, setFetchingModels] = React.useState(false);
  const [modelError, setModelError] = React.useState<string | null>(null);
  const modelDatalistId = React.useId();

  React.useEffect(() => {
    firstFieldRef.current?.focus();
  }, []);
  React.useEffect(() => {
    const onKey = (e: KeyboardEvent) => {
      if (e.key === "Escape") onClose();
    };
    window.addEventListener("keydown", onKey);
    return () => window.removeEventListener("keydown", onKey);
  }, [onClose]);

  const update = <K extends keyof Provider>(key: K, value: Provider[K]) => {
    setDraft((cur) => ({ ...cur, [key]: value }));
  };

  // OpenCode requires a model (its defaultModel() throws "no models
  // found" when our provider block has an empty `models: {}` map).
  // Claude / Codex / Gemini accept an empty model and fall back to
  // their own default — the placeholder "默认" makes that explicit.
  const modelRequired = draft.app === "opencode";
  const canSave =
    draft.name.trim().length > 0 &&
    (draft.baseUrl ?? "").trim().length > 0 &&
    (!modelRequired || (draft.model ?? "").trim().length > 0);

  const canFetchModels = (draft.baseUrl ?? "").trim().length > 0 && !fetchingModels;

  const fetchModels = async () => {
    if (!canFetchModels) return;
    setFetchingModels(true);
    setModelError(null);
    try {
      const result = await invoke<{
        ok: boolean;
        models: string[];
        status: number | null;
        message: string;
      }>("fetch_provider_models", { provider: draft });
      setModelOptions(result.models);
      if (!result.ok) {
        setModelError(
          result.status ? `${result.status} ${result.message}` : result.message
        );
      }
    } catch (err) {
      setModelError(String(err));
    } finally {
      setFetchingModels(false);
    }
  };

  const handleSubmit = (e: React.FormEvent) => {
    e.preventDefault();
    if (!canSave) return;
    // Trim every string field; `undefined` falls through so the
    // stripEmpty pass in config.ts later prunes them from disk.
    onSave({
      ...draft,
      name: draft.name.trim(),
      baseUrl: draft.baseUrl?.trim() || undefined,
      apiKey: draft.apiKey?.trim() || undefined,
      model: draft.model?.trim() || undefined,
      claudeSonnetModel: draft.claudeSonnetModel?.trim() || undefined,
      claudeOpusModel: draft.claudeOpusModel?.trim() || undefined,
      claudeHaikuModel: draft.claudeHaikuModel?.trim() || undefined
    });
  };

  return (
    <div
      className="providerEditorBackdrop"
      role="dialog"
      aria-modal="true"
      onMouseDown={(e) => {
        if (e.target === e.currentTarget) onClose();
      }}
    >
      <form
        className="providerEditorCard"
        onSubmit={handleSubmit}
        onMouseDown={(e) => e.stopPropagation()}
      >
        <header className="providerEditorHeader">
          <h2>{isNew ? "Add provider" : "Edit provider"}</h2>
          <span className="providerEditorPlatform">{CLI_APP_LABEL[draft.app]}</span>
        </header>

        <div className="providerEditorFields">
          <label className="providerField">
            <span className="providerFieldLabel">Name *</span>
            <input
              ref={firstFieldRef}
              type="text"
              className="providerInput"
              placeholder="Display name for this provider"
              value={draft.name}
              onChange={(e) => update("name", e.target.value)}
              required
            />
          </label>

          <label className="providerField">
            <span className="providerFieldLabel">Base URL *</span>
            <input
              type="text"
              className="providerInput mono"
              placeholder={baseUrlPlaceholder(draft.app)}
              value={draft.baseUrl ?? ""}
              onChange={(e) => update("baseUrl", e.target.value)}
              required
            />
            <span className="providerFieldHelp">{baseUrlHelp(draft.app)}</span>
          </label>

          <label className="providerField">
            <span className="providerFieldLabel">API key</span>
            <div className="providerKeyInput">
              <input
                type={revealKey ? "text" : "password"}
                className="providerInput mono providerKeyInputField"
                placeholder="sk-..."
                value={draft.apiKey ?? ""}
                onChange={(e) => update("apiKey", e.target.value)}
                autoComplete="off"
                spellCheck={false}
              />
              <button
                type="button"
                className="providerKeyToggle"
                onClick={() => setRevealKey((c) => !c)}
                aria-label={revealKey ? "Hide API key" : "Show API key"}
                title={revealKey ? "Hide API key" : "Show API key"}
              >
                {revealKey ? <EyeOff size={16} /> : <Eye size={16} />}
              </button>
            </div>
            <span className="providerFieldHelp">{apiKeyHelp(draft.app)}</span>
          </label>

          <label className="providerField">
            <span className="providerFieldLabel">
              Model{modelRequired ? " *" : " (optional)"}
            </span>
            <div className="providerKeyInput">
              <input
                type="text"
                className="providerInput mono providerKeyInputField"
                placeholder={
                  modelRequired
                    ? "Enter a model id"
                    : "Leave blank to use the default"
                }
                value={draft.model ?? ""}
                onChange={(e) => update("model", e.target.value)}
                list={modelOptions.length > 0 ? modelDatalistId : undefined}
                autoComplete="off"
              />
              <button
                type="button"
                className="providerKeyToggle"
                onClick={() => void fetchModels()}
                disabled={!canFetchModels}
                aria-label="Fetch available models from API"
                title="Fetch models from API"
              >
                {fetchingModels ? (
                  <Loader2 size={16} className="spin" />
                ) : (
                  <RefreshCw size={16} />
                )}
              </button>
            </div>
            {modelOptions.length > 0 && (
              <datalist id={modelDatalistId}>
                {modelOptions.map((m) => (
                  <option key={m} value={m} />
                ))}
              </datalist>
            )}
            {modelError && (
              <span className="providerFieldHelp providerFieldError">
                {modelError}
              </span>
            )}
            {!modelError && modelOptions.length > 0 && (
              <span className="providerFieldHelp">
                {modelOptions.length} models available — start typing to pick
              </span>
            )}
          </label>

          {draft.app === "claude" && (
            <details className="providerAdvanced">
              <summary className="providerAdvancedSummary">
                Advanced — per-size routing (Sonnet / Opus / Haiku)
              </summary>
              <p className="providerFieldHelp providerAdvancedHelp">
                When Claude Code's <code>/model</code> menu picks a size,
                it sends the model id below to your provider. Leave blank
                to fall back to the main model.
              </p>
              {(
                [
                  ["claudeSonnetModel", "Sonnet route", "e.g. gpt-5"],
                  ["claudeOpusModel", "Opus route", "e.g. claude-opus-4-7"],
                  ["claudeHaikuModel", "Haiku route", "e.g. deepseek-chat"]
                ] as const
              ).map(([key, label, ph]) => (
                <label key={key} className="providerField providerAdvancedField">
                  <span className="providerFieldLabel">{label}</span>
                  <input
                    type="text"
                    className="providerInput mono"
                    placeholder={ph}
                    value={(draft[key] as string | undefined) ?? ""}
                    onChange={(e) => update(key, e.target.value)}
                  />
                </label>
              ))}
            </details>
          )}

        </div>

        <footer className="providerEditorFooter">
          <button type="button" className="providersGhost" onClick={onClose}>
            Cancel
          </button>
          <button type="submit" className="providersPrimary" disabled={!canSave}>
            {isNew ? "Create" : "Save"}
          </button>
        </footer>
      </form>
    </div>
  );
}

function customProviderHint(app: CliApp): string {
  switch (app) {
    case "claude":
      return "Writes ~/.claude/settings.json env block (ANTHROPIC_BASE_URL + auth token + model).";
    case "codex":
      return "Writes ~/.codex/auth.json (OPENAI_API_KEY) + ~/.codex/config.toml ([model_providers.termory] block).";
    case "gemini":
      return "Writes ~/.gemini/.env (GOOGLE_GEMINI_BASE_URL + GEMINI_API_KEY), file mode 0600.";
    case "opencode":
      return "Writes ~/.config/opencode/opencode.json (provider.termory.options + qualified model).";
  }
}

function baseUrlPlaceholder(app: CliApp): string {
  switch (app) {
    case "claude":
      return "https://api.anthropic.com";
    case "codex":
      return "https://api.openai.com/v1";
    case "gemini":
      return "https://generativelanguage.googleapis.com";
    case "opencode":
      return "https://api.anthropic.com";
  }
}

function baseUrlHelp(app: CliApp): string {
  switch (app) {
    case "claude":
      return "Hits ANTHROPIC_BASE_URL. Don't include /v1 — Claude appends it.";
    case "codex":
      return "Include /v1 (Codex uses it as model_providers.<id>.base_url verbatim).";
    case "gemini":
      return "Triggers Gemini's GATEWAY mode via GOOGLE_GEMINI_BASE_URL env var.";
    case "opencode":
      return "Goes to provider.termory.options.baseURL. Use the OpenAI/Anthropic-compatible root URL.";
  }
}

function apiKeyHelp(app: CliApp): string {
  switch (app) {
    case "claude":
      return "Stored in ~/.claude/settings.json env block.";
    case "codex":
      return "Stored in ~/.codex/auth.json under OPENAI_API_KEY.";
    case "gemini":
      return "Stored in ~/.gemini/.env (chmod 600).";
    case "opencode":
      return "Stored in opencode.json under provider.termory.options.apiKey.";
  }
}



function useSearchHits(query: string) {
  const [hits, setHits] = React.useState<SearchHit[]>([]);
  const [loading, setLoading] = React.useState(false);
  const [committedQuery, setCommittedQuery] = React.useState("");
  const [error, setError] = React.useState<string | null>(null);

  React.useEffect(() => {
    const trimmed = query.trim();
    if (trimmed.length < 2) {
      setHits([]);
      setCommittedQuery("");
      setLoading(false);
      setError(null);
      return;
    }
    let cancelled = false;
    const handle = window.setTimeout(() => {
      setLoading(true);
      invoke<SearchHit[]>("search_all_sessions", { query: trimmed })
        .then((result) => {
          if (cancelled) return;
          setHits(result);
          setCommittedQuery(trimmed);
          setError(null);
        })
        .catch((err) => {
          if (!cancelled) setError(String(err));
        })
        .finally(() => {
          if (!cancelled) setLoading(false);
        });
    }, 300);
    return () => {
      cancelled = true;
      window.clearTimeout(handle);
    };
  }, [query]);

  return { hits, loading, committedQuery, error };
}

function SearchPage({
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
    <div className="searchPage">
      <div className="searchHeader">
        <div className="searchInputBox">
          <Search size={16} />
          <input
            ref={inputRef}
            type="search"
            className="searchInput"
            placeholder="Search across sessions, memories, skills…"
            value={query}
            onChange={(event) => setQuery(event.target.value)}
            autoFocus
          />
          {loading && <Loader2 className="spin searchSpinner" size={14} />}
        </div>
        {settled && hits.length > 0 && (
          <div className="searchSummary">
            {formatFullNumber(hits.length)} {hits.length === 1 ? "match" : "matches"}
            <span className="searchSummarySep">·</span>
            <span>Sessions {groups.sessions.length}</span>
            <span className="searchSummarySep">·</span>
            <span>Memories {groups.memories.length}</span>
            <span className="searchSummarySep">·</span>
            <span>Skills {groups.skills.length}</span>
          </div>
        )}
      </div>
      <div className="searchResults">
        {error && <div className="error">{error}</div>}
        {trimmed.length < 2 && !loading && (
          <div className="searchHint">
            <Search size={28} />
            <p>Search inside every session, memory, and skill Termory scans.</p>
            <p className="searchKbdHint">
              <span>Press</span>
              <kbd>⌘</kbd>
              <kbd>K</kbd>
              <span>to summon search from anywhere.</span>
            </p>
            <p className="searchCorpusHint">
              {formatFullNumber(sessions.length)} records indexed.
            </p>
            {recentSearches.length > 0 && (
              <div className="searchRecent">
                <div className="searchRecentHeader">
                  <span>Recent</span>
                  <button
                    type="button"
                    className="searchRecentClear"
                    onClick={onClearRecent}
                  >
                    Clear
                  </button>
                </div>
                <div className="searchRecentChips">
                  {recentSearches.map((entry) => (
                    <button
                      key={entry}
                      type="button"
                      className="searchRecentChip"
                      onClick={() => setQuery(entry)}
                    >
                      {entry}
                    </button>
                  ))}
                </div>
              </div>
            )}
          </div>
        )}
        {noResults && (
          <EmptyState icon={<Search />} title={`No matches for "${trimmed}"`} />
        )}
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

function SearchGroup({
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
    <section className="searchGroup">
      <header className="searchGroupHeader">
        <span className="searchGroupIcon">{icon}</span>
        <h3>{title}</h3>
        <b>{hits.length}</b>
      </header>
      <div className="searchGroupList">
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
        <div className="searchGroupTruncated">+ {formatFullNumber(truncated)} more</div>
      )}
    </section>
  );
}

function SearchResultCard({
  hit,
  query,
  onOpen
}: {
  hit: SearchHit;
  query: string;
  onOpen: () => void;
}) {
  const session = hit.session;
  const sessionTypeLabel = !isSessionItem(session);
  const tools = memoryToolsOf(session);
  return (
    <button className="sessionCard searchResultCard" onClick={onOpen}>
      <div className="sessionHeader">
        {sessionTypeLabel ? (
          <span className="memoryBadges">
            {tools.map((tool) => (
              <span
                key={tool}
                className={`badge ${tool === "Other" ? "memory" : tool.toLowerCase()}`}
              >
                {tool === "Other" ? "Memory" : sourceDisplayName(tool)}
              </span>
            ))}
          </span>
        ) : (
          <span className={`badge ${session.source.toLowerCase()}`}>{sourceDisplayName(session.source)}</span>
        )}
        <span className="date">{formatDate(session.updated_at ?? session.started_at)}</span>
      </div>
      <h2>{session.title || "(untitled)"}</h2>
      <div className="sessionMeta">
        <span className="sessionMetaProject" title={session.project}>
          <Folder size={12} />
          <span>{projectDisplayName(session.project)}</span>
        </span>
        {isSessionItem(session) && <span>{session.message_count} messages</span>}
      </div>
      <SnippetLine
        snippet={hit.snippet}
        query={query}
        role={hit.role}
        matchCount={hit.match_count}
      />
    </button>
  );
}

function CommandPalette({
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
  const [open, setOpen] = React.useState(false);
  const [query, setQuery] = React.useState("");
  const { hits, loading, committedQuery } = useSearchHits(query);
  const inputRef = React.useRef<HTMLInputElement>(null);
  const [activeIndex, setActiveIndex] = React.useState(0);

  const handleOpen = React.useCallback(
    (item: AppSession) => {
      onCommitSearch(committedQuery || query);
      onOpenItem(item);
      setOpen(false);
    },
    [committedQuery, onCommitSearch, onOpenItem, query]
  );

  // Global ⌘K / Ctrl+K toggle. Esc closes (handled here so it works
  // even before the input gets focus).
  React.useEffect(() => {
    const onKeyDown = (event: KeyboardEvent) => {
      const isToggle = (event.metaKey || event.ctrlKey) && event.key.toLowerCase() === "k";
      if (isToggle) {
        event.preventDefault();
        setOpen((current) => !current);
      } else if (event.key === "Escape") {
        setOpen((current) => (current ? false : current));
      }
    };
    window.addEventListener("keydown", onKeyDown);
    return () => window.removeEventListener("keydown", onKeyDown);
  }, []);

  // On open: focus the input and reset state. On close: clear query so
  // the next open is a fresh slate.
  React.useEffect(() => {
    if (open) {
      inputRef.current?.focus();
      setActiveIndex(0);
    } else {
      setQuery("");
    }
  }, [open]);

  React.useEffect(() => {
    setActiveIndex(0);
  }, [committedQuery, query]);

  // Cheap metadata-only fallback so the palette feels live before the
  // backend debounce settles (or when there are 1-char queries the
  // backend rejects). Capped — palette is a quick-jump surface.
  const fallbackHits = React.useMemo<SearchHit[]>(() => {
    const trimmed = query.trim();
    if (committedQuery === trimmed) return [];
    if (trimmed.length === 0) return [];
    const needle = trimmed.toLowerCase();
    const matches: SearchHit[] = [];
    for (const session of sessions) {
      const haystack = `${session.title}\n${session.project}\n${session.source}`.toLowerCase();
      if (haystack.includes(needle)) {
        matches.push({ session, snippet: "", role: "", match_count: 0 });
        if (matches.length >= 16) break;
      }
    }
    return matches;
  }, [query, committedQuery, sessions]);

  const rows = hits.length > 0 ? hits.slice(0, 8) : fallbackHits.slice(0, 8);
  const trimmed = query.trim();
  const settled = committedQuery === trimmed && trimmed.length >= 2;

  const handleKeyDown = (event: React.KeyboardEvent) => {
    if (event.key === "ArrowDown") {
      event.preventDefault();
      setActiveIndex((idx) => Math.min(idx + 1, Math.max(rows.length - 1, 0)));
    } else if (event.key === "ArrowUp") {
      event.preventDefault();
      setActiveIndex((idx) => Math.max(idx - 1, 0));
    } else if (event.key === "Enter") {
      const row = rows[activeIndex];
      if (row) {
        event.preventDefault();
        handleOpen(row.session);
      }
    }
  };

  if (!open) return null;

  return (
    <div
      className="paletteBackdrop"
      role="dialog"
      aria-modal="true"
      aria-label="Quick search"
      onMouseDown={(event) => {
        if (event.target === event.currentTarget) setOpen(false);
      }}
    >
      <div className="paletteCard" onMouseDown={(event) => event.stopPropagation()}>
        <div className="paletteInputBox">
          <Search size={16} />
          <input
            ref={inputRef}
            className="paletteInput"
            type="search"
            placeholder="Find sessions, memories, skills…"
            value={query}
            onChange={(event) => setQuery(event.target.value)}
            onKeyDown={handleKeyDown}
            autoFocus
          />
          {loading && <Loader2 className="spin paletteSpinner" size={14} />}
          <kbd className="paletteKbd">ESC</kbd>
        </div>
        <div className="paletteResults">
          {trimmed.length === 0 && recentSearches.length === 0 && (
            <div className="paletteEmpty">Type to search across all records.</div>
          )}
          {trimmed.length === 0 && recentSearches.length > 0 && (
            <div className="paletteRecent">
              <div className="paletteRecentHeader">
                <span>Recent searches</span>
                <button
                  type="button"
                  className="paletteRecentClear"
                  onClick={onClearRecent}
                >
                  Clear
                </button>
              </div>
              {recentSearches.map((entry) => (
                <button
                  key={entry}
                  type="button"
                  className="paletteRecentRow"
                  onClick={() => setQuery(entry)}
                >
                  <Search size={13} />
                  <span>{entry}</span>
                </button>
              ))}
            </div>
          )}
          {trimmed.length > 0 && rows.length === 0 && settled && !loading && (
            <div className="paletteEmpty">No matches.</div>
          )}
          {rows.map((row, idx) => (
            <PaletteRow
              key={sessionKey(row.session)}
              hit={row}
              query={committedQuery}
              active={idx === activeIndex}
              onMouseEnter={() => setActiveIndex(idx)}
              onClick={() => handleOpen(row.session)}
            />
          ))}
        </div>
        <div className="paletteFooter">
          <span><kbd>↑</kbd><kbd>↓</kbd> navigate</span>
          <span><kbd>↵</kbd> open</span>
          <span><kbd>esc</kbd> close</span>
        </div>
      </div>
    </div>
  );
}

function PaletteRow({
  hit,
  query,
  active,
  onMouseEnter,
  onClick
}: {
  hit: SearchHit;
  query: string;
  active: boolean;
  onMouseEnter: () => void;
  onClick: () => void;
}) {
  const session = hit.session;
  const rowRef = React.useRef<HTMLButtonElement>(null);
  React.useEffect(() => {
    if (active) {
      rowRef.current?.scrollIntoView({ block: "nearest" });
    }
  }, [active]);
  const typeLabel = typeLabelOf(session);
  const showSnippet = hit.snippet.length > 0;
  return (
    <button
      ref={rowRef}
      className={active ? "paletteRow active" : "paletteRow"}
      onMouseEnter={onMouseEnter}
      onClick={onClick}
    >
      <span className={`badge ${typeLabel.toLowerCase()} paletteRowType`}>{typeLabel}</span>
      <span className="paletteRowBody">
        <span className="paletteRowTitle">{session.title || "(untitled)"}</span>
        <span className="paletteRowMeta">
          <span className="paletteRowSource">{sourceDisplayName(session.source)}</span>
          <span className="paletteRowSep">·</span>
          <span className="paletteRowProject" title={session.project}>
            {projectDisplayName(session.project)}
          </span>
        </span>
        {showSnippet && (
          <span className="paletteRowSnippet">
            {splitSnippet(hit.snippet, query).map((seg, index) =>
              seg.match ? <mark key={index}>{seg.text}</mark> : <span key={index}>{seg.text}</span>
            )}
          </span>
        )}
      </span>
    </button>
  );
}

function RoutePlaceholder({ route }: { route: Route }) {
  const labels: Record<Route, { title: string; detail: string }> = {
    records: { title: "Records", detail: "" },
    search: { title: "Search", detail: "" },
    stats: {
      title: "Stats",
      detail: "Dashboards (sessions / day, tokens per tool, top projects) land here in a later phase."
    },
    config: {
      title: "Providers",
      detail: "Per-CLI provider profile editor — base URL / API key / model, with quick-switch. Lands in a later phase."
    },
    settings: {
      title: "Settings",
      detail: "App preferences (theme, scan paths, keyboard shortcuts). Lands in a later phase."
    }
  };
  const { title, detail } = labels[route];
  return (
    <div className="placeholderPage">
      <div className="placeholderCard">
        <h2>{title}</h2>
        <p>{detail}</p>
      </div>
    </div>
  );
}

function MemoryCard({
  item,
  selected,
  onClick,
  query,
  contentQuery,
  hit
}: {
  item: AppSession;
  selected: AppSession | null;
  onClick: () => void;
  query: string;
  contentQuery: string;
  hit: SearchHit | undefined;
}) {
  const showSnippet = !!hit && query.toLowerCase() === contentQuery.toLowerCase();
  const isActive = selected?.path === item.path && selected?.id === item.id;
  const tools = memoryToolsOf(item);
  return (
    <button
      className={isActive ? "sessionCard active" : "sessionCard"}
      onClick={onClick}
    >
      <div className="sessionHeader">
        <span className="memoryBadges">
          {tools.map((tool) => (
            <span
              key={tool}
              className={`badge ${tool === "Other" ? "memory" : tool.toLowerCase()}`}
            >
              {tool === "Other" ? "Memory" : sourceDisplayName(tool)}
            </span>
          ))}
        </span>
        <span className="date">{formatDate(item.updated_at ?? item.started_at)}</span>
      </div>
      <h2>{item.title}</h2>
      <div className="sessionMeta">
        <span className="sessionMetaProject" title={item.project}>
          <Folder size={12} />
          <span>{projectDisplayName(item.project)}</span>
        </span>
      </div>
      {showSnippet && (
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

function SnippetLine({
  snippet,
  query,
  role,
  matchCount
}: {
  snippet: string;
  query: string;
  role: string;
  matchCount: number;
}) {
  const segments = React.useMemo(() => splitSnippet(snippet, query), [snippet, query]);
  const label = role ? role : "match";
  return (
    <div className="sessionSnippet">
      <span className="sessionSnippetMeta">
        <MessageSquare size={11} />
        <span>{label}</span>
        {matchCount > 1 && <span className="sessionSnippetCount">×{matchCount}</span>}
      </span>
      <span className="sessionSnippetText">
        {segments.map((seg, index) =>
          seg.match ? <mark key={index}>{seg.text}</mark> : <span key={index}>{seg.text}</span>
        )}
      </span>
    </div>
  );
}

function splitSnippet(snippet: string, query: string): { text: string; match: boolean }[] {
  if (!query) return [{ text: snippet, match: false }];
  const lowerSnippet = snippet.toLowerCase();
  const lowerQuery = query.toLowerCase();
  const out: { text: string; match: boolean }[] = [];
  let cursor = 0;
  while (cursor < snippet.length) {
    const idx = lowerSnippet.indexOf(lowerQuery, cursor);
    if (idx === -1) {
      out.push({ text: snippet.slice(cursor), match: false });
      break;
    }
    if (idx > cursor) out.push({ text: snippet.slice(cursor, idx), match: false });
    out.push({ text: snippet.slice(idx, idx + lowerQuery.length), match: true });
    cursor = idx + lowerQuery.length;
  }
  return out;
}

function EmptyState({
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
  const detailed = description != null || action != null;
  return (
    <div className={detailed ? "emptyState emptyStateDetailed" : "emptyState"}>
      <span className="emptyStateIcon">{icon}</span>
      <span className="emptyStateTitle">{title}</span>
      {description && <span className="emptyStateDescription">{description}</span>}
      {action && (
        <button type="button" className="emptyStateAction" onClick={action.onClick}>
          {action.label}
        </button>
      )}
    </div>
  );
}

function InfoPill({ icon, label }: { icon: React.ReactNode; label: string }) {
  return (
    <div className="infoPill">
      {icon}
      <span>{label}</span>
    </div>
  );
}

function BrandIcon({ source }: { source: string }) {
  if (source === "Codex") {
    return (
      <svg className="brandIcon codexIcon" viewBox="0 0 24 24" aria-hidden="true">
        <path d="M22.2819 9.8211a5.9847 5.9847 0 0 0-.5157-4.9108 6.0462 6.0462 0 0 0-6.5098-2.9A6.0651 6.0651 0 0 0 4.9807 4.1818a5.9847 5.9847 0 0 0-3.9977 2.9 6.0462 6.0462 0 0 0 .7427 7.0966 5.98 5.98 0 0 0 .511 4.9107 6.051 6.051 0 0 0 6.5146 2.9001A5.9847 5.9847 0 0 0 13.2599 24a6.0557 6.0557 0 0 0 5.7718-4.2058 5.9894 5.9894 0 0 0 3.9977-2.9001 6.0557 6.0557 0 0 0-.7475-7.0729Zm-9.022 12.6081a4.4755 4.4755 0 0 1-2.8764-1.0408l.1419-.0804 4.7783-2.7582a.7948.7948 0 0 0 .3927-.6813v-6.7369l2.02 1.1686a.071.071 0 0 1 .038.052v5.5826a4.504 4.504 0 0 1-4.4945 4.4944Zm-9.6607-4.1254a4.4708 4.4708 0 0 1-.5346-3.0137l.142.0852 4.783 2.7582a.7712.7712 0 0 0 .7806 0l5.8428-3.3685v2.3324a.0804.0804 0 0 1-.0332.0615L9.74 19.9502a4.4992 4.4992 0 0 1-6.1408-1.6464ZM2.3408 7.8956a4.485 4.485 0 0 1 2.3655-1.9728V11.6a.7664.7664 0 0 0 .3879.6765l5.8144 3.3543-2.0201 1.1685a.0757.0757 0 0 1-.071 0l-4.8303-2.7865A4.504 4.504 0 0 1 2.3408 7.872Zm16.5963 3.8558L13.1038 8.364 15.1192 7.2a.0757.0757 0 0 1 .071 0l4.8303 2.7913a4.4944 4.4944 0 0 1-.6765 8.1042v-5.6772a.79.79 0 0 0-.407-.667Zm2.0107-3.0231-.142-.0852-4.7735-2.7818a.7759.7759 0 0 0-.7854 0L9.409 9.2297V6.8974a.0662.0662 0 0 1 .0284-.0615l4.8303-2.7866a4.4992 4.4992 0 0 1 6.6802 4.66ZM8.3065 12.863l-2.02-1.1638a.0804.0804 0 0 1-.038-.0567V6.0742a4.4992 4.4992 0 0 1 7.3757-3.4537l-.142.0805L8.704 5.459a.7948.7948 0 0 0-.3927.6813Zm1.0976-2.3654 2.602-1.4998 2.6069 1.4998v2.9994l-2.5974 1.4997-2.6067-1.4997Z" />
      </svg>
    );
  }
  if (source === "Claude") {
    return (
      <svg className="brandIcon claudeIcon" viewBox="0 0 24 24" aria-hidden="true">
        <path d="m4.7144 15.9555 4.7174-2.6471.079-.2307-.079-.1275h-.2307l-.7893-.0486-2.6956-.0729-2.3375-.0971-2.2646-.1214-.5707-.1215-.5343-.7042.0546-.3522.4797-.3218.686.0608 1.5179.1032 2.2767.1578 1.6514.0972 2.4468.255h.3886l.0546-.1579-.1336-.0971-.1032-.0972L6.973 9.8356l-2.55-1.6879-1.3356-.9714-.7225-.4918-.3643-.4614-.1578-1.0078.6557-.7225.8803.0607.2246.0607.8925.686 1.9064 1.4754 2.4893 1.8336.3643.3035.1457-.1032.0182-.0728-.164-.2733-1.3539-2.4467-1.445-2.4893-.6435-1.032-.17-.6194c-.0607-.255-.1032-.4674-.1032-.7285L6.287.1335 6.6997 0l.9957.1336.419.3642.6192 1.4147 1.0018 2.2282 1.5543 3.0296.4553.8985.2429.8318.091.255h.1579v-.1457l.1275-1.706.2368-2.0947.2307-2.6957.0789-.7589.3764-.9107.7468-.4918.5828.2793.4797.686-.0668.4433-.2853 1.8517-.5586 2.9021-.3643 1.9429h.2125l.2429-.2429.9835-1.3053 1.6514-2.0643.7286-.8196.85-.9046.5464-.4311h1.0321l.759 1.1293-.34 1.1657-1.0625 1.3478-.8804 1.1414-1.2628 1.7-.7893 1.36.0729.1093.1882-.0183 2.8535-.607 1.5421-.2794 1.8396-.3157.8318.3886.091.3946-.3278.8075-1.967.4857-2.3072.4614-3.4364.8136-.0425.0304.0486.0607 1.5482.1457.6618.0364h1.621l3.0175.2247.7892.522.4736.6376-.079.4857-1.2142.6193-1.6393-.3886-3.825-.9107-1.3113-.3279h-.1822v.1093l1.0929 1.0686 2.0035 1.8092 2.5075 2.3314.1275.5768-.3218.4554-.34-.0486-2.2039-1.6575-.85-.7468-1.9246-1.621h-.1275v.17l.4432.6496 2.3436 3.5214.1214 1.0807-.17.3521-.6071.2125-.6679-.1214-1.3721-1.9246L14.38 17.959l-1.1414-1.9428-.1397.079-.674 7.2552-.3156.3703-.7286.2793-.6071-.4614-.3218-.7468.3218-1.4753.3886-1.9246.3157-1.53.2853-1.9004.17-.6314-.0121-.0425-.1397.0182-1.4328 1.9672-2.1796 2.9446-1.7243 1.8456-.4128.164-.7164-.3704.0667-.6618.4008-.5889 2.386-3.0357 1.4389-1.882.929-1.0868-.0062-.1579h-.0546l-6.3385 4.1164-1.1293.1457-.4857-.4554.0608-.7467.2307-.2429 1.9064-1.3114Z" />
      </svg>
    );
  }
  if (source === "Gemini") {
    return (
      <svg className="brandIcon geminiIcon" viewBox="0 0 24 24" aria-hidden="true">
        <defs>
          <linearGradient id="geminiGradient" x1="4" x2="20" y1="20" y2="4" gradientUnits="userSpaceOnUse">
            <stop stopColor="#4285f4" />
            <stop offset="0.5" stopColor="#a142f4" />
            <stop offset="1" stopColor="#34a853" />
          </linearGradient>
        </defs>
        <path d="M11.04 19.32Q12 21.51 12 24q0-2.49.93-4.68.96-2.19 2.58-3.81t3.81-2.55Q21.51 12 24 12q-2.49 0-4.68-.93a12.3 12.3 0 0 1-3.81-2.58 12.3 12.3 0 0 1-2.58-3.81Q12 2.49 12 0q0 2.49-.96 4.68-.93 2.19-2.55 3.81a12.3 12.3 0 0 1-3.81 2.58Q2.49 12 0 12q2.49 0 4.68.96 2.19.93 3.81 2.55t2.55 3.81" />
      </svg>
    );
  }
  if (source === "OpenCode") {
    return (
      <svg className="brandIcon opencodeIcon" viewBox="0 0 24 24" aria-hidden="true">
        <path d="M4 3h16v18H4Z" />
        <path d="M8.2 7.7h7.6v4.2H8.2Z" />
        <path d="M8.2 11.9h7.6v4.4H8.2Z" className="opencodePane" />
      </svg>
    );
  }
  if (source === "Memory") {
    return <BookOpen className="brandIcon memoryIcon" size={18} aria-hidden="true" />;
  }
  return (
    <svg className="brandIcon allIcon" viewBox="0 0 24 24" aria-hidden="true">
      <rect x="4" y="4" width="6.5" height="6.5" rx="1.7" />
      <rect x="13.5" y="4" width="6.5" height="6.5" rx="1.7" />
      <rect x="4" y="13.5" width="6.5" height="6.5" rx="1.7" />
      <rect x="13.5" y="13.5" width="6.5" height="6.5" rx="1.7" />
    </svg>
  );
}

// `Intl.DateTimeFormat` construction is expensive (loads locale data
// the first time per option set). Cache one instance and reuse on
// every call — formatDate runs hundreds of times per App re-render
// across the session list + detail messages.
const dateFormatter = new Intl.DateTimeFormat(undefined, {
  month: "short",
  day: "numeric",
  hour: "2-digit",
  minute: "2-digit"
});

function formatDate(value?: string | null) {
  if (!value) return "Unknown time";
  const date = new Date(value);
  if (Number.isNaN(date.getTime())) return value;
  return dateFormatter.format(date);
}

function roleClass(role: string) {
  const lowered = role.toLowerCase();
  if (lowered.includes("user")) return "user";
  if (lowered.includes("assistant")) return "assistant";
  if (lowered.includes("tool")) return "tool";
  return "event";
}

function formatCompactNumber(value: number) {
  if (value < 1000) return String(value);
  const compact = value / 1000;
  const rounded = compact >= 10 ? Math.round(compact).toString() : compact.toFixed(1);
  return `${rounded.replace(/\.0$/, "")}k`;
}

// Same caching reason as `dateFormatter` — `Intl.NumberFormat()` is
// reconstructed for every metric tile and project tooltip otherwise.
const numberFormatter = new Intl.NumberFormat();

function formatFullNumber(value: number) {
  return numberFormatter.format(value);
}

function projectDisplayName(project: string) {
  // Tool config "projects" (`~/.codex`, `~/.claude/skills`, `~/.gemini`,
  // etc.) keep their full label — basenaming them yields useless
  // strings like `.codex` or `skills` that don't tell the user which
  // platform or scope they're looking at. Only real filesystem paths
  // (cwd / git repo roots) get shortened to their leaf folder.
  if (project.startsWith("~/") || project.startsWith("~\\")) return project;
  return project.split(/[\\/]+/).filter(Boolean).pop() ?? project;
}

function addSetValue(current: Set<string>, value: string) {
  const next = new Set(current);
  next.add(value);
  return next;
}

function toggleSetValue(current: Set<string>, value: string) {
  const next = new Set(current);
  if (next.has(value)) {
    next.delete(value);
  } else {
    next.add(value);
  }
  return next;
}

ReactDOM.createRoot(document.getElementById("root") as HTMLElement).render(
  <React.StrictMode>
    <App />
  </React.StrictMode>
);
