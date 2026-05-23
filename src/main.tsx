import React from "react";
import ReactDOM from "react-dom/client";
import { invoke } from "@tauri-apps/api/core";
import { revealItemInDir } from "@tauri-apps/plugin-opener";
import {
  Bot,
  BookOpen,
  CalendarDays,
  ChevronDown,
  ChevronRight,
  Folder,
  ExternalLink,
  FileJson,
  Loader2,
  MessageSquare,
  MessagesSquare,
  RefreshCw,
  Search,
  Sparkles,
  Tags
} from "lucide-react";
import ReactMarkdown from "react-markdown";
import remarkGfm from "remark-gfm";
import "./styles.css";

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
  const [pane, setPane] = React.useState<"sessions" | "memory" | "skills">("sessions");

  const refresh = React.useCallback(async () => {
    setLoading(true);
    setError(null);
    try {
      const result = await invoke<AppSession[]>("scan_all_sessions");
      setSessions(result);
      setSelected((current) => {
        if (!current) return result[0] ?? null;
        return result.find((session) => session.source === current.source && session.path === current.path && session.id === current.id) ?? result[0] ?? null;
      });
    } catch (err) {
      setError(String(err));
    } finally {
      setLoading(false);
    }
  }, []);

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


  const stats = React.useMemo(() => {
    const totalMessages = sessionItems.reduce((sum, item) => sum + item.message_count, 0);
    const projects = new Set(sessionItems.map((item) => item.project).filter(Boolean)).size;
    return { totalMessages, projects };
  }, [sessionItems]);

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
                  <span>{group.source}</span>
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

        <div className="metricGrid" aria-label="Library statistics">
          <div title={`${formatFullNumber(stats.projects)} Projects`}>
            <span className="metricIcon"><Folder size={11} /></span>
            <span className="metricValue">{formatCompactNumber(stats.projects)}</span>
          </div>
          <div title={`${formatFullNumber(sessionItems.length)} Sessions`}>
            <span className="metricIcon"><MessagesSquare size={11} /></span>
            <span className="metricValue">{formatCompactNumber(sessionItems.length)}</span>
          </div>
          <div title={`${formatFullNumber(stats.totalMessages)} Messages`}>
            <span className="metricIcon"><MessageSquare size={11} /></span>
            <span className="metricValue">{formatCompactNumber(stats.totalMessages)}</span>
          </div>
        </div>
      </aside>

      <section className="sessionsPane">
        <header className="toolbar">
          <div className="searchBox">
            <Search size={17} />
            <input
              value={query}
              onChange={(event) => setQuery(event.target.value)}
              placeholder="Search sessions, projects, message content"
            />
            {searchingContent && <Loader2 size={14} className="spin searchSpinner" />}
          </div>
          <button className="iconButton" onClick={refresh} disabled={loading} title="Refresh sessions">
            {loading ? <Loader2 size={18} className="spin" /> : <RefreshCw size={18} />}
          </button>
        </header>

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
            {!loading && filtered.length === 0 && (
              <EmptyState icon={<FileJson />} title="No sessions found" />
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
                      <span className={`badge ${session.source.toLowerCase()}`}>{session.source}</span>
                      <span className="date">{formatDate(session.updated_at ?? session.started_at)}</span>
                    </div>
                    <h2>{session.title}</h2>
                    <div className="sessionMeta">
                      <span>{session.project}</span>
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
            {!loading && filteredMemories.length === 0 && (
              <EmptyState icon={<BookOpen />} title="No memory found" />
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
            {!loading && filteredSkills.length === 0 && (
              <EmptyState icon={<Sparkles />} title="No skills found" />
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
        {!selected && <EmptyState icon={<Sparkles />} title="Select a session" />}
        {selected && (
          <>
            <header className="detailHeader">
              <div>
                <div className="detailKicker">
                  <span className={`badge ${typeLabelOf(selected).toLowerCase()}`}>{typeLabelOf(selected)}</span>
                </div>
                <h2>{selected.title}</h2>
                <p>{selected.project}</p>
                {isSessionItem(selected) && (
                  <p className="detailGuid" title="Session GUID">{selected.id}</p>
                )}
              </div>
              <div className="detailHeaderActions">
                <button className="iconButton" onClick={() => revealItemInDir(selected.path)} title="Show original file">
                  <ExternalLink size={18} />
                </button>
              </div>
            </header>

            <div className="detailStats">
              <InfoPill icon={<CalendarDays size={15} />} label={formatDate(selected.updated_at ?? selected.started_at)} />
              {isSessionItem(selected) && (
                <InfoPill icon={<Bot size={15} />} label={`${selected.message_count} messages`} />
              )}
              <InfoPill icon={<Tags size={15} />} label={selected.path} />
            </div>

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
              {tool === "Other" ? "Memory" : tool}
            </span>
          ))}
        </span>
        <span className="date">{formatDate(item.updated_at ?? item.started_at)}</span>
      </div>
      <h2>{item.title}</h2>
      <div className="sessionMeta">
        <span title={item.path}>{item.project}</span>
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

function EmptyState({ icon, title }: { icon: React.ReactNode; title: string }) {
  return (
    <div className="emptyState">
      {icon}
      <span>{title}</span>
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
