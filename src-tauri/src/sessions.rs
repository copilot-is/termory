use chrono::{DateTime, Local, TimeZone, Utc};
use rusqlite::{Connection, OpenFlags, OptionalExtension};
use serde::{Deserialize, Serialize};
use serde_json::Value;
use std::collections::{HashMap, HashSet};
use std::error::Error;
use std::fs;
use std::io::{Read, Seek, SeekFrom};
use std::path::Path;
use std::time::SystemTime;
use walkdir::WalkDir;

const CLAUDE_LITE_READ_BUF_SIZE: usize = 65_536;

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AppSession {
    pub id: String,
    pub source: String,
    pub title: String,
    pub project: String,
    pub path: String,
    pub started_at: Option<String>,
    pub updated_at: Option<String>,
    pub message_count: usize,
    pub preview: String,
    pub message_previews: Vec<SessionMessage>,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct SessionMessage {
    pub role: String,
    pub text: String,
    pub timestamp: Option<String>,
    pub kind: String,
    /// Internal: links a tool_use message with its tool_result by provider id
    /// (Claude tool_use.id / Codex function_call.call_id). Used by
    /// `merge_tool_outputs` and not exposed to the frontend.
    #[serde(skip)]
    pub tool_use_id: Option<String>,
    /// Internal: shell exit code parsed from a Codex `function_call_output`
    /// metadata wrapper. `merge_tool_outputs` surfaces it as part of the
    /// `**Error** · exit {N}` footer so failed commands carry diagnosis
    /// info even when stdout/stderr were empty.
    #[serde(skip)]
    pub exit_code: Option<i64>,
}

/// Leading status marker for tool-card headers. Pure unicode (no HTML
/// or styling — the frontend renders as-is):
/// * success → `⏺` BLACK CIRCLE FOR RECORD (Claude TUI BLACK_CIRCLE,
///   constants/figures.ts:4)
/// * failure → `✗` BALLOT X (Codex failure badge,
///   exec_cell/render.rs:236)
fn status_marker(failed: bool) -> &'static str {
    if failed {
        "✗ "
    } else {
        "⏺ "
    }
}

/// Wrap a string as a markdown inline code span, choosing the backtick
/// delimiter so the content's own backticks don't terminate the span.
/// Implements CommonMark 6.1 (Code spans): if the content begins or ends
/// with a backtick, both delimiters are padded with a space (those spaces
/// are stripped during render).
fn wrap_inline_code(content: &str) -> String {
    let max_run = content
        .chars()
        .fold((0usize, 0usize), |(max, cur), c| {
            if c == '`' {
                let new_cur = cur + 1;
                (max.max(new_cur), new_cur)
            } else {
                (max, 0)
            }
        })
        .0;
    let delim = "`".repeat(max_run + 1);
    if content.starts_with('`') || content.ends_with('`') {
        format!("{delim} {content} {delim}")
    } else {
        format!("{delim}{content}{delim}")
    }
}

/// Format reasoning content as italic blockquote (`> *content*`) so the
/// frontend's default markdown renderer styles it visibly distinct from
/// normal assistant text — matching the common "thinking" display
/// convention.
fn format_reasoning_body(content: &str) -> String {
    let trimmed = content.trim();
    if trimmed.is_empty() {
        return String::new();
    }
    // Italic doesn't survive multiple paragraphs cleanly, so wrap each
    // non-empty line in `> *...*` so the entire block renders as one
    // styled blockquote.
    let mut out = String::new();
    for (idx, line) in trimmed.lines().enumerate() {
        if idx > 0 {
            out.push('\n');
        }
        let line = line.trim_end();
        if line.is_empty() {
            out.push('>');
        } else {
            out.push_str("> *");
            // Escape stray `*` chars in the source so italic isn't broken.
            for c in line.chars() {
                if c == '*' || c == '_' {
                    out.push('\\');
                }
                out.push(c);
            }
            out.push('*');
        }
    }
    out
}

/// Walk a list of tool-related messages and fold `tool_result` /
/// `tool_error` output into the preceding `tool_use` matched by
/// `tool_use_id`. Mirrors the TUI experience where a tool call and its
/// output render in a single block. Messages without a matching pair are
/// left in place so orphaned results are not lost.
fn merge_tool_outputs(messages: Vec<SessionMessage>) -> Vec<SessionMessage> {
    let call_ids: HashSet<String> = messages
        .iter()
        .filter(|m| m.kind == "tool_use")
        .filter_map(|m| m.tool_use_id.clone())
        .collect();
    if call_ids.is_empty() {
        return messages;
    }

    // (text, is_error, exit_code) — multiple result blocks per call get
    // concatenated; any error marks the whole bundle so merge can append
    // `**Error**` (with the most recent non-zero exit code, when known).
    let mut output_by_id: HashMap<String, (String, bool, Option<i64>)> = HashMap::new();
    for msg in &messages {
        if !matches!(msg.kind.as_str(), "tool_result" | "tool_error") {
            continue;
        }
        let Some(id) = msg.tool_use_id.clone() else {
            continue;
        };
        if !call_ids.contains(&id) {
            continue;
        }
        let is_error = msg.kind == "tool_error";
        output_by_id
            .entry(id)
            .and_modify(|(text, err, code)| {
                text.push_str("\n\n");
                text.push_str(&msg.text);
                *err = *err || is_error;
                if msg.exit_code.is_some() {
                    *code = msg.exit_code;
                }
            })
            .or_insert_with(|| (msg.text.clone(), is_error, msg.exit_code));
    }

    messages
        .into_iter()
        .filter_map(|mut msg| match msg.kind.as_str() {
            "tool_result" | "tool_error" => {
                if let Some(id) = &msg.tool_use_id {
                    if output_by_id.contains_key(id) {
                        return None;
                    }
                }
                // Orphan result (no matching tool_use kept it). Prefix
                // with the same status marker we use for tool_use cards
                // so the user can scan failures at a glance.
                let prefix = status_marker(msg.kind == "tool_error");
                msg.text = format!("{prefix}{}", msg.text);
                Some(msg)
            }
            "tool_use" => {
                // Status marker BEFORE the tool name. `⏺` for success
                // (Claude TUI's BLACK_CIRCLE, constants/figures.ts:4),
                // `✗` for failure (Codex `exec_cell/render.rs:236-239`
                // failure badge). Either way it's the first visual signal.
                let matched = msg.tool_use_id.as_ref().and_then(|id| output_by_id.get(id));
                let failed = matched.is_some_and(|(_, err, _)| *err);
                let marker = status_marker(failed);
                msg.text = format!("{marker}{}", msg.text);

                if let Some((output, is_error, exit_code)) = matched {
                    // Build the merged body. On failures, prefix with
                    // `Error:` and the exit code (when known) — matching
                    // Claude Code TUI's `⎿ Error: Exit code N` line and
                    // unifying the format across platforms. Codex carries
                    // an exit_code, Claude doesn't (is_error alone signals
                    // failure).
                    let trimmed = output.trim();
                    let mut body = String::new();
                    if *is_error {
                        body.push_str("Error:");
                        // Codex: `Error: Exit code N` then (if any) the
                        // command output on subsequent lines.
                        // Claude: no exit_code — content comes inline
                        // after `Error: ` (single line head + multi-line
                        // continuation if the error message has \n).
                        match *exit_code {
                            Some(code) if code != 0 => {
                                body.push_str(&format!(" Exit code {code}"));
                                if !trimmed.is_empty() {
                                    body.push('\n');
                                    body.push_str(trimmed);
                                }
                            }
                            _ if !trimmed.is_empty() => {
                                body.push(' ');
                                body.push_str(trimmed);
                            }
                            _ => {}
                        }
                    } else {
                        body.push_str(trimmed);
                    }
                    if !body.is_empty() {
                        msg.text.push_str("\n\n");
                        // If the body ALREADY starts with a code fence
                        // (e.g. Claude's structuredPatch builder emits
                        // ```diff\n...\n```), append it verbatim so the
                        // markdown layer sees a single fence — wrapping
                        // it in another 4-backtick fence would make the
                        // inner fence render as literal characters.
                        // Otherwise, wrap in 4-backticks (handles
                        // triple-backtick content inside plain output).
                        if body.starts_with("```") {
                            msg.text.push_str(&body);
                        } else {
                            msg.text.push_str("````\n");
                            msg.text.push_str(&body);
                            msg.text.push_str("\n````");
                        }
                    }
                }
                Some(msg)
            }
            _ => Some(msg),
        })
        .collect()
}

// ===========================================================================
// Termory unified message format helpers
// ===========================================================================
//
// Goal: take heterogeneous tool/message data from 4 platforms (Claude /
// Codex / Gemini / OpenCode) and emit a single, consistent markdown body
// shape per content kind. **Content fidelity** is the constraint — every
// source field that has user-visible meaning gets a place in the output
// (either the primary template line, the body block, or the `- key: value`
// extras list below). Nothing is silently dropped, truncated, or renamed.
//
// Templates do NOT mimic any one CLI's visual style (Claude TUI bold,
// Codex magenta `$ `, OpenCode panel border, Gemini `> `/`✦ ` prefixes).
// Termory's rendering layer is markdown → HTML, so the templates use
// neutral markdown constructs (code fences, headings, bullet lists,
// blockquotes) that the renderer can style with CSS.

/// Symbolic constants for the SessionMessage.kind values used across this
/// module. The strings match what each platform's parser historically
/// emitted; the constants only exist to avoid bare string literals in
/// the parser code.
mod kind {
    pub const TEXT: &str = "message";
    pub const REASONING: &str = "reasoning";
    pub const TOOL_USE: &str = "tool_use";
    pub const TOOL_RESULT: &str = "tool_result";
    pub const TOOL_ERROR: &str = "tool_error";
    pub const COMPACTION: &str = "compaction";
    pub const LOCAL_COMMAND: &str = "local_command";
    pub const SHELL: &str = "shell";
    pub const COMMAND_EXECUTION: &str = "command_execution";
    pub const PLAN: &str = "plan";
    pub const AGENT_SWITCHED: &str = "agent-switched";
    pub const MODEL_SWITCHED: &str = "model-switched";
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SessionDetail {
    pub session: AppSession,
    pub messages: Vec<SessionMessage>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SearchHit {
    pub session: AppSession,
    pub snippet: String,
    pub role: String,
    pub match_count: usize,
}

pub fn search_sessions(query: &str) -> Result<Vec<SearchHit>, Box<dyn Error>> {
    let trimmed = query.trim();
    if trimmed.chars().count() < 2 {
        return Ok(Vec::new());
    }
    let needle = trimmed.to_lowercase();
    let sessions = scan_sessions()?;
    let mut hits = Vec::new();
    for session in sessions {
        let Ok(detail) = get_session(&session.source, &session.path, &session.id) else {
            continue;
        };
        let mut first: Option<(String, String)> = None;
        let mut match_count = 0usize;
        for message in &detail.messages {
            let lower = message.text.to_lowercase();
            let mut cursor = 0usize;
            while let Some(found) = lower[cursor..].find(&needle) {
                let pos = cursor + found;
                let end = pos + needle.len();
                if first.is_none()
                    && message.text.is_char_boundary(pos)
                    && message.text.is_char_boundary(end)
                {
                    first = Some((
                        message.role.clone(),
                        make_search_snippet(&message.text, pos, end),
                    ));
                }
                match_count += 1;
                if match_count >= 500 {
                    break;
                }
                cursor = end;
            }
            if match_count >= 500 {
                break;
            }
        }
        if let Some((role, snippet)) = first {
            hits.push(SearchHit {
                session: detail.session,
                snippet,
                role,
                match_count,
            });
        }
    }
    Ok(hits)
}

fn make_search_snippet(text: &str, match_start: usize, match_end: usize) -> String {
    let before_chars = 60usize;
    let after_chars = 100usize;

    let mut snippet_start = match_start;
    for (_, (b, _)) in (0..before_chars).zip(text[..match_start].char_indices().rev()) {
        snippet_start = b;
    }

    let mut snippet_end = text.len();
    for (taken, (b, c)) in text[match_end..].char_indices().enumerate() {
        if taken >= after_chars {
            snippet_end = match_end + b;
            break;
        }
        snippet_end = match_end + b + c.len_utf8();
    }

    let prefix = if snippet_start > 0 { "…" } else { "" };
    let suffix = if snippet_end < text.len() { "…" } else { "" };
    let core = collapse_whitespace(&text[snippet_start..snippet_end]);
    format!("{prefix}{}{suffix}", core.trim())
}

fn collapse_whitespace(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    let mut prev_space = false;
    for c in s.chars() {
        if c.is_whitespace() {
            if !prev_space {
                out.push(' ');
                prev_space = true;
            }
        } else {
            out.push(c);
            prev_space = false;
        }
    }
    out
}

pub fn scan_sessions() -> Result<Vec<AppSession>, Box<dyn Error>> {
    let mut sessions = Vec::new();
    sessions.extend(scan_codex()?);
    sessions.extend(scan_claude()?);
    sessions.extend(scan_gemini()?);
    sessions.extend(scan_opencode()?);
    let project_cwds: HashSet<String> = sessions
        .iter()
        .filter(|s| !s.project.is_empty() && Path::new(&s.project).is_absolute())
        .map(|s| s.project.clone())
        .collect();
    sessions.extend(scan_memory(&project_cwds)?);
    sessions.extend(scan_skills(&project_cwds)?);
    sessions.sort_by(|a, b| b.updated_at.cmp(&a.updated_at));
    Ok(sessions)
}

pub fn get_session(source: &str, path: &str, id: &str) -> Result<SessionDetail, Box<dyn Error>> {
    match source {
        "Codex" => parse_codex_session(Path::new(path), id),
        "Claude" => parse_claude_session(Path::new(path)),
        "Memory" => parse_doc_file(Path::new(path), "Memory"),
        "Skill" => parse_doc_file(Path::new(path), "Skill"),
        "Gemini" if path.ends_with(".jsonl") => parse_gemini_jsonl_session(Path::new(path)),
        "Gemini" if path.ends_with(".json") => parse_gemini_json_session(Path::new(path)),
        "OpenCode" if path.ends_with(".db") => parse_opencode_db_session(Path::new(path), id),
        "OpenCode" => parse_opencode_storage_session(Path::new(path)),
        _ => Err(format!("unsupported source: {source}").into()),
    }
}

fn scan_codex() -> Result<Vec<AppSession>, Box<dyn Error>> {
    let Some(home) = dirs::home_dir() else {
        return Ok(Vec::new());
    };
    let state_db = home.join(".codex").join("state_5.sqlite");
    if !state_db.exists() {
        return Ok(Vec::new());
    }
    scan_codex_state_db(&state_db)
}

fn scan_codex_state_db(path: &Path) -> Result<Vec<AppSession>, Box<dyn Error>> {
    let conn = Connection::open_with_flags(path, OpenFlags::SQLITE_OPEN_READ_ONLY)?;
    // Filter mirrors Codex's official thread listing:
    //   * codex-rs/state/src/runtime/threads.rs `push_thread_filters` requires
    //     `archived = 0` and `preview <> ''` for non-archived listings.
    //   * codex-rs/rollout/src/lib.rs `INTERACTIVE_SESSION_SOURCES` lists the
    //     four sources surfaced in the resume picker: cli, vscode, atlas,
    //     chatgpt. SessionSource is serialized with `rename_all = "lowercase"`,
    //     and `Custom("atlas")` / `Custom("chatgpt")` round-trip as their
    //     inner string ("atlas" / "chatgpt") in the `source` column.
    // The `preview` column was added in migration 0032_threads_preview.sql;
    // older state_5.sqlite files predate it, so we omit the filter when the
    // column is absent.
    let preview_clause = if column_exists(&conn, "threads", "preview")? {
        " and preview <> ''"
    } else {
        ""
    };
    let sql = format!(
        "select id, rollout_path, created_at, updated_at, cwd, title, first_user_message \
         from threads \
         where archived = 0{preview_clause} \
           and source in ('cli', 'vscode', 'atlas', 'chatgpt') \
         order by updated_at desc"
    );
    let mut stmt = conn.prepare(&sql)?;
    let rows = stmt.query_map([], |row| {
        let id: String = row.get(0)?;
        let rollout_path: String = row.get(1)?;
        let created_at: i64 = row.get(2)?;
        let updated_at: i64 = row.get(3)?;
        let cwd: String = row.get(4)?;
        let title: String = row.get(5).unwrap_or_default();
        let first_user_message: String = row.get(6).unwrap_or_default();
        let display_title = codex_display_title(&title)
            .or_else(|| codex_display_title(&first_user_message))
            .unwrap_or_default();
        let message_count = estimate_codex_message_count(Path::new(&rollout_path));

        Ok(AppSession {
            id: id.clone(),
            source: "Codex".to_string(),
            title: display_title,
            project: cwd,
            path: rollout_path.clone(),
            started_at: normalize_time(created_at.to_string()),
            updated_at: normalize_time(updated_at.to_string()),
            message_count,
            preview: String::new(),
            message_previews: Vec::new(),
        })
    })?;

    let mut sessions = Vec::new();
    for row in rows {
        let session = row?;
        if Path::new(&session.path).exists() {
            sessions.push(session);
        }
    }
    Ok(sessions)
}

fn scan_claude() -> Result<Vec<AppSession>, Box<dyn Error>> {
    let Some(home) = dirs::home_dir() else {
        return Ok(Vec::new());
    };
    let root = std::env::var("CLAUDE_CONFIG_DIR")
        .map(|dir| Path::new(&dir).join("projects"))
        .unwrap_or_else(|_| home.join(".claude").join("projects"));
    scan_claude_projects(&root)
}

fn scan_claude_projects(root: &Path) -> Result<Vec<AppSession>, Box<dyn Error>> {
    if !root.exists() {
        return Ok(Vec::new());
    }

    let mut sessions = Vec::new();
    for project_entry in fs::read_dir(root)? {
        let project_entry = project_entry?;
        let project_dir = project_entry.path();
        if !project_dir.is_dir() {
            continue;
        }
        for entry in fs::read_dir(project_dir)? {
            let entry = entry?;
            let path = entry.path();
            if !path.is_file() || !is_claude_session_file(&path) {
                continue;
            }
            if let Ok(session) = parse_claude_lite_session(&path, None) {
                sessions.push(session);
            }
        }
    }

    let mut latest_by_id: HashMap<String, AppSession> = HashMap::new();
    for session in sessions {
        let replace = latest_by_id
            .get(&session.id)
            .map(|current| session.updated_at > current.updated_at)
            .unwrap_or(true);
        if replace {
            latest_by_id.insert(session.id.clone(), session);
        }
    }
    let mut sessions = latest_by_id.into_values().collect::<Vec<_>>();
    sessions.sort_by(|a, b| {
        b.updated_at
            .cmp(&a.updated_at)
            .then_with(|| b.id.cmp(&a.id))
    });
    Ok(sessions)
}

fn scan_gemini() -> Result<Vec<AppSession>, Box<dyn Error>> {
    let Some(home) = dirs::home_dir() else {
        return Ok(Vec::new());
    };
    let tmp_dir = home.join(".gemini").join("tmp");
    if !tmp_dir.exists() {
        return Ok(Vec::new());
    }
    let mut sessions = Vec::new();
    for entry in fs::read_dir(tmp_dir)? {
        let entry = entry?;
        let path = entry.path();
        if !path.is_dir() {
            continue;
        }
        let chats_dir = path.join("chats");
        if chats_dir.exists() {
            sessions.extend(scan_gemini_chats_dir(&chats_dir)?);
        }
    }

    let mut latest_by_id: HashMap<String, AppSession> = HashMap::new();
    for session in sessions {
        let replace = latest_by_id
            .get(&session.id)
            .map(|current| session.updated_at > current.updated_at)
            .unwrap_or(true);
        if replace {
            latest_by_id.insert(session.id.clone(), session);
        }
    }
    let mut sessions = latest_by_id.into_values().collect::<Vec<_>>();
    sessions.sort_by(|a, b| a.started_at.cmp(&b.started_at));
    Ok(sessions)
}

fn scan_opencode() -> Result<Vec<AppSession>, Box<dyn Error>> {
    let Some(home) = dirs::home_dir() else {
        return Ok(Vec::new());
    };
    let root = home.join(".local").join("share").join("opencode");
    if !root.exists() {
        return Ok(Vec::new());
    }

    let mut sessions = Vec::new();
    let db = root.join("opencode.db");
    if db.exists() {
        sessions.extend(scan_opencode_db(&db)?);
    }
    for storage_root in opencode_storage_roots(&root) {
        sessions.extend(scan_opencode_storage(&storage_root)?);
    }

    let mut seen = HashSet::new();
    sessions.retain(|session| seen.insert(session.id.clone()));
    Ok(sessions)
}

fn scan_memory(project_cwds: &HashSet<String>) -> Result<Vec<AppSession>, Box<dyn Error>> {
    let mut sessions = Vec::new();
    sessions.extend(scan_claude_memory()?);
    sessions.extend(scan_claude_rules(project_cwds));
    sessions.extend(scan_codex_memory());
    sessions.extend(scan_gemini_memory());
    sessions.extend(scan_global_instructions());

    // Per the AGENTS.md spec the file is always usable by both Codex and
    // OpenCode, so project-level AGENTS.md is tagged with both regardless of
    // which tool had a session in the cwd.

    // Codex, Gemini CLI, and OpenCode all gate their ancestor walk on the
    // presence of a project-root marker (`.git` by default). Verified against
    // each tool's source:
    //   * Codex (codex-rs/core/src/agents_md.rs): "if no marker is found, only
    //     the current working directory is considered".
    //   * Gemini (packages/core/src/utils/memoryDiscovery.ts findProjectRoot):
    //     defaults to ['.git']; when null, ceiling = startDir → walk no-ops.
    //   * OpenCode (packages/opencode/src/project/project.ts): worktree comes
    //     from `git rev-parse --git-common-dir`; without git the worktree
    //     equals the cwd and findUp does not ascend.
    // We mirror that: scan cwd, then only ascend if a .git is found at cwd or
    // any ancestor up to (but not including) $HOME. Walk stops at the git root
    // (inclusive).
    let home_for_walk = dirs::home_dir();
    for cwd in project_cwds {
        let cwd_path = Path::new(cwd);
        push_project_root_instruction_files(cwd_path, &mut sessions);

        let git_root = find_git_root(cwd_path, home_for_walk.as_deref());
        if let Some(git_root) = git_root {
            if git_root != cwd_path {
                let mut current = cwd_path.parent().map(|p| p.to_path_buf());
                while let Some(dir) = current {
                    if home_for_walk.as_deref() == Some(dir.as_path()) {
                        break;
                    }
                    push_ancestor_instruction_files(&dir, &mut sessions);
                    if dir == git_root {
                        break;
                    }
                    current = match dir.parent() {
                        Some(parent) if parent != dir => Some(parent.to_path_buf()),
                        _ => None,
                    };
                }
            }
        }
    }

    // Deduplicate by path — one entry per file even if multiple tools support it
    // (the tool list is encoded into preview).
    let mut seen = HashSet::new();
    sessions.retain(|s| seen.insert(s.path.clone()));
    Ok(sessions)
}

fn scan_global_instructions() -> Vec<AppSession> {
    let mut sessions = Vec::new();
    let Some(home) = dirs::home_dir() else {
        return sessions;
    };

    let claude_root = std::env::var("CLAUDE_CONFIG_DIR")
        .map(std::path::PathBuf::from)
        .unwrap_or_else(|_| home.join(".claude"));
    // OpenCode officially falls back to ~/.claude/CLAUDE.md and project
    // CLAUDE.md when no AGENTS.md is found, per https://opencode.ai/docs/rules/
    push_tagged_instruction_file(
        &claude_root.join("CLAUDE.md"),
        "~/.claude",
        &["claude", "opencode"],
        &mut sessions,
    );

    let codex_dir = home.join(".codex");
    push_tagged_instruction_file(
        &codex_dir.join("AGENTS.md"),
        "~/.codex",
        &["codex"],
        &mut sessions,
    );
    push_tagged_instruction_file(
        &codex_dir.join("AGENTS.override.md"),
        "~/.codex",
        &["codex"],
        &mut sessions,
    );

    let opencode_config = home.join(".config").join("opencode");
    push_tagged_instruction_file(
        &opencode_config.join("AGENTS.md"),
        "~/.config/opencode",
        &["opencode"],
        &mut sessions,
    );

    sessions
}

// Walks up from `start` looking for a `.git` entry. Returns the first
// directory that contains one (the project root). Stops before $HOME (never
// returns the home directory itself) and at the filesystem root. Mirrors
// Codex's default `project_root_markers = [".git"]` lookup.
fn find_git_root(start: &Path, home: Option<&Path>) -> Option<std::path::PathBuf> {
    let mut cursor = start.to_path_buf();
    loop {
        if home == Some(cursor.as_path()) {
            return None;
        }
        if cursor.join(".git").exists() {
            return Some(cursor);
        }
        match cursor.parent() {
            Some(parent) if parent != cursor => cursor = parent.to_path_buf(),
            _ => return None,
        }
    }
}

// Instruction files at the project root (the session cwd). The .claude/CLAUDE.md
// layout only applies at the project root per Claude Code docs, not at every
// ancestor.
fn push_project_root_instruction_files(dir: &Path, sessions: &mut Vec<AppSession>) {
    let label = dir.to_string_lossy().to_string();
    push_tagged_instruction_file(
        &dir.join("CLAUDE.md"),
        &label,
        &["claude", "opencode"],
        sessions,
    );
    push_tagged_instruction_file(&dir.join("CLAUDE.local.md"), &label, &["claude"], sessions);
    push_tagged_instruction_file(
        &dir.join(".claude").join("CLAUDE.md"),
        &label,
        &["claude"],
        sessions,
    );
    push_tagged_instruction_file(&dir.join("GEMINI.md"), &label, &["gemini"], sessions);
    // MEMORY.md is Gemini's modern context file (legacy GEMINI.md alias)
    // per packages/core/src/tools/memoryTool.ts:12.
    push_tagged_instruction_file(&dir.join("MEMORY.md"), &label, &["gemini"], sessions);
    push_tagged_instruction_file(
        &dir.join("AGENTS.md"),
        &label,
        &["codex", "opencode"],
        sessions,
    );
    // AGENTS.override.md is Codex-only per
    // https://developers.openai.com/codex/guides/agents-md
    push_tagged_instruction_file(
        &dir.join("AGENTS.override.md"),
        &label,
        &["codex"],
        sessions,
    );
}

// Instruction files at an ancestor directory (above the session cwd, up the
// walk). Same set as project-root EXCEPT for .claude/CLAUDE.md, which is a
// project-root-only convention.
fn push_ancestor_instruction_files(dir: &Path, sessions: &mut Vec<AppSession>) {
    let label = dir.to_string_lossy().to_string();
    push_tagged_instruction_file(
        &dir.join("CLAUDE.md"),
        &label,
        &["claude", "opencode"],
        sessions,
    );
    push_tagged_instruction_file(&dir.join("CLAUDE.local.md"), &label, &["claude"], sessions);
    push_tagged_instruction_file(&dir.join("GEMINI.md"), &label, &["gemini"], sessions);
    push_tagged_instruction_file(&dir.join("MEMORY.md"), &label, &["gemini"], sessions);
    push_tagged_instruction_file(
        &dir.join("AGENTS.md"),
        &label,
        &["codex", "opencode"],
        sessions,
    );
    push_tagged_instruction_file(
        &dir.join("AGENTS.override.md"),
        &label,
        &["codex"],
        sessions,
    );
}

fn push_tagged_instruction_file(
    path: &Path,
    project_label: &str,
    tools: &[&str],
    sessions: &mut Vec<AppSession>,
) {
    if !path.is_file() {
        return;
    }
    if let Some(mut session) = memory_session_from_file(path, project_label) {
        session.preview = tools.join(",");
        sessions.push(session);
    }
}

// Claude Code "rules" are personal/project-wide markdown instructions per
// https://code.claude.com/docs/en/memory. `~/.claude/rules/**/*.md` apply
// globally, `<cwd>/.claude/rules/**/*.md` apply to that project. All .md files
// are discovered recursively.
fn scan_claude_rules(project_cwds: &HashSet<String>) -> Vec<AppSession> {
    let mut sessions = Vec::new();
    let Some(home) = dirs::home_dir() else {
        return sessions;
    };

    let claude_root = std::env::var("CLAUDE_CONFIG_DIR")
        .map(std::path::PathBuf::from)
        .unwrap_or_else(|_| home.join(".claude"));

    let global_dir = claude_root.join("rules");
    if global_dir.is_dir() {
        push_doc_files_recursive(
            &global_dir,
            &global_dir,
            "~/.claude/rules",
            "claude",
            "Memory",
            &[],
            &mut sessions,
        );
    }

    for cwd in project_cwds {
        let project_dir = Path::new(cwd).join(".claude").join("rules");
        if project_dir.is_dir() {
            push_doc_files_recursive(
                &project_dir,
                &project_dir,
                cwd,
                "claude",
                "Memory",
                &[],
                &mut sessions,
            );
        }
    }

    sessions
}

fn scan_claude_memory() -> Result<Vec<AppSession>, Box<dyn Error>> {
    let Some(home) = dirs::home_dir() else {
        return Ok(Vec::new());
    };
    let root = std::env::var("CLAUDE_CONFIG_DIR")
        .map(|dir| Path::new(&dir).join("projects"))
        .unwrap_or_else(|_| home.join(".claude").join("projects"));
    if !root.exists() {
        return Ok(Vec::new());
    }
    let mut sessions = Vec::new();
    for project_entry in fs::read_dir(&root)? {
        let project_entry = project_entry?;
        let project_dir = project_entry.path();
        if !project_dir.is_dir() {
            continue;
        }
        let memory_dir = project_dir.join("memory");
        if !memory_dir.is_dir() {
            continue;
        }
        let slug = project_dir
            .file_name()
            .and_then(|n| n.to_str())
            .unwrap_or_default()
            .to_string();
        let project_name = decode_claude_project_slug(&slug);
        push_memory_files_recursive(
            &memory_dir,
            &memory_dir,
            &project_name,
            "claude",
            &mut sessions,
        );
    }
    Ok(sessions)
}

fn scan_codex_memory() -> Vec<AppSession> {
    let Some(home) = dirs::home_dir() else {
        return Vec::new();
    };
    let root = home.join(".codex").join("memories");
    if !root.is_dir() {
        return Vec::new();
    }
    let mut sessions = Vec::new();
    push_doc_files_recursive(
        &root,
        &root,
        "~/.codex/memories",
        "codex",
        "Memory",
        &["skills"],
        &mut sessions,
    );
    sessions
}

fn scan_gemini_memory() -> Vec<AppSession> {
    let Some(home) = dirs::home_dir() else {
        return Vec::new();
    };
    let mut sessions = Vec::new();

    // Global memory: ~/.gemini/{GEMINI.md,MEMORY.md}. Both filenames are
    // returned by getAllGeminiMdFilenames() in Gemini's source
    // (packages/core/src/tools/memoryTool.ts:11-12 and
    // packages/core/src/utils/memoryDiscovery.ts:323).
    let gemini_dir = home.join(".gemini");
    for filename in ["GEMINI.md", "MEMORY.md"] {
        let global_file = gemini_dir.join(filename);
        if global_file.is_file() {
            if let Some(mut session) = memory_session_from_file(&global_file, "~/.gemini") {
                session.preview = "gemini".to_string();
                sessions.push(session);
            }
        }
    }

    // Per-project memory: ~/.gemini/tmp/<id>/memory/{MEMORY.md preferred,
    // GEMINI.md legacy fallback}. Confirmed via
    // packages/core/src/config/storage.ts getProjectMemoryDir() →
    // getProjectMemoryTempDir() = path.join(getProjectTempDir(), 'memory').
    // The skills/ subdir is surfaced under Skills (scan_gemini_skills), so
    // it is skipped here.
    let tmp_dir = gemini_dir.join("tmp");
    if tmp_dir.is_dir() {
        if let Ok(entries) = fs::read_dir(&tmp_dir) {
            for entry in entries.flatten() {
                let project_dir = entry.path();
                if !project_dir.is_dir() {
                    continue;
                }
                let memory_dir = project_dir.join("memory");
                if !memory_dir.is_dir() {
                    continue;
                }
                let project_label = gemini_project_label(&project_dir);
                push_doc_files_recursive(
                    &memory_dir,
                    &memory_dir,
                    &project_label,
                    "gemini",
                    "Memory",
                    &["skills"],
                    &mut sessions,
                );
            }
        }
    }

    sessions
}

fn gemini_project_label(project_dir: &Path) -> String {
    let project_root = project_dir.join(".project_root");
    if let Ok(raw) = fs::read_to_string(&project_root) {
        let trimmed = raw.trim();
        if !trimmed.is_empty() {
            return trimmed.to_string();
        }
    }
    project_dir
        .file_name()
        .and_then(|s| s.to_str())
        .unwrap_or_default()
        .to_string()
}

fn scan_skills(project_cwds: &HashSet<String>) -> Result<Vec<AppSession>, Box<dyn Error>> {
    let mut sessions = Vec::new();
    sessions.extend(scan_claude_skills(project_cwds));
    sessions.extend(scan_codex_skills(project_cwds));
    sessions.extend(scan_gemini_skills(project_cwds));
    sessions.extend(scan_opencode_skills(project_cwds));
    sessions.extend(scan_agents_skills(project_cwds));

    let mut seen = HashSet::new();
    sessions.retain(|s| seen.insert(s.path.clone()));
    Ok(sessions)
}

fn scan_claude_skills(project_cwds: &HashSet<String>) -> Vec<AppSession> {
    let mut sessions = Vec::new();
    let Some(home) = dirs::home_dir() else {
        return sessions;
    };

    // OpenCode officially also reads .claude/skills/, so tag with both tools.
    // (https://opencode.ai/docs/skills/)
    let tag = "claude,opencode";

    let claude_root = std::env::var("CLAUDE_CONFIG_DIR")
        .map(std::path::PathBuf::from)
        .unwrap_or_else(|_| home.join(".claude"));
    let global_dir = claude_root.join("skills");
    if global_dir.is_dir() {
        push_doc_files_recursive(
            &global_dir,
            &global_dir,
            "~/.claude/skills",
            tag,
            "Skill",
            &[],
            &mut sessions,
        );
    }

    for cwd in project_cwds {
        let project_dir = Path::new(cwd).join(".claude").join("skills");
        if project_dir.is_dir() {
            push_doc_files_recursive(
                &project_dir,
                &project_dir,
                cwd,
                tag,
                "Skill",
                &[],
                &mut sessions,
            );
        }
    }

    sessions
}

fn scan_codex_skills(project_cwds: &HashSet<String>) -> Vec<AppSession> {
    let mut sessions = Vec::new();
    let Some(home) = dirs::home_dir() else {
        return sessions;
    };

    // Codex stores user skills at $CODEX_HOME/skills/, verified in
    // codex-rs/core/src/session/tests.rs (`codex_home.join("skills")`) and
    // codex-rs/core/tests/suite/compact_remote_parity.rs
    // (`<CODEX_HOME>/skills/.system/imagegen/SKILL.md`).
    let global_dir = home.join(".codex").join("skills");
    if global_dir.is_dir() {
        push_doc_files_recursive(
            &global_dir,
            &global_dir,
            "~/.codex/skills",
            "codex",
            "Skill",
            &[],
            &mut sessions,
        );
    }

    for cwd in project_cwds {
        let project_dir = Path::new(cwd).join(".codex").join("skills");
        if project_dir.is_dir() {
            push_doc_files_recursive(
                &project_dir,
                &project_dir,
                cwd,
                "codex",
                "Skill",
                &[],
                &mut sessions,
            );
        }
    }

    sessions
}

fn scan_gemini_skills(project_cwds: &HashSet<String>) -> Vec<AppSession> {
    let mut sessions = Vec::new();
    let Some(home) = dirs::home_dir() else {
        return sessions;
    };

    let global_dir = home.join(".gemini").join("skills");
    if global_dir.is_dir() {
        push_doc_files_recursive(
            &global_dir,
            &global_dir,
            "~/.gemini/skills",
            "gemini",
            "Skill",
            &[],
            &mut sessions,
        );
    }

    // Per-project skills: ~/.gemini/tmp/<id>/memory/skills/. The Gemini docs
    // are misleading here — they mention tmp/<hash>/ as chat checkpoints — but
    // the source code (packages/core/src/config/storage.ts) clearly defines
    // getProjectSkillsMemoryDir() as path.join(getProjectMemoryTempDir(),
    // 'skills'), which resolves to this exact path.
    let tmp_dir = home.join(".gemini").join("tmp");
    if tmp_dir.is_dir() {
        if let Ok(entries) = fs::read_dir(&tmp_dir) {
            for entry in entries.flatten() {
                let project_dir = entry.path();
                if !project_dir.is_dir() {
                    continue;
                }
                let skills_dir = project_dir.join("memory").join("skills");
                if !skills_dir.is_dir() {
                    continue;
                }
                let project_label = gemini_project_label(&project_dir);
                push_doc_files_recursive(
                    &skills_dir,
                    &skills_dir,
                    &project_label,
                    "gemini",
                    "Skill",
                    &[],
                    &mut sessions,
                );
            }
        }
    }

    for cwd in project_cwds {
        let project_dir = Path::new(cwd).join(".gemini").join("skills");
        if project_dir.is_dir() {
            push_doc_files_recursive(
                &project_dir,
                &project_dir,
                cwd,
                "gemini",
                "Skill",
                &[],
                &mut sessions,
            );
        }
    }

    sessions
}

fn scan_opencode_skills(project_cwds: &HashSet<String>) -> Vec<AppSession> {
    let mut sessions = Vec::new();
    let Some(home) = dirs::home_dir() else {
        return sessions;
    };

    let global_dir = home.join(".config").join("opencode").join("skills");
    if global_dir.is_dir() {
        push_doc_files_recursive(
            &global_dir,
            &global_dir,
            "~/.config/opencode/skills",
            "opencode",
            "Skill",
            &[],
            &mut sessions,
        );
    }

    for cwd in project_cwds {
        let project_dir = Path::new(cwd).join(".opencode").join("skills");
        if project_dir.is_dir() {
            push_doc_files_recursive(
                &project_dir,
                &project_dir,
                cwd,
                "opencode",
                "Skill",
                &[],
                &mut sessions,
            );
        }
    }

    sessions
}

// Tool-neutral skills location. Codex, Gemini CLI, and OpenCode all officially
// read from ~/.agents/skills/ (global) and <cwd>/.agents/skills/ (project) as
// an interoperable, cross-tool path. Claude Code does not currently read from
// this location.
fn scan_agents_skills(project_cwds: &HashSet<String>) -> Vec<AppSession> {
    let mut sessions = Vec::new();
    let Some(home) = dirs::home_dir() else {
        return sessions;
    };

    let tag = "codex,gemini,opencode";

    let global_dir = home.join(".agents").join("skills");
    if global_dir.is_dir() {
        push_doc_files_recursive(
            &global_dir,
            &global_dir,
            "~/.agents/skills",
            tag,
            "Skill",
            &[],
            &mut sessions,
        );
    }

    for cwd in project_cwds {
        let project_dir = Path::new(cwd).join(".agents").join("skills");
        if project_dir.is_dir() {
            push_doc_files_recursive(
                &project_dir,
                &project_dir,
                cwd,
                tag,
                "Skill",
                &[],
                &mut sessions,
            );
        }
    }

    sessions
}

fn push_memory_files_recursive(
    dir: &Path,
    base: &Path,
    project_label: &str,
    tool_tag: &str,
    sessions: &mut Vec<AppSession>,
) {
    push_doc_files_recursive(dir, base, project_label, tool_tag, "Memory", &[], sessions);
}

fn push_doc_files_recursive(
    dir: &Path,
    base: &Path,
    project_label: &str,
    tool_tag: &str,
    source: &str,
    skip_dirs: &[&str],
    sessions: &mut Vec<AppSession>,
) {
    let Ok(entries) = fs::read_dir(dir) else {
        return;
    };
    for entry in entries.flatten() {
        let path = entry.path();
        let name = entry.file_name();
        let name_str = name.to_string_lossy();
        if name_str == ".git" || name_str.starts_with('.') {
            continue;
        }
        if path.is_dir() {
            if skip_dirs.iter().any(|d| name_str == *d) {
                continue;
            }
            push_doc_files_recursive(
                &path,
                base,
                project_label,
                tool_tag,
                source,
                skip_dirs,
                sessions,
            );
            continue;
        }
        if !path.is_file() {
            continue;
        }
        let ext_match = path
            .extension()
            .and_then(|e| e.to_str())
            .map(|e| e.eq_ignore_ascii_case("md"))
            .unwrap_or(false);
        if !ext_match {
            continue;
        }
        let Some(mut session) = doc_session_from_file(&path, project_label, source) else {
            continue;
        };
        session.preview = tool_tag.to_string();
        if let Ok(rel) = path.strip_prefix(base) {
            let rel_str = rel.to_string_lossy().to_string();
            if !rel_str.is_empty() {
                session.title = rel_str.clone();
                session.id = rel_str;
            }
        }
        sessions.push(session);
    }
}

fn decode_claude_project_slug(slug: &str) -> String {
    if slug.is_empty() {
        return String::new();
    }
    if let Some(rest) = slug.strip_prefix('-') {
        format!("/{}", rest.replace('-', "/"))
    } else {
        slug.replace('-', "/")
    }
}

fn memory_session_from_file(path: &Path, project: &str) -> Option<AppSession> {
    doc_session_from_file(path, project, "Memory")
}

fn doc_session_from_file(path: &Path, project: &str, source: &str) -> Option<AppSession> {
    let raw = fs::read_to_string(path).ok()?;
    let (front, body) = split_memory_frontmatter(&raw);
    let body_is_empty = body.trim().is_empty();
    let name = front
        .as_ref()
        .and_then(|f| f.get("name").cloned())
        .filter(|s| !s.is_empty());
    let description = front
        .as_ref()
        .and_then(|f| f.get("description").cloned())
        .unwrap_or_default();
    let mem_type = front
        .as_ref()
        .and_then(|f| f.get("type").cloned())
        .unwrap_or_default();
    let file_name = path
        .file_name()
        .and_then(|s| s.to_str())
        .unwrap_or("memory")
        .to_string();
    let title = name.clone().unwrap_or_else(|| file_name.clone());
    let id = name.unwrap_or_else(|| file_name.clone());
    let updated_at = file_time(path, true);
    let started_at = file_time(path, false);
    let preview = if !description.is_empty() {
        description
    } else if !mem_type.is_empty() {
        mem_type
    } else {
        memory_tool_for_file(&file_name).to_string()
    };
    Some(AppSession {
        id,
        source: source.to_string(),
        title,
        project: project.to_string(),
        path: path.to_string_lossy().to_string(),
        started_at,
        updated_at,
        message_count: if body_is_empty { 0 } else { 1 },
        preview,
        message_previews: Vec::new(),
    })
}

fn memory_tool_for_file(file_name: &str) -> &'static str {
    let lower = file_name.to_ascii_lowercase();
    if lower.starts_with("claude") {
        "claude"
    } else if lower.starts_with("agents") {
        "agents"
    } else if lower.starts_with("gemini") {
        "gemini"
    } else if lower.ends_with(".rules") {
        "rules"
    } else {
        "memory"
    }
}

fn split_memory_frontmatter(
    raw: &str,
) -> (Option<std::collections::HashMap<String, String>>, String) {
    let trimmed = raw.trim_start_matches('\u{feff}');
    if !trimmed.starts_with("---") {
        return (None, raw.to_string());
    }
    let after_open = match trimmed.strip_prefix("---") {
        Some(rest) => rest.trim_start_matches('\n').trim_start_matches("\r\n"),
        None => return (None, raw.to_string()),
    };
    let Some(close_pos) = after_open.find("\n---") else {
        return (None, raw.to_string());
    };
    let yaml_block = &after_open[..close_pos];
    let body_start = close_pos + 4;
    let body = after_open[body_start..]
        .trim_start_matches('\n')
        .trim_start_matches("\r\n")
        .to_string();
    let mut map = std::collections::HashMap::new();
    for line in yaml_block.lines() {
        let stripped = line.trim_end_matches('\r');
        let key_value = stripped.trim_start();
        if key_value.is_empty() || key_value.starts_with('#') {
            continue;
        }
        if let Some((key, value)) = key_value.split_once(':') {
            let key = key.trim().to_string();
            let value = value
                .trim()
                .trim_matches('"')
                .trim_matches('\'')
                .to_string();
            if !value.is_empty() {
                map.insert(key, value);
            }
        }
    }
    (Some(map), body)
}

fn parse_doc_file(path: &Path, source: &str) -> Result<SessionDetail, Box<dyn Error>> {
    let raw = fs::read_to_string(path)?;
    let (front, body) = split_memory_frontmatter(&raw);
    let file_name = path
        .file_name()
        .and_then(|s| s.to_str())
        .unwrap_or("doc")
        .to_string();
    let project = derive_memory_project_label(path);
    let name = front
        .as_ref()
        .and_then(|f| f.get("name").cloned())
        .filter(|s| !s.is_empty())
        .unwrap_or_else(|| file_name.clone());
    let mem_type = front
        .as_ref()
        .and_then(|f| f.get("type").cloned())
        .unwrap_or_default();
    let description = front
        .as_ref()
        .and_then(|f| f.get("description").cloned())
        .unwrap_or_default();
    let updated_at = file_time(path, true);
    let started_at = file_time(path, false);

    let body_text = body.trim();
    let display_body = if body_text.is_empty() {
        if description.is_empty() {
            "(empty)".to_string()
        } else {
            description.clone()
        }
    } else {
        body_text.to_string()
    };

    let session = AppSession {
        id: name.clone(),
        source: source.to_string(),
        title: name,
        project,
        path: path.to_string_lossy().to_string(),
        started_at: started_at.clone(),
        updated_at: updated_at.clone(),
        message_count: 1,
        preview: description,
        message_previews: Vec::new(),
    };
    let role = if !mem_type.is_empty() {
        mem_type
    } else {
        memory_tool_for_file(&file_name).to_string()
    };
    let kind = if source == "Skill" { "skill" } else { "memory" };
    let messages = vec![SessionMessage {
        role,
        text: display_body,
        timestamp: updated_at,
        kind: kind.to_string(),
        tool_use_id: None,
        exit_code: None,
    }];
    Ok(SessionDetail { session, messages })
}

fn derive_memory_project_label(path: &Path) -> String {
    let Some(home) = dirs::home_dir() else {
        return String::new();
    };

    let claude_root = std::env::var("CLAUDE_CONFIG_DIR")
        .map(std::path::PathBuf::from)
        .unwrap_or_else(|_| home.join(".claude"));

    // Claude global skills: <claude_root>/skills/<name>/...
    if path.starts_with(claude_root.join("skills")) {
        return "~/.claude/skills".to_string();
    }

    // Claude global rules: <claude_root>/rules/**
    if path.starts_with(claude_root.join("rules")) {
        return "~/.claude/rules".to_string();
    }

    // Claude structured memory: <claude_root>/projects/<slug>/memory/.../<file>.md
    let claude_projects_root = claude_root.join("projects");
    if let Ok(rel) = path.strip_prefix(&claude_projects_root) {
        if let Some(slug) = rel.iter().next().and_then(|c| c.to_str()) {
            return decode_claude_project_slug(slug);
        }
    }

    // Codex global skills: ~/.codex/skills/**
    if path.starts_with(home.join(".codex").join("skills")) {
        return "~/.codex/skills".to_string();
    }

    // Codex global memory: ~/.codex/memories/**
    if path.starts_with(home.join(".codex").join("memories")) {
        return "~/.codex/memories".to_string();
    }

    // Gemini global skills: ~/.gemini/skills/**
    if path.starts_with(home.join(".gemini").join("skills")) {
        return "~/.gemini/skills".to_string();
    }

    // Gemini per-project memory/skills: ~/.gemini/tmp/<id>/{memory,skills}/**
    let gemini_tmp = home.join(".gemini").join("tmp");
    if let Ok(rel) = path.strip_prefix(&gemini_tmp) {
        if let Some(project_id) = rel.iter().next() {
            let project_dir = gemini_tmp.join(project_id);
            return gemini_project_label(&project_dir);
        }
    }

    // OpenCode global skills: ~/.config/opencode/skills/**
    if path.starts_with(home.join(".config").join("opencode").join("skills")) {
        return "~/.config/opencode/skills".to_string();
    }

    // Tool-neutral global skills: ~/.agents/skills/**
    if path.starts_with(home.join(".agents").join("skills")) {
        return "~/.agents/skills".to_string();
    }

    // Global instruction files
    if path == claude_root.join("CLAUDE.md") || path == claude_root.join("CLAUDE.local.md") {
        return "~/.claude".to_string();
    }
    let codex_dir = home.join(".codex");
    if path == codex_dir.join("AGENTS.md") || path == codex_dir.join("instructions.md") {
        return "~/.codex".to_string();
    }
    if path == home.join(".gemini").join("GEMINI.md") {
        return "~/.gemini".to_string();
    }
    let opencode_config = home.join(".config").join("opencode");
    if path == opencode_config.join("AGENTS.md") || path == opencode_config.join("AGENTS.local.md")
    {
        return "~/.config/opencode".to_string();
    }

    // Project-level skill/rule files: <cwd>/.{claude,codex,gemini,opencode,agents}/{skills,rules}/<name>/...
    // Walk up the path; if we cross a "skills" or "rules" dir whose parent is a
    // known tool dotdir, the grandparent of the tool dotdir is the project cwd.
    let mut cursor = path.parent();
    while let Some(dir) = cursor {
        let dir_name = dir.file_name().and_then(|s| s.to_str()).unwrap_or("");
        if dir_name == "skills" || dir_name == "rules" {
            if let Some(tool_dir) = dir.parent() {
                let is_tool_dot = tool_dir
                    .file_name()
                    .and_then(|s| s.to_str())
                    .map(|n| {
                        matches!(
                            n,
                            ".claude" | ".codex" | ".gemini" | ".opencode" | ".agents"
                        )
                    })
                    .unwrap_or(false);
                if is_tool_dot {
                    if let Some(cwd) = tool_dir.parent() {
                        return cwd.to_string_lossy().to_string();
                    }
                }
            }
            break;
        }
        cursor = dir.parent();
    }

    // Project-level instruction files. Strip a trailing ".claude" wrapper if any.
    let parent = path.parent().unwrap_or(path);
    if parent
        .file_name()
        .and_then(|s| s.to_str())
        .map(|n| n == ".claude")
        .unwrap_or(false)
    {
        if let Some(grand) = parent.parent() {
            return grand.to_string_lossy().to_string();
        }
    }
    parent.to_string_lossy().to_string()
}

#[derive(Default)]
struct ClaudeConversation {
    session_id: String,
    project: String,
    timestamps: Vec<String>,
    custom_title: Option<String>,
    ai_title: Option<String>,
    last_prompt: Option<String>,
    agent_name: Option<String>,
    summary: Option<String>,
    first_prompt: Option<String>,
    command_prompt_fallback: Option<String>,
    team_name: Option<String>,
    messages: Vec<SessionMessage>,
    visible_message_count: usize,
}

struct ClaudeLiteSessionFile {
    head: String,
    tail: String,
    mtime: SystemTime,
}

fn parse_claude_lite_session(
    path: &Path,
    project_path: Option<&str>,
) -> Result<AppSession, Box<dyn Error>> {
    let session_id = path
        .file_stem()
        .and_then(|name| name.to_str())
        .ok_or("missing Claude session id")?
        .to_string();
    let lite = read_claude_lite_session(path).ok_or("unreadable Claude session")?;
    let first_line = lite.head.lines().next().unwrap_or_default();
    if first_line.contains("\"isSidechain\":true") || first_line.contains("\"isSidechain\": true") {
        return Err("Claude sidechain session is hidden from resume".into());
    }

    let custom_or_ai_title = extract_last_json_string_field(&lite.tail, "customTitle")
        .or_else(|| extract_last_json_string_field(&lite.head, "customTitle"))
        .or_else(|| extract_last_json_string_field(&lite.tail, "aiTitle"))
        .or_else(|| extract_last_json_string_field(&lite.head, "aiTitle"));
    let first_prompt = extract_claude_first_prompt_from_head(&lite.head);
    let summary = custom_or_ai_title
        .or_else(|| extract_last_json_string_field(&lite.tail, "lastPrompt"))
        .or_else(|| extract_last_json_string_field(&lite.tail, "summary"))
        .or(first_prompt)
        .filter(|summary| !summary.trim().is_empty())
        .ok_or("Claude metadata-only session is hidden from resume")?;
    let project = extract_json_string_field(&lite.head, "cwd")
        .or_else(|| project_path.map(ToString::to_string))
        .unwrap_or_default();

    Ok(AppSession {
        id: session_id.clone(),
        source: "Claude".to_string(),
        title: official_title_from_text(&strip_display_tags(&summary)).unwrap_or(summary),
        project,
        path: path.display().to_string(),
        started_at: extract_json_string_field(&lite.head, "timestamp")
            .and_then(normalize_time)
            .or_else(|| file_time(path, false)),
        updated_at: Some(system_time_to_iso(lite.mtime)),
        message_count: estimate_claude_message_count(path),
        preview: String::new(),
        message_previews: Vec::new(),
    })
}

fn read_claude_lite_session(path: &Path) -> Option<ClaudeLiteSessionFile> {
    let mut file = fs::File::open(path).ok()?;
    let metadata = file.metadata().ok()?;
    if metadata.len() == 0 {
        return None;
    }
    let mut buffer = vec![0_u8; CLAUDE_LITE_READ_BUF_SIZE];
    let head_len = file.read(&mut buffer).ok()?;
    if head_len == 0 {
        return None;
    }
    let head = String::from_utf8_lossy(&buffer[..head_len]).to_string();

    let tail = if metadata.len() as usize <= CLAUDE_LITE_READ_BUF_SIZE {
        head.clone()
    } else {
        let tail_offset = metadata
            .len()
            .saturating_sub(CLAUDE_LITE_READ_BUF_SIZE as u64);
        file.seek(SeekFrom::Start(tail_offset)).ok()?;
        let tail_len = file.read(&mut buffer).ok()?;
        String::from_utf8_lossy(&buffer[..tail_len]).to_string()
    };

    Some(ClaudeLiteSessionFile {
        head,
        tail,
        mtime: metadata.modified().ok()?,
    })
}

fn extract_claude_first_prompt_from_head(head: &str) -> Option<String> {
    let mut command_fallback = None;
    for line in head.lines() {
        if !line.contains("\"type\":\"user\"") && !line.contains("\"type\": \"user\"") {
            continue;
        }
        if line.contains("\"tool_result\"")
            || line.contains("\"isMeta\":true")
            || line.contains("\"isMeta\": true")
            || line.contains("\"isCompactSummary\":true")
            || line.contains("\"isCompactSummary\": true")
        {
            continue;
        }
        let Ok(value) = serde_json::from_str::<Value>(line) else {
            continue;
        };
        match claude_first_prompt_from_value(&value) {
            ClaudePromptCandidate::Meaningful(prompt) => return Some(prompt),
            ClaudePromptCandidate::Command(command) => {
                if command_fallback.is_none() {
                    command_fallback = Some(command);
                }
            }
            ClaudePromptCandidate::None => {}
        }
    }
    command_fallback
}

fn estimate_claude_message_count(path: &Path) -> usize {
    let Ok(content) = fs::read_to_string(path) else {
        return 0;
    };
    content
        .lines()
        .filter_map(|line| serde_json::from_str::<Value>(line).ok())
        .flat_map(|value| claude_message_from_value(&value))
        .filter(|message| message.kind == kind::TEXT)
        .count()
}

fn parse_claude_session(path: &Path) -> Result<SessionDetail, Box<dyn Error>> {
    if !is_claude_session_file(path) {
        return Err("not a Claude resume session file".into());
    }
    let official_session = parse_claude_lite_session(path, None)?;

    let content = fs::read_to_string(path)?;
    let mut conversation = ClaudeConversation {
        session_id: path
            .file_stem()
            .and_then(|name| name.to_str())
            .unwrap_or("unknown")
            .to_string(),
        project: String::new(),
        ..Default::default()
    };

    for line in content.lines().filter(|line| !line.trim().is_empty()) {
        let Ok(value) = serde_json::from_str::<Value>(line) else {
            continue;
        };
        collect_claude_metadata(&mut conversation, &value);
        for message in claude_message_from_value(&value) {
            if message.kind == kind::TEXT {
                conversation.visible_message_count += 1;
            }
            conversation.messages.push(message);
        }
        if conversation.first_prompt.is_none() {
            match claude_first_prompt_from_value(&value) {
                ClaudePromptCandidate::Meaningful(prompt) => {
                    conversation.first_prompt = Some(prompt)
                }
                ClaudePromptCandidate::Command(command) => {
                    if conversation.command_prompt_fallback.is_none() {
                        conversation.command_prompt_fallback = Some(command);
                    }
                }
                ClaudePromptCandidate::None => {}
            }
        }
    }

    // team_name is a real LIST filter from videcoding/cli sessionStorage.ts
    // (`if (enriched.teamName) return null`) — team-shared sessions are
    // hidden from the resume picker.
    //
    // `is_sidechain` is intentionally NOT checked here: official only filters
    // at LIST time using the FIRST line's isSidechain (handled in
    // parse_claude_lite_session). Mid-session sidechain entries are filtered
    // per-message inside `claude_message_from_value` so the main thread of a
    // session that spawns sub-agents stays visible.
    if conversation.team_name.is_some() {
        return Err("Claude related session is hidden from resume".into());
    }
    if conversation.first_prompt.is_none() {
        conversation.first_prompt = conversation.command_prompt_fallback.take();
    }
    let merged_messages = merge_tool_outputs(conversation.messages);
    let mut detail = detail_from_messages(
        "Claude",
        path,
        conversation.session_id,
        conversation.project,
        merged_messages,
        conversation.timestamps,
        Some(official_session.title.clone()),
    );
    detail.session.title = official_session.title;
    if detail.session.project.is_empty() {
        detail.session.project = official_session.project;
    }
    detail.session.message_count = conversation.visible_message_count;
    if detail.session.started_at.is_none() {
        detail.session.started_at = official_session
            .started_at
            .or_else(|| file_time(path, false));
    }
    if detail.session.updated_at.is_none() {
        detail.session.updated_at = official_session
            .updated_at
            .or_else(|| file_time(path, true));
    }
    Ok(detail)
}

fn collect_claude_metadata(conversation: &mut ClaudeConversation, value: &Value) {
    // Note: isSidechain is filtered per-message in claude_message_from_value
    // and at LIST time in parse_claude_lite_session. We intentionally do not
    // aggregate it onto the conversation struct, because hiding a whole
    // session because of mid-session sub-agent branches is over-restrictive
    // vs official behavior (see parse_claude_session for details).
    if let Some(team_name) = value.get("teamName").and_then(value_to_string) {
        conversation.team_name = Some(team_name);
    }
    if let Some(session_id) = value.get("sessionId").and_then(value_to_string) {
        conversation.session_id = session_id;
    }
    if let Some(cwd) = value.get("cwd").and_then(value_to_string) {
        conversation.project = cwd;
    }
    if let Some(timestamp) = value.get("timestamp").and_then(value_to_string) {
        conversation.timestamps.push(timestamp);
    }
    if let Some(custom_title) = value.get("customTitle").and_then(value_to_string) {
        conversation.custom_title = Some(custom_title);
    }
    if let Some(ai_title) = value.get("aiTitle").and_then(value_to_string) {
        conversation.ai_title = Some(ai_title);
    }
    if let Some(last_prompt) = value.get("lastPrompt").and_then(value_to_string) {
        conversation.last_prompt = Some(last_prompt);
    }
    if let Some(agent_name) = value.get("agentName").and_then(value_to_string) {
        conversation.agent_name = Some(agent_name);
    }
    if let Some(summary) = value.get("summary").and_then(value_to_string) {
        conversation.summary = Some(summary);
    }
}

fn is_claude_session_file(path: &Path) -> bool {
    path.extension().and_then(|ext| ext.to_str()) == Some("jsonl")
        && path
            .file_stem()
            .and_then(|stem| stem.to_str())
            .is_some_and(is_uuid_like)
}

fn is_uuid_like(value: &str) -> bool {
    let bytes = value.as_bytes();
    bytes.len() == 36
        && [8, 13, 18, 23].iter().all(|index| bytes[*index] == b'-')
        && value.chars().all(|c| c.is_ascii_hexdigit() || c == '-')
}

enum ClaudePromptCandidate {
    Meaningful(String),
    Command(String),
    None,
}

fn claude_first_prompt_from_value(value: &Value) -> ClaudePromptCandidate {
    if value.get("type").and_then(Value::as_str) != Some("user")
        || value.get("isMeta").and_then(Value::as_bool) == Some(true)
        || value.get("isCompactSummary").and_then(Value::as_bool) == Some(true)
    {
        return ClaudePromptCandidate::None;
    }
    let Some(content) = value
        .get("message")
        .and_then(|message| message.get("content"))
    else {
        return ClaudePromptCandidate::None;
    };
    if claude_user_content_is_tool_result_only(content) {
        return ClaudePromptCandidate::None;
    }
    for text in claude_text_blocks(content) {
        let prompt = text.replace('\n', " ").trim().to_string();
        if prompt.is_empty() {
            continue;
        }
        if let Some(command) = extract_xml_tag_value(&prompt, "command-name") {
            if !command.trim().is_empty() {
                return ClaudePromptCandidate::Command(command.trim().to_string());
            }
            continue;
        }
        if let Some(command) = extract_xml_tag_value(&prompt, "bash-input") {
            return ClaudePromptCandidate::Meaningful(truncate_claude_first_prompt(&format!(
                "! {}",
                command.trim()
            )));
        }
        if claude_skip_first_prompt_pattern(&prompt) {
            continue;
        }
        return ClaudePromptCandidate::Meaningful(truncate_claude_first_prompt(&prompt));
    }
    ClaudePromptCandidate::None
}

fn truncate_claude_first_prompt(value: &str) -> String {
    if value.chars().count() > 200 {
        format!("{}…", value.chars().take(200).collect::<String>().trim())
    } else {
        value.to_string()
    }
}

fn claude_skip_first_prompt_pattern(text: &str) -> bool {
    let trimmed = text.trim_start();
    trimmed.starts_with("[Request interrupted by user")
        || trimmed
            .strip_prefix('<')
            .and_then(|rest| rest.chars().next())
            .is_some_and(|c| c.is_ascii_lowercase())
}

fn strip_display_tags(text: &str) -> String {
    let stripped = strip_display_tags_allow_empty(text);
    if stripped.is_empty() {
        text.trim().to_string()
    } else {
        stripped
    }
}

fn strip_display_tags_allow_empty(text: &str) -> String {
    let mut result = String::new();
    let mut index = 0;
    while let Some(relative_start) = text[index..].find('<') {
        let start = index + relative_start;
        result.push_str(&text[index..start]);
        let Some(relative_close) = text[start..].find('>') else {
            result.push_str(&text[start..]);
            index = text.len();
            break;
        };
        let tag_content = &text[start + 1..start + relative_close];
        let tag_name = tag_content
            .split_whitespace()
            .next()
            .unwrap_or_default()
            .trim_start_matches('/');
        if tag_name
            .chars()
            .next()
            .is_some_and(|c| c.is_ascii_lowercase())
            && tag_name
                .chars()
                .all(|c| c.is_ascii_alphanumeric() || c == '_' || c == '-')
        {
            let end_tag = format!("</{tag_name}>");
            if let Some(relative_end) = text[start + relative_close + 1..].find(&end_tag) {
                index = start + relative_close + 1 + relative_end + end_tag.len();
                if text[index..].starts_with('\n') {
                    index += 1;
                }
                continue;
            }
        }
        result.push_str(&text[start..start + relative_close + 1]);
        index = start + relative_close + 1;
    }
    if index < text.len() {
        result.push_str(&text[index..]);
    }
    result.trim().to_string()
}

fn file_time(path: &Path, modified: bool) -> Option<String> {
    let metadata = fs::metadata(path).ok()?;
    let time = if modified {
        metadata.modified().ok()
    } else {
        metadata.created().ok().or_else(|| metadata.modified().ok())
    }?;
    Some(system_time_to_iso(time))
}

fn system_time_to_iso(time: SystemTime) -> String {
    let datetime: DateTime<Utc> = time.into();
    datetime.to_rfc3339()
}

fn scan_gemini_chats_dir(root: &Path) -> Result<Vec<AppSession>, Box<dyn Error>> {
    if !root.exists() {
        return Ok(Vec::new());
    }
    let mut sessions = Vec::new();
    for entry in fs::read_dir(root)? {
        let entry = entry?;
        let path = entry.path();
        if !path.is_file() || !is_gemini_session_file(&path) {
            continue;
        }
        if let Ok(detail) = parse_gemini_session_file(&path, true) {
            sessions.push(detail.session);
        }
    }
    Ok(sessions)
}

fn parse_gemini_jsonl_session(path: &Path) -> Result<SessionDetail, Box<dyn Error>> {
    parse_gemini_session_file(path, false)
}

fn parse_gemini_json_session(path: &Path) -> Result<SessionDetail, Box<dyn Error>> {
    parse_gemini_session_file(path, false)
}

#[derive(Default)]
struct GeminiConversation {
    session_id: String,
    start_time: Option<String>,
    last_updated: Option<String>,
    summary: Option<String>,
    first_user_message: Option<String>,
    kind: Option<String>,
    messages: Vec<Value>,
    message_count: usize,
    has_user_or_assistant: bool,
}

fn is_gemini_session_file(path: &Path) -> bool {
    let Some(file_name) = path.file_name().and_then(|name| name.to_str()) else {
        return false;
    };
    file_name.starts_with("session-")
        && (file_name.ends_with(".json") || file_name.ends_with(".jsonl"))
}

fn parse_gemini_session_file(
    path: &Path,
    metadata_only: bool,
) -> Result<SessionDetail, Box<dyn Error>> {
    let mut conversation = match path.extension().and_then(|ext| ext.to_str()) {
        Some("jsonl") => load_gemini_jsonl_conversation(path, metadata_only)?,
        Some("json") => load_gemini_json_conversation(path, metadata_only)?,
        _ => return Err("unsupported Gemini session file".into()),
    };
    if conversation.session_id.is_empty()
        || !conversation.has_user_or_assistant
        || conversation.kind.as_deref() == Some("subagent")
    {
        return Err("not a Gemini session list entry".into());
    }

    // Timestamp fallback mirrors getAllSessionFiles in
    // packages/cli/src/utils/sessionUtils.ts: when startTime/lastUpdated are
    // missing on the record, fall back to (in order) the other field, the
    // file's mtime, then "now". Without this, sessions written by older
    // recorders that omit timestamps would be hidden from Termory even though
    // the official CLI still lists them.
    if conversation.start_time.is_none() || conversation.last_updated.is_none() {
        let mtime = file_time(path, true);
        let now = Utc::now().to_rfc3339_opts(chrono::SecondsFormat::Secs, true);
        let fallback = mtime.unwrap_or(now);
        if conversation.start_time.is_none() {
            conversation.start_time = conversation
                .last_updated
                .clone()
                .or_else(|| Some(fallback.clone()));
        }
        if conversation.last_updated.is_none() {
            conversation.last_updated = conversation.start_time.clone().or(Some(fallback));
        }
    }

    let project = gemini_project_from_chat_path(path).ok_or("missing Gemini .project_root")?;
    let messages = if metadata_only {
        Vec::new()
    } else {
        gemini_messages_from_values(&conversation.messages)
    };
    let title = conversation
        .summary
        .as_deref()
        .map(strip_unsafe_characters)
        .and_then(|title| official_title_from_text(&title))
        .or_else(|| conversation.first_user_message.clone());
    let mut detail = detail_from_messages(
        "Gemini",
        path,
        conversation.session_id,
        project,
        messages,
        Vec::new(),
        title,
    );
    detail.session.started_at = conversation.start_time;
    detail.session.updated_at = conversation.last_updated;
    detail.session.message_count = conversation.message_count;
    Ok(detail)
}

fn load_gemini_jsonl_conversation(
    path: &Path,
    metadata_only: bool,
) -> Result<GeminiConversation, Box<dyn Error>> {
    let content = fs::read_to_string(path)?;
    let mut metadata = Value::Object(Default::default());
    let mut messages_map = HashMap::<String, Value>::new();
    let mut message_order = Vec::<String>::new();
    let mut message_ids = Vec::<String>::new();
    let mut message_kinds = HashMap::<String, (bool, bool)>::new();
    let mut first_user_message: Option<String> = None;
    for line in content.lines().filter(|line| !line.trim().is_empty()) {
        let Ok(value) = serde_json::from_str::<Value>(line) else {
            continue;
        };
        if let Some(rewind_id) = value.get("$rewindTo").and_then(value_to_string) {
            if metadata_only {
                if let Some(index) = message_ids.iter().position(|id| id == &rewind_id) {
                    for removed in message_ids.split_off(index) {
                        message_kinds.remove(&removed);
                    }
                } else {
                    message_ids.clear();
                    message_kinds.clear();
                }
            } else {
                if let Some(index) = message_order.iter().position(|id| id == &rewind_id) {
                    for id in message_order.split_off(index) {
                        messages_map.remove(&id);
                    }
                } else {
                    messages_map.clear();
                    message_order.clear();
                }
            }
            continue;
        }

        if let Some(id) = value.get("id").and_then(value_to_string) {
            let is_user = value.get("type").and_then(Value::as_str) == Some("user");
            let is_user_or_assistant = matches!(
                value.get("type").and_then(Value::as_str),
                Some("user" | "gemini")
            );
            if is_user && first_user_message.is_none() {
                first_user_message = value
                    .get("content")
                    .and_then(gemini_content_text_raw)
                    .map(|content| gemini_clean_message(&content));
            }
            if metadata_only {
                message_ids.push(id.clone());
                message_kinds.insert(id.clone(), (is_user, is_user_or_assistant));
            }
            if !metadata_only {
                if !messages_map.contains_key(&id) {
                    message_order.push(id.clone());
                }
                messages_map.insert(id, value);
            }
            continue;
        }

        if let Some(update) = value.get("$set").and_then(Value::as_object) {
            let mut next = metadata.as_object().cloned().unwrap_or_default();
            for (key, value) in update {
                next.insert(key.clone(), value.clone());
            }
            metadata = Value::Object(next);
            continue;
        }

        if value.get("sessionId").is_some() && value.get("projectHash").is_some() {
            let mut next = metadata.as_object().cloned().unwrap_or_default();
            if let Some(object) = value.as_object() {
                for (key, value) in object {
                    next.insert(key.clone(), value.clone());
                }
            }
            metadata = Value::Object(next);
        }
    }

    let metadata_messages = metadata
        .get("messages")
        .and_then(Value::as_array)
        .cloned()
        .unwrap_or_default();
    let loaded_messages = if metadata_messages.is_empty() {
        message_order
            .into_iter()
            .filter_map(|id| messages_map.remove(&id))
            .collect::<Vec<_>>()
    } else {
        metadata_messages.clone()
    };
    let message_count = if metadata_only {
        if metadata_messages.is_empty() {
            message_ids.len()
        } else {
            metadata_messages.len()
        }
    } else {
        loaded_messages.len()
    };
    let has_user_or_assistant = if metadata_only && !metadata_messages.is_empty() {
        metadata_messages.iter().any(gemini_is_user_or_assistant)
    } else if metadata_only {
        message_kinds.values().any(|(_, visible)| *visible)
    } else {
        loaded_messages.iter().any(gemini_is_user_or_assistant)
    };

    Ok(GeminiConversation {
        session_id: metadata
            .get("sessionId")
            .and_then(value_to_string)
            .unwrap_or_default(),
        start_time: metadata
            .get("startTime")
            .and_then(value_to_string)
            .and_then(normalize_time),
        last_updated: metadata
            .get("lastUpdated")
            .and_then(value_to_string)
            .and_then(normalize_time),
        summary: metadata.get("summary").and_then(value_to_string),
        first_user_message: metadata
            .get("firstUserMessage")
            .and_then(value_to_string)
            .map(|message| gemini_clean_message(&message))
            .or_else(|| gemini_extract_first_user_message(&loaded_messages))
            .or(first_user_message),
        kind: metadata.get("kind").and_then(value_to_string),
        messages: if metadata_only {
            Vec::new()
        } else {
            loaded_messages
        },
        message_count,
        has_user_or_assistant,
    })
}

fn load_gemini_json_conversation(
    path: &Path,
    metadata_only: bool,
) -> Result<GeminiConversation, Box<dyn Error>> {
    let value = serde_json::from_str::<Value>(&fs::read_to_string(path)?)?;
    let messages = value
        .get("messages")
        .and_then(Value::as_array)
        .cloned()
        .unwrap_or_default();
    let has_user_or_assistant = messages.iter().any(gemini_is_user_or_assistant);
    Ok(GeminiConversation {
        session_id: value
            .get("sessionId")
            .and_then(value_to_string)
            .unwrap_or_default(),
        start_time: value
            .get("startTime")
            .and_then(value_to_string)
            .and_then(normalize_time),
        last_updated: value
            .get("lastUpdated")
            .and_then(value_to_string)
            .and_then(normalize_time),
        summary: value.get("summary").and_then(value_to_string),
        first_user_message: value
            .get("firstUserMessage")
            .and_then(value_to_string)
            .map(|message| gemini_clean_message(&message))
            .or_else(|| gemini_extract_first_user_message(&messages)),
        kind: value.get("kind").and_then(value_to_string),
        messages: if metadata_only {
            Vec::new()
        } else {
            messages.clone()
        },
        message_count: messages.len(),
        has_user_or_assistant,
    })
}

fn gemini_extract_first_user_message(messages: &[Value]) -> Option<String> {
    let first_meaningful = messages
        .iter()
        .filter(|message| message.get("type").and_then(Value::as_str) == Some("user"))
        .filter_map(|message| {
            message
                .get("content")
                .and_then(gemini_content_text_raw)
                .map(|content| gemini_clean_message(&content))
        })
        .find(|content| {
            !content.starts_with('/') && !content.starts_with('?') && !content.trim().is_empty()
        });
    if first_meaningful.is_some() {
        return first_meaningful;
    }
    messages
        .iter()
        .filter(|message| message.get("type").and_then(Value::as_str) == Some("user"))
        .filter_map(|message| {
            message
                .get("content")
                .and_then(gemini_content_text_raw)
                .map(|content| gemini_clean_message(&content))
        })
        .find(|content| !content.trim().is_empty())
        .or_else(|| Some("Empty conversation".to_string()))
}

fn gemini_clean_message(message: &str) -> String {
    message
        .split_whitespace()
        .collect::<Vec<_>>()
        .join(" ")
        .chars()
        .filter(|c| c.is_ascii_graphic() || *c == ' ')
        .collect::<String>()
        .trim()
        .to_string()
}

fn strip_unsafe_characters(message: &str) -> String {
    message
        .chars()
        .filter(|c| c.is_ascii_graphic() || *c == ' ')
        .collect::<String>()
}

fn gemini_messages_from_values(values: &[Value]) -> Vec<SessionMessage> {
    let mut messages = Vec::new();
    for value in values {
        // Gemini's `thoughts` array (rendered by the TUI's `ThinkingMessage`
        // at .audit-sources/gemini-cli/packages/cli/src/ui/components/messages/ThinkingMessage.tsx)
        // is sibling to `content`. We surface each thought as a separate
        // reasoning message using the shared italic-blockquote format so
        // Claude / Codex / OpenCode / Gemini all use the same convention.
        messages.extend(gemini_thought_messages_from_value(value));
        if let Some(message) = gemini_message_from_value(value) {
            messages.push(message);
        }
        if value.get("type").and_then(Value::as_str) != Some("user") {
            messages.extend(gemini_tool_messages_from_value(value));
        }
    }
    messages
}

/// Extract Gemini thought entries (`thoughts: [{subject, description, ...}]`)
/// and emit each as a reasoning SessionMessage. Mirrors
/// `normalizeThoughtLines` in
/// `.audit-sources/gemini-cli/packages/cli/src/ui/components/messages/ThinkingMessage.tsx:22`
/// — subject + description joined, noise lines (whitespace / `...` runs)
/// filtered out.
fn gemini_thought_messages_from_value(value: &Value) -> Vec<SessionMessage> {
    let Some(thoughts) = value.get("thoughts").and_then(Value::as_array) else {
        return Vec::new();
    };
    let parent_ts = value
        .get("timestamp")
        .and_then(value_to_string)
        .and_then(normalize_time);
    let mut out = Vec::new();
    for thought in thoughts {
        let subject = thought
            .get("subject")
            .and_then(Value::as_str)
            .unwrap_or("")
            .trim();
        let description = thought
            .get("description")
            .and_then(Value::as_str)
            .unwrap_or("")
            .trim();
        let is_noise = |line: &str| {
            let trimmed = line.trim();
            trimmed.is_empty() || trimmed.chars().all(|c| c == '.')
        };
        let mut lines: Vec<&str> = Vec::new();
        if !subject.is_empty() && !is_noise(subject) {
            lines.push(subject);
        }
        if !description.is_empty() {
            lines.extend(
                description
                    .lines()
                    .map(str::trim)
                    .filter(|line| !is_noise(line)),
            );
        }
        if lines.is_empty() {
            continue;
        }
        let joined = lines.join("\n");
        let formatted = format_reasoning_body(&joined);
        if formatted.is_empty() {
            continue;
        }
        let ts = thought
            .get("timestamp")
            .and_then(value_to_string)
            .and_then(normalize_time)
            .or_else(|| parent_ts.clone());
        out.push(SessionMessage {
            role: "assistant".to_string(),
            text: formatted,
            timestamp: ts,
            kind: kind::REASONING.to_string(),
            tool_use_id: None,
            exit_code: None,
        });
    }
    out
}

fn gemini_message_from_value(value: &Value) -> Option<SessionMessage> {
    let raw_type = value.get("type").and_then(value_to_string)?;
    let role = match raw_type.as_str() {
        "user" => "user",
        "gemini" => "assistant",
        "info" | "error" | "warning" => raw_type.as_str(),
        _ => return None,
    };
    let text = value
        .get("displayContent")
        .and_then(gemini_content_text_raw)
        .or_else(|| value.get("content").and_then(gemini_content_text_raw))?;
    if text.trim().is_empty() {
        return None;
    }
    let timestamp = value
        .get("timestamp")
        .and_then(value_to_string)
        .and_then(normalize_time);
    Some(SessionMessage {
        role: role.to_string(),
        text,
        timestamp,
        kind: kind::TEXT.to_string(),
        tool_use_id: None,
        exit_code: None,
    })
}

fn gemini_tool_messages_from_value(value: &Value) -> Vec<SessionMessage> {
    let Some(tools) = value.get("toolCalls").and_then(Value::as_array) else {
        return Vec::new();
    };
    let timestamp = value
        .get("timestamp")
        .and_then(value_to_string)
        .and_then(normalize_time);
    tools
        .iter()
        .filter_map(|tool| {
            // Gemini TUI render: ToolMessage → ToolInfo at
            //   .audit-sources/gemini-cli/packages/cli/src/ui/components/messages/ToolShared.tsx:202
            // `<bold>{displayName}</bold> {description}` — bold tool display
            // name + space + description in secondary text. Result content
            // renders separately below via ToolResultDisplay using the
            // `resultDisplay` field (IndividualToolCallDisplay).
            let display_name = tool
                .get("displayName")
                .and_then(value_to_string)
                .or_else(|| tool.get("name").and_then(value_to_string))?;
            let description = tool
                .get("description")
                .and_then(value_to_string)
                .unwrap_or_default();
            // Unified `**Verb**(args)` shape — description (Gemini's TUI
            // secondary text from `ToolShared.tsx:202`) goes inside parens
            // so the card reads the same as Codex / Claude / OpenCode tool
            // headers.
            let header = if description.is_empty() {
                format!("**{display_name}**")
            } else {
                format!("**{display_name}**({description})")
            };
            let mut text = header;
            if let Some(result) = tool.get("resultDisplay").and_then(gemini_content_text_raw) {
                let trimmed = result.trim();
                if !trimmed.is_empty() {
                    // resultDisplay can be plain text or structured. Wrap in
                    // a 4-backtick fence so the markdown renderer applies
                    // monospace + highlight.js can infer a language,
                    // approximating ToolResultDisplay's per-tool formatting.
                    text.push_str("\n\n````\n");
                    text.push_str(trimmed);
                    text.push_str("\n````");
                }
            }
            Some(SessionMessage {
                role: "tool".to_string(),
                text,
                timestamp: timestamp.clone(),
                kind: kind::TOOL_USE.to_string(),
                tool_use_id: None,
                exit_code: None,
            })
        })
        .collect()
}

fn gemini_content_text_raw(value: &Value) -> Option<String> {
    let mut parts = Vec::new();
    match value {
        Value::String(text) => parts.push(text.clone()),
        Value::Array(items) => {
            for item in items {
                if let Some(text) = gemini_part_to_string(item) {
                    parts.push(text);
                }
            }
        }
        _ => {}
    }
    let text = parts.join("").trim().to_string();
    if text.is_empty() {
        None
    } else {
        Some(text)
    }
}

fn gemini_part_to_string(value: &Value) -> Option<String> {
    if let Some(text) = value.get("text").and_then(value_to_string) {
        return Some(text);
    }
    if value.get("videoMetadata").is_some() {
        return Some("[Video Metadata]".to_string());
    }
    if let Some(thought) = value.get("thought").and_then(value_to_string) {
        return Some(format!("[Thought: {thought}]"));
    }
    // `executableCode` ({code, language}) is the model's "I will run this
    // code" announcement; render as a fenced block so it's visible
    // (matches Gemini TUI's `ExecutableCodeRenderer`, which displays code
    // in a syntax-highlighted block).
    if let Some(exec) = value.get("executableCode") {
        let code = exec.get("code").and_then(Value::as_str).unwrap_or("");
        let lang = exec
            .get("language")
            .and_then(Value::as_str)
            .map(|s| s.to_lowercase())
            .filter(|s| !s.is_empty());
        if !code.trim().is_empty() {
            return Some(match lang {
                Some(l) => format!("```{l}\n{}\n```", code.trim_end()),
                None => format!("```\n{}\n```", code.trim_end()),
            });
        }
    }
    // `codeExecutionResult` ({outcome, output}) is the result of running
    // executable code. Show the output in a fenced block + outcome label
    // (e.g., `OUTCOME_OK` / `OUTCOME_FAILED`).
    if let Some(result) = value.get("codeExecutionResult") {
        let output = result
            .get("output")
            .and_then(Value::as_str)
            .unwrap_or("")
            .trim_end();
        let outcome = result.get("outcome").and_then(Value::as_str).unwrap_or("");
        if !output.is_empty() || !outcome.is_empty() {
            let mut text = String::new();
            if !output.is_empty() {
                text.push_str("````\n");
                text.push_str(output);
                text.push_str("\n````");
            }
            if !outcome.is_empty() && outcome != "OUTCOME_OK" {
                if !text.is_empty() {
                    text.push_str("\n\n");
                }
                text.push_str(&format!("*Outcome: {outcome}*"));
            }
            return Some(text);
        }
    }
    // `inlineData` ({mimeType, data}) carries base64 binary content
    // (images / audio). We can't show it in markdown but we should
    // signal what was attached.
    if let Some(inline) = value.get("inlineData") {
        let mime = inline
            .get("mimeType")
            .and_then(Value::as_str)
            .unwrap_or("application/octet-stream");
        return Some(format!("*Inline data ({mime})*"));
    }
    if value.get("fileData").is_some() {
        let uri = value
            .get("fileData")
            .and_then(|f| f.get("fileUri"))
            .and_then(Value::as_str);
        return Some(match uri {
            Some(u) => format!("*File: {u}*"),
            None => "[File Data]".to_string(),
        });
    }
    // `functionCall` / `functionResponse` in PARTS (sibling to `text`)
    // are inline markers of the assistant invoking a tool. The structured
    // ToolCall/ToolResponse cards arrive separately via `toolCalls` and
    // are rendered by `gemini_tool_messages_from_value`. The PART version
    // here just signals "a tool call happened" without duplicating the
    // full body.
    if let Some(call) = value.get("functionCall") {
        let name = call
            .get("name")
            .and_then(value_to_string)
            .unwrap_or_default();
        let display = if name.is_empty() {
            "*Tool call*".to_string()
        } else {
            format!("*Tool call: {name}*")
        };
        return Some(display);
    }
    if let Some(response) = value.get("functionResponse") {
        let name = response
            .get("name")
            .and_then(value_to_string)
            .unwrap_or_default();
        let display = if name.is_empty() {
            "*Tool response*".to_string()
        } else {
            format!("*Tool response: {name}*")
        };
        return Some(display);
    }
    None
}

fn gemini_is_user_or_assistant(value: &Value) -> bool {
    matches!(
        value.get("type").and_then(Value::as_str),
        Some("user" | "gemini")
    )
}

fn gemini_project_from_chat_path(path: &Path) -> Option<String> {
    let project_dir = path.parent()?.parent()?;
    fs::read_to_string(project_dir.join(".project_root"))
        .ok()
        .map(|project| project.trim().to_string())
        .filter(|project| !project.is_empty())
}

fn parse_codex_session(path: &Path, id: &str) -> Result<SessionDetail, Box<dyn Error>> {
    let mut session_from_state = codex_thread_from_state(id).ok();
    let content = fs::read_to_string(path)?;
    let mut messages = Vec::new();
    let mut session_id = id.to_string();
    let mut project = session_from_state
        .as_ref()
        .map(|session| session.project.clone())
        .unwrap_or_default();
    let mut timestamps = Vec::new();

    for line in content.lines().filter(|line| !line.trim().is_empty()) {
        let Ok(value) = serde_json::from_str::<Value>(line) else {
            continue;
        };
        if let Some(timestamp) = value.get("timestamp").and_then(value_to_string) {
            timestamps.push(timestamp.clone());
        }
        if value.get("type").and_then(Value::as_str) == Some("session_meta") {
            if let Some(payload) = value.get("payload") {
                if let Some(found_id) = payload.get("id").and_then(value_to_string) {
                    session_id = found_id;
                }
                if let Some(cwd) = payload.get("cwd").and_then(value_to_string) {
                    project = cwd;
                }
            }
            continue;
        }
        if let Some(message) = codex_message_from_value(&value) {
            messages.push(message);
        }
    }

    let messages = merge_tool_outputs(messages);

    let mut detail = detail_from_messages(
        "Codex",
        path,
        session_id,
        project,
        messages,
        timestamps,
        session_from_state
            .as_ref()
            .map(|session| session.title.clone()),
    );

    if let Some(session) = session_from_state.take() {
        detail.session.title = session.title;
        detail.session.started_at = session.started_at;
        detail.session.updated_at = session.updated_at;
        detail.session.message_count = detail.messages.len();
    }

    Ok(detail)
}

fn codex_thread_from_state(id: &str) -> Result<AppSession, Box<dyn Error>> {
    let Some(home) = dirs::home_dir() else {
        return Err("home directory not found".into());
    };
    let path = home.join(".codex").join("state_5.sqlite");
    let conn = Connection::open_with_flags(path, OpenFlags::SQLITE_OPEN_READ_ONLY)?;
    let mut stmt = conn.prepare(
        "select id, rollout_path, created_at, updated_at, cwd, title, first_user_message \
         from threads \
         where id = ?1 \
           and archived = 0 \
           and source in ('cli', 'vscode', 'atlas', 'chatgpt')",
    )?;
    let session = stmt.query_row([id], |row| {
        let id: String = row.get(0)?;
        let rollout_path: String = row.get(1)?;
        let created_at: i64 = row.get(2)?;
        let updated_at: i64 = row.get(3)?;
        let cwd: String = row.get(4)?;
        let title: String = row.get(5).unwrap_or_default();
        let first_user_message: String = row.get(6).unwrap_or_default();
        let display_title = codex_display_title(&title)
            .or_else(|| codex_display_title(&first_user_message))
            .unwrap_or_default();
        Ok(AppSession {
            id,
            source: "Codex".to_string(),
            title: display_title,
            project: cwd,
            path: rollout_path.clone(),
            started_at: normalize_time(created_at.to_string()),
            updated_at: normalize_time(updated_at.to_string()),
            message_count: 0,
            preview: String::new(),
            message_previews: Vec::new(),
        })
    })?;
    Ok(session)
}

fn scan_opencode_db(path: &Path) -> Result<Vec<AppSession>, Box<dyn Error>> {
    let conn = Connection::open_with_flags(path, OpenFlags::SQLITE_OPEN_READ_ONLY)?;
    ensure_opencode_schema(&conn)?;
    let mut stmt = conn.prepare(
        "select id, directory, title, time_created, time_updated \
         from session \
         where parent_id is null and time_archived is null \
         order by time_updated desc, id desc",
    )?;
    let rows = stmt.query_map([], |row| {
        let id: String = row.get(0)?;
        let project: String = row.get(1)?;
        let title: String = row.get(2)?;
        let created_raw: i64 = row.get(3)?;
        let updated_raw: i64 = row.get(4)?;
        let message_count = count_opencode_visible_messages(&conn, &id).unwrap_or(0);
        Ok(AppSession {
            id: id.clone(),
            source: "OpenCode".to_string(),
            title: opencode_display_title(&title),
            project,
            path: path.display().to_string(),
            started_at: normalize_time(created_raw.to_string()),
            updated_at: normalize_time(updated_raw.to_string()),
            message_count,
            preview: String::new(),
            message_previews: Vec::new(),
        })
    })?;

    let mut sessions = Vec::new();
    for row in rows {
        sessions.push(row?);
    }
    Ok(sessions)
}

fn parse_opencode_db_session(path: &Path, id: &str) -> Result<SessionDetail, Box<dyn Error>> {
    let conn = Connection::open_with_flags(path, OpenFlags::SQLITE_OPEN_READ_ONLY)?;
    ensure_opencode_schema(&conn)?;
    let session = opencode_session_from_db(&conn, path, id)?
        .ok_or_else(|| format!("OpenCode session not found: {id}"))?;
    let messages = read_opencode_db_messages(&conn, id)?;
    let mut detail = SessionDetail { session, messages };
    detail.session.message_count = detail.messages.len();
    Ok(detail)
}

fn ensure_opencode_schema(conn: &Connection) -> Result<(), Box<dyn Error>> {
    first_existing_table(conn, &["session"])?;
    if table_exists(conn, "session_message")? {
        return Ok(());
    }
    first_existing_table(conn, &["message"])?;
    first_existing_table(conn, &["part"])?;
    Ok(())
}

fn opencode_session_from_db(
    conn: &Connection,
    path: &Path,
    id: &str,
) -> Result<Option<AppSession>, Box<dyn Error>> {
    let session = conn
        .query_row(
            "select id, directory, title, time_created, time_updated \
             from session \
             where id = ?1 and time_archived is null",
            [id],
            |row| {
                let id: String = row.get(0)?;
                let project: String = row.get(1)?;
                let title: String = row.get(2)?;
                let created_raw: i64 = row.get(3)?;
                let updated_raw: i64 = row.get(4)?;
                Ok(AppSession {
                    id: id.clone(),
                    source: "OpenCode".to_string(),
                    title: opencode_display_title(&title),
                    project,
                    path: path.display().to_string(),
                    started_at: normalize_time(created_raw.to_string()),
                    updated_at: normalize_time(updated_raw.to_string()),
                    message_count: 0,
                    preview: String::new(),
                    message_previews: Vec::new(),
                })
            },
        )
        .optional()?;
    Ok(session)
}

fn count_opencode_visible_messages(
    conn: &Connection,
    session_id: &str,
) -> Result<usize, Box<dyn Error>> {
    Ok(read_opencode_db_messages(conn, session_id)?.len())
}

fn read_opencode_db_messages(
    conn: &Connection,
    session_id: &str,
) -> Result<Vec<SessionMessage>, Box<dyn Error>> {
    if !table_exists(conn, "message")? || !table_exists(conn, "part")? {
        if table_exists(conn, "session_message")? {
            return read_opencode_v2_db_messages(conn, session_id);
        }
        return Ok(Vec::new());
    }
    let mut stmt = conn.prepare(
        "select id, time_created, data \
         from message \
         where session_id = ?1 \
         order by time_created asc, id asc",
    )?;
    let message_rows: Vec<(String, i64, String)> = stmt
        .query_map([session_id], |row| {
            Ok((
                row.get::<_, String>(0)?,
                row.get::<_, i64>(1)?,
                row.get::<_, String>(2)?,
            ))
        })?
        .collect::<Result<_, _>>()?;

    let mut out = Vec::new();
    for (message_id, created, data) in message_rows {
        let Ok(value) = serde_json::from_str::<Value>(&data) else {
            continue;
        };
        let Some(role) = value.get("role").and_then(value_to_string) else {
            continue;
        };
        if role != "user" && role != "assistant" {
            continue;
        }
        let timestamp = value
            .get("time")
            .and_then(|time| time.get("created"))
            .and_then(value_to_string)
            .and_then(normalize_time)
            .or_else(|| normalize_time(created.to_string()));

        // Emit one SessionMessage per relevant part so the TUI-style tool
        // labels (Bash, Update, Read, ...) and per-part timestamps show
        // through. Matches transcript.ts formatPart's part-by-part loop.
        let mut part_stmt = conn.prepare(
            "select data from part \
             where session_id = ?1 and message_id = ?2 \
             order by time_created asc, id asc",
        )?;
        let part_rows: Vec<String> = part_stmt
            .query_map((session_id, &message_id), |row| row.get::<_, String>(0))?
            .collect::<Result<_, _>>()?;
        for data in part_rows {
            let Ok(part) = serde_json::from_str::<Value>(&data) else {
                continue;
            };
            let part_type = part.get("type").and_then(Value::as_str).unwrap_or("");
            match part_type {
                "text" => {
                    // Skip synthetic env / tool-ack injections.
                    if part.get("synthetic").and_then(Value::as_bool) == Some(true) {
                        continue;
                    }
                    let Some(text) = part.get("text").and_then(value_to_string) else {
                        continue;
                    };
                    let trimmed = text.trim();
                    if trimmed.is_empty() {
                        continue;
                    }
                    out.push(SessionMessage {
                        role: role.clone(),
                        text: trimmed.to_string(),
                        timestamp: timestamp.clone(),
                        kind: kind::TEXT.to_string(),
                        tool_use_id: None,
                        exit_code: None,
                    });
                }
                "reasoning" => {
                    let Some(text) = part.get("text").and_then(value_to_string) else {
                        continue;
                    };
                    let cleaned = text.replace("[REDACTED]", "");
                    let formatted = format_reasoning_body(&cleaned);
                    if formatted.is_empty() {
                        continue;
                    }
                    out.push(SessionMessage {
                        role: "assistant".to_string(),
                        text: formatted,
                        timestamp: timestamp.clone(),
                        kind: kind::REASONING.to_string(),
                        tool_use_id: None,
                        exit_code: None,
                    });
                }
                "tool" => {
                    let Some(body) = opencode_v2_tool_part_text(&part) else {
                        continue;
                    };
                    out.push(SessionMessage {
                        role: "tool".to_string(),
                        text: body,
                        timestamp: timestamp.clone(),
                        kind: kind::TOOL_USE.to_string(),
                        tool_use_id: part.get("callID").and_then(value_to_string),
                        exit_code: None,
                    });
                }
                _ => {}
            }
        }
    }
    Ok(out)
}

fn read_opencode_v2_db_messages(
    conn: &Connection,
    session_id: &str,
) -> Result<Vec<SessionMessage>, Box<dyn Error>> {
    let mut stmt = conn.prepare(
        "select id, type, time_created, data \
         from session_message \
         where session_id = ?1 \
         order by time_created asc, id asc",
    )?;
    let rows = stmt.query_map([session_id], |row| {
        Ok((
            row.get::<_, String>(0)?,
            row.get::<_, String>(1)?,
            row.get::<_, i64>(2)?,
            row.get::<_, String>(3)?,
        ))
    })?;

    let mut messages = Vec::new();
    for row in rows {
        let (id, message_type, created, data) = row?;
        let Ok(mut value) = serde_json::from_str::<Value>(&data) else {
            continue;
        };
        if let Some(object) = value.as_object_mut() {
            object.insert("id".to_string(), Value::String(id));
            object.insert("type".to_string(), Value::String(message_type.clone()));
        }
        if let Some(message) = opencode_v2_message_from_value(&value, created) {
            messages.push(message);
        }
    }
    Ok(messages)
}

fn opencode_v2_message_from_value(value: &Value, created: i64) -> Option<SessionMessage> {
    let message_type = value.get("type").and_then(Value::as_str)?;
    let timestamp = value
        .get("time")
        .and_then(|time| time.get("created"))
        .and_then(value_to_string)
        .and_then(normalize_time)
        .or_else(|| normalize_time(created.to_string()));
    match message_type {
        "user" => value
            .get("text")
            .and_then(value_to_string)
            .map(|text| SessionMessage {
                role: "user".to_string(),
                text: text.trim().to_string(),
                timestamp,
                kind: kind::TEXT.to_string(),
                tool_use_id: None,
                exit_code: None,
            }),
        "assistant" => opencode_v2_assistant_text(value).map(|text| SessionMessage {
            role: "assistant".to_string(),
            text,
            timestamp,
            kind: kind::TEXT.to_string(),
            tool_use_id: None,
            exit_code: None,
        }),
        "shell" => {
            let command = value.get("command").and_then(value_to_string)?;
            let mut lines = vec![format!("$ {command}")];
            if let Some(output) = value.get("output").and_then(value_to_string) {
                let output = output.trim();
                if !output.is_empty() {
                    lines.push(output.to_string());
                }
            }
            Some(SessionMessage {
                role: "tool".to_string(),
                text: lines.join("\n"),
                timestamp,
                kind: kind::SHELL.to_string(),
                tool_use_id: None,
                exit_code: None,
            })
        }
        "compaction" => value
            .get("summary")
            .and_then(value_to_string)
            .map(|summary| SessionMessage {
                role: "system".to_string(),
                text: summary.trim().to_string(),
                timestamp,
                kind: kind::COMPACTION.to_string(),
                tool_use_id: None,
                exit_code: None,
            }),
        "agent-switched" => {
            value
                .get("agent")
                .and_then(value_to_string)
                .map(|agent| SessionMessage {
                    role: "system".to_string(),
                    text: format!("Switched agent to {}", title_case(&agent)),
                    timestamp,
                    kind: kind::AGENT_SWITCHED.to_string(),
                    tool_use_id: None,
                    exit_code: None,
                })
        }
        "model-switched" => value.get("model").map(|model| {
            let provider = model
                .get("providerID")
                .and_then(value_to_string)
                .unwrap_or_default();
            let id = model
                .get("id")
                .and_then(value_to_string)
                .unwrap_or_default();
            let variant = model.get("variant").and_then(value_to_string);
            let mut label = format!("{provider}/{id}");
            if let Some(variant) = variant {
                if !variant.is_empty() && variant != "default" {
                    label.push('/');
                    label.push_str(&variant);
                }
            }
            SessionMessage {
                role: "system".to_string(),
                text: format!("Switched model to {label}"),
                timestamp,
                kind: kind::MODEL_SWITCHED.to_string(),
                tool_use_id: None,
                exit_code: None,
            }
        }),
        _ => None,
    }
}

fn opencode_v2_assistant_text(value: &Value) -> Option<String> {
    let content = value.get("content").and_then(Value::as_array)?;
    let parts = content
        .iter()
        .filter_map(opencode_v2_assistant_part_text)
        .collect::<Vec<_>>();
    let text = parts.join("\n\n").trim().to_string();
    if text.is_empty() {
        None
    } else {
        Some(text)
    }
}

fn opencode_v2_assistant_part_text(part: &Value) -> Option<String> {
    match part.get("type").and_then(Value::as_str)? {
        "text" => part.get("text").and_then(value_to_string),
        "reasoning" => part
            .get("text")
            .and_then(value_to_string)
            .map(|text| format_reasoning_body(&text.replace("[REDACTED]", "")))
            .filter(|s| !s.is_empty()),
        "tool" => opencode_v2_tool_part_text(part),
        _ => None,
    }
}

// OpenCode tool rendering — unified `**Verb**(args)` format across the
// four platforms. The verb (Bash / Read / Edit / etc.) still comes
// from OpenCode's TUI source (session-v2.tsx referenced per-branch),
// but the layout is the Termory unified shape:
//   * header → `**Verb**({wrap_inline_code(arg)})`
//   * body   → 4-backtick fence with output / content / diff (when present)
// The OpenCode-specific decorations (`\# Title` H1 escape, `←` arrow,
// `↳ Loaded`, `"pattern" in path` quoted format) are dropped in favor
// of the unified shape so all four platforms read consistently.
fn opencode_v2_tool_part_text(part: &Value) -> Option<String> {
    let name = part
        .get("name")
        .and_then(value_to_string)
        .or_else(|| part.get("tool").and_then(value_to_string))?;
    let state = part.get("state")?;
    let input = state.get("input").unwrap_or(&Value::Null);
    let input_record = input.as_object();
    let metadata = part
        .get("provider")
        .and_then(|provider| provider.get("metadata"))
        .or_else(|| state.get("metadata"))
        .or_else(|| part.get("metadata"));
    let output = state
        .get("content")
        .and_then(opencode_v2_tool_output)
        .or_else(|| state.get("output").and_then(value_to_string))
        .or_else(|| {
            metadata
                .and_then(|metadata| metadata.get("output"))
                .and_then(value_to_string)
        })
        .unwrap_or_default();
    // Unified block builder: header `**Verb**(args)` + (optional) fenced
    // body. The 4-backtick fence matches `merge_tool_outputs` so the
    // OpenCode parts (which arrive already combined) look identical to
    // Codex / Claude merged tool cards.
    let unified = |verb: &str, args: &str, body: &str, lang: &str| -> String {
        let header = if args.is_empty() {
            format!("**{verb}**")
        } else {
            format!("**{verb}**({args})")
        };
        let trimmed = body.trim();
        if trimmed.is_empty() {
            header
        } else if lang.is_empty() {
            format!("{header}\n\n````\n{trimmed}\n````")
        } else {
            format!("{header}\n\n```{lang}\n{trimmed}\n```")
        }
    };
    // session-v2.tsx:707 Bash — header `**Shell**(\`cmd\`)` (matches the
    // original OpenCode default title `\# Shell`); body keeps the
    // description title + bash fence with `$ cmd` prefix unchanged.
    if name == "bash" || name == "shell" {
        let command = input_record
            .and_then(|record| record.get("command"))
            .and_then(value_to_string)
            .or_else(|| input.as_str().map(ToString::to_string))
            .unwrap_or_default();
        let description = input_record
            .and_then(|record| record.get("description"))
            .and_then(value_to_string);
        let args = if command.is_empty() {
            String::new()
        } else {
            wrap_inline_code(command.trim())
        };
        let header = if args.is_empty() {
            "**Shell**".to_string()
        } else {
            format!("**Shell**({args})")
        };
        let trimmed_output = output.trim();
        if trimmed_output.is_empty() {
            return Some(header);
        }
        let title_text = description
            .as_deref()
            .filter(|d| !d.is_empty())
            .unwrap_or("Shell");
        let title = format!("\\# {title_text}");
        let body = format!("$ {command}\n{trimmed_output}");
        return Some(format!("{header}\n\n{title}\n\n```bash\n{body}\n```"));
    }
    // session-v2.tsx:748 Glob
    if name == "glob" {
        let pattern = input_string(input_record, "pattern")
            .or_else(|| input.as_str().map(ToString::to_string))?;
        let mut args = format!("pattern: {}", wrap_inline_code(&pattern));
        if let Some(path) = input_string(input_record, "path") {
            args.push_str(&format!(", path: {}", wrap_inline_code(&path)));
        }
        if let Some(count) = metadata
            .and_then(|m| m.get("count"))
            .and_then(value_to_string)
        {
            let plural = if count == "1" { "match" } else { "matches" };
            args.push_str(&format!(" — {count} {plural}"));
        }
        return Some(format!("**Glob**({args})"));
    }
    // session-v2.tsx:764 Read
    if name == "read" {
        let file_path = input_string(input_record, "filePath")
            .or_else(|| input.as_str().map(ToString::to_string))?;
        let other = input_record
            .map(|r| opencode_v2_input_other(r, &["filePath"]))
            .unwrap_or_default();
        let args = if other.is_empty() {
            wrap_inline_code(&file_path)
        } else {
            format!("{} {other}", wrap_inline_code(&file_path))
        };
        let mut text = format!("**Read**({args})");
        // Restore `metadata.loaded` (was `↳ Loaded {path}` per entry before
        // the unified-format refactor stripped it). Each entry on its own
        // line — trailing `\` is a CommonMark hard line break so the lines
        // don't collapse into a paragraph.
        if let Some(loaded) = metadata
            .and_then(|m| m.get("loaded"))
            .and_then(Value::as_array)
        {
            let paths: Vec<String> = loaded.iter().filter_map(value_to_string).collect();
            if !paths.is_empty() {
                text.push('\\');
                for path in &paths {
                    text.push_str("\n↳ Loaded ");
                    text.push_str(path);
                    text.push('\\');
                }
                // Drop the trailing `\` on the last line so it doesn't
                // produce an empty `<br>` after the final entry.
                if text.ends_with('\\') {
                    text.pop();
                }
            }
        }
        return Some(text);
    }
    // session-v2.tsx:794 Grep
    if name == "grep" {
        let pattern = input_string(input_record, "pattern")
            .or_else(|| input.as_str().map(ToString::to_string))?;
        let mut args = format!("pattern: {}", wrap_inline_code(&pattern));
        if let Some(path) = input_string(input_record, "path") {
            args.push_str(&format!(", path: {}", wrap_inline_code(&path)));
        }
        if let Some(matches) = metadata
            .and_then(|m| m.get("matches"))
            .and_then(value_to_string)
        {
            let plural = if matches == "1" { "match" } else { "matches" };
            args.push_str(&format!(" — {matches} {plural}"));
        }
        return Some(format!("**Grep**({args})"));
    }
    // session-v2.tsx:810 WebFetch
    if name == "webfetch" {
        let url = input_string(input_record, "url")
            .or_else(|| input.as_str().map(ToString::to_string))?;
        return Some(format!("**WebFetch**({})", wrap_inline_code(&url)));
    }
    // session-v2.tsx:818 WebSearch
    if name == "websearch" {
        let query = input_string(input_record, "query")
            .or_else(|| input.as_str().map(ToString::to_string))?;
        let mut args = wrap_inline_code(&query);
        if let Some(results) = metadata
            .and_then(|m| m.get("numResults"))
            .and_then(value_to_string)
        {
            args.push_str(&format!(" — {results} results"));
        }
        return Some(format!("**WebSearch**({args})"));
    }
    // session-v2.tsx:828 Write
    if name == "write" {
        let file_path = input_string(input_record, "filePath").unwrap_or_default();
        let args = if file_path.is_empty() {
            String::new()
        } else {
            wrap_inline_code(&file_path)
        };
        if state.get("status").and_then(Value::as_str) == Some("completed") {
            if let Some(content) = input_string(input_record, "content") {
                let lang = filetype_hint(&file_path);
                return Some(unified("Write", &args, &content, lang));
            }
        }
        let header = if args.is_empty() {
            "**Write**".to_string()
        } else {
            format!("**Write**({args})")
        };
        return Some(header);
    }
    // session-v2.tsx:857 Edit
    if name == "edit" {
        let file_path = input_string(input_record, "filePath").unwrap_or_default();
        let args = if file_path.is_empty() {
            String::new()
        } else {
            wrap_inline_code(&file_path)
        };
        if let Some(diff) = metadata
            .and_then(|m| m.get("diff"))
            .and_then(value_to_string)
        {
            return Some(unified("Edit", &args, &diff, "diff"));
        }
        let replace_all = input_record
            .and_then(|r| r.get("replaceAll"))
            .and_then(value_to_string);
        let header = if args.is_empty() {
            "**Edit**".to_string()
        } else {
            format!("**Edit**({args})")
        };
        return Some(match replace_all {
            Some(value) => format!("{header} [replaceAll={value}]"),
            None => header,
        });
    }
    // session-v2.tsx:891 ApplyPatch
    if name == "apply_patch" {
        if let Some(files) = metadata
            .and_then(|m| m.get("files"))
            .and_then(Value::as_array)
        {
            let blocks = files
                .iter()
                .filter_map(|file| {
                    let (verb, path) = opencode_v2_patch_file_verb_path(file)?;
                    let body = file
                        .get("patch")
                        .and_then(value_to_string)
                        .or_else(|| {
                            file.get("deletions")
                                .and_then(value_to_string)
                                .map(|n| format!("-{n} lines"))
                        })
                        .unwrap_or_default();
                    Some(unified(verb, &wrap_inline_code(&path), &body, "diff"))
                })
                .collect::<Vec<_>>();
            if !blocks.is_empty() {
                return Some(blocks.join("\n\n"));
            }
        }
        return Some("**ApplyPatch**".to_string());
    }
    // session-v2.tsx:964 TodoWrite — verb is "Todos" (matches the original
    // BlockTool title `\# Todos`).
    if name == "todowrite" {
        let metadata_has_todos = metadata
            .and_then(|m| m.get("todos"))
            .and_then(Value::as_array)
            .is_some_and(|todos| !todos.is_empty());
        let completed = state.get("status").and_then(Value::as_str) == Some("completed");
        if let Some(todos) = input_record
            .and_then(|r| r.get("todos"))
            .and_then(Value::as_array)
        {
            if (metadata_has_todos || completed) && !todos.is_empty() {
                let lines = todos
                    .iter()
                    .filter_map(|todo| {
                        let status = todo
                            .get("status")
                            .and_then(value_to_string)
                            .unwrap_or_default();
                        let content = todo.get("content").and_then(value_to_string)?;
                        Some(format!("{} {content}", todo_icon(&status)))
                    })
                    .collect::<Vec<_>>();
                if !lines.is_empty() {
                    return Some(format!("**Todos**\n\n{}", lines.join("\n")));
                }
            }
        }
        return Some("**Todos**".to_string());
    }
    // session-v2.tsx:991 Question — verb is "Questions" (matches the
    // original BlockTool title `\# Questions`).
    if name == "question" {
        let questions = input_record
            .and_then(|r| r.get("questions"))
            .and_then(Value::as_array);
        let answers = metadata
            .and_then(|m| m.get("answers"))
            .and_then(Value::as_array);
        if let (Some(questions), Some(answers)) = (questions, answers) {
            if !answers.is_empty() {
                let lines = questions
                    .iter()
                    .enumerate()
                    .filter_map(|(idx, question)| {
                        let question = question.get("question").and_then(value_to_string)?;
                        let answer = answers.get(idx).map(format_answer).unwrap_or_default();
                        Some(format!("{question}\n{answer}"))
                    })
                    .collect::<Vec<_>>();
                if !lines.is_empty() {
                    return Some(format!("**Questions**\n\n{}", lines.join("\n\n")));
                }
            }
        }
        let count = questions.map(|q| q.len()).unwrap_or(0);
        let plural = if count == 1 { "question" } else { "questions" };
        return Some(format!("**Questions**({count} {plural})"));
    }
    // session-v2.tsx:1022 Skill
    if name == "skill" {
        let skill = input_string(input_record, "name")
            .or_else(|| input.as_str().map(ToString::to_string))?;
        return Some(format!("**Skill**({})", wrap_inline_code(&skill)));
    }
    // session-v2.tsx:1030 Task — verb is `{Titlecase(agent)} Task`
    // (matches the original `{Agent} Task — description` heading where
    // the agent name prefixes "Task").
    if name == "task" {
        let description = input_string(input_record, "description")
            .or_else(|| input.as_str().map(ToString::to_string))?;
        let agent =
            input_string(input_record, "subagent_type").unwrap_or_else(|| "General".to_string());
        return Some(format!(
            "**{} Task**({})",
            title_case(&agent),
            wrap_inline_code(&description)
        ));
    }
    // session-v2.tsx:522 GenericTool fallback
    let input_text = input_record.map(opencode_v2_tool_input).unwrap_or_default();
    let header = if input_text.is_empty() {
        format!("**{name}**")
    } else {
        format!("**{name}**({input_text})")
    };
    if output.trim().is_empty() {
        Some(header)
    } else {
        Some(format!("{header}\n\n````\n{}\n````", output.trim()))
    }
}

// Helper functions ported from TUI conventions.

fn opencode_v2_input_other(input: &serde_json::Map<String, Value>, omit: &[&str]) -> String {
    let pairs = input
        .iter()
        .filter(|(key, _)| !omit.contains(&key.as_str()))
        .filter_map(|(key, value)| match value {
            Value::String(text) => Some(format!("{key}={text}")),
            Value::Number(number) => Some(format!("{key}={number}")),
            Value::Bool(boolean) => Some(format!("{key}={boolean}")),
            _ => None,
        })
        .collect::<Vec<_>>();
    if pairs.is_empty() {
        String::new()
    } else {
        format!("[{}]", pairs.join(", "))
    }
}

fn opencode_v2_patch_file_verb_path(file: &Value) -> Option<(&'static str, String)> {
    // session-v2.tsx:905-912 fileTitle() — picks the verb based on the
    // FileChange tag. Returns `(verb, path)` for the unified
    // `**Verb**(args)` header builder.
    let file_type = file.get("type").and_then(value_to_string);
    let relative_path = file
        .get("relativePath")
        .and_then(value_to_string)
        .or_else(|| file.get("filePath").and_then(value_to_string))
        .unwrap_or_else(|| "patch".to_string());
    Some(match file_type.as_deref() {
        Some("delete") => ("Deleted", relative_path),
        Some("add") => ("Created", relative_path),
        Some("move") => {
            let original = file
                .get("filePath")
                .and_then(value_to_string)
                .unwrap_or_default();
            ("Moved", format!("{original} → {relative_path}"))
        }
        _ => ("Patched", relative_path),
    })
}

fn todo_icon(status: &str) -> &'static str {
    // Matches packages/opencode/src/cli/cmd/tui/feature-plugins/system/session-v2.tsx
    // (todoIcon helper) — ✓ completed, ~ in_progress, ✕ cancelled, ☐ pending.
    match status {
        "completed" => "✓",
        "in_progress" => "~",
        "cancelled" => "✕",
        _ => "☐",
    }
}

fn filetype_hint(path: &str) -> &'static str {
    // Hint for the markdown renderer's syntax highlighter. Mirrors the
    // intent of the OpenCode TUI `filetype(...)` helper (returns a language
    // id for highlight.js). Conservative subset — unknown extensions fall
    // back to no hint so highlight.js can auto-detect.
    let lower = path.to_ascii_lowercase();
    match lower.rsplit('.').next() {
        Some("rs") => "rust",
        Some("ts" | "tsx") => "typescript",
        Some("js" | "jsx" | "mjs" | "cjs") => "javascript",
        Some("py") => "python",
        Some("go") => "go",
        Some("rb") => "ruby",
        Some("sh" | "bash" | "zsh") => "bash",
        Some("toml") => "toml",
        Some("yaml" | "yml") => "yaml",
        Some("json" | "jsonc") => "json",
        Some("md" | "mdx") => "markdown",
        Some("html" | "htm") => "html",
        Some("css") => "css",
        Some("scss" | "sass") => "scss",
        Some("c" | "h") => "c",
        Some("cpp" | "cc" | "hpp") => "cpp",
        Some("java") => "java",
        Some("kt") => "kotlin",
        Some("swift") => "swift",
        Some("sql") => "sql",
        Some("dockerfile") => "dockerfile",
        _ => "",
    }
}

fn input_string(input: Option<&serde_json::Map<String, Value>>, key: &str) -> Option<String> {
    input?.get(key).and_then(value_to_string)
}

fn format_answer(value: &Value) -> String {
    let Some(items) = value.as_array() else {
        return "(no answer)".to_string();
    };
    let answers = items.iter().filter_map(value_to_string).collect::<Vec<_>>();
    if answers.is_empty() {
        "(no answer)".to_string()
    } else {
        answers.join(", ")
    }
}

fn opencode_v2_tool_output(content: &Value) -> Option<String> {
    let items = content.as_array()?;
    let output = items
        .iter()
        .filter_map(|item| match item.get("type").and_then(Value::as_str)? {
            "text" => item
                .get("text")
                .and_then(value_to_string)
                .map(|text| text.trim().to_string()),
            "file" => {
                let name = item
                    .get("name")
                    .and_then(value_to_string)
                    .or_else(|| item.get("uri").and_then(value_to_string))
                    .unwrap_or_default();
                Some(format!("[file {name}]"))
            }
            _ => None,
        })
        .filter(|text| !text.is_empty())
        .collect::<Vec<_>>()
        .join("\n");
    (!output.is_empty()).then_some(output)
}

fn opencode_v2_tool_input(input: &serde_json::Map<String, Value>) -> String {
    let primitives = input
        .iter()
        .filter_map(|(key, value)| match value {
            Value::String(text) => Some(format!("{key}={text}")),
            Value::Number(number) => Some(format!("{key}={number}")),
            Value::Bool(boolean) => Some(format!("{key}={boolean}")),
            _ => None,
        })
        .collect::<Vec<_>>();
    if primitives.is_empty() {
        String::new()
    } else {
        format!("[{}]", primitives.join(", "))
    }
}

// Note: prior helpers (read_opencode_db_parts_text + opencode_part_text)
// joined every part into one text body. They were replaced by per-part
// emission inside read_opencode_db_messages so tool parts carry a TUI
// label and own their own SessionMessage. The synthetic / reasoning /
// tool dispatching that used to live here is now inline at the call site.

fn opencode_storage_roots(root: &Path) -> Vec<std::path::PathBuf> {
    let mut roots = Vec::new();
    let legacy = root.join("storage");
    if legacy.join("session").exists() {
        roots.push(legacy);
    }

    let project_root = root.join("project");
    if let Ok(entries) = fs::read_dir(project_root) {
        for entry in entries.filter_map(Result::ok) {
            let storage = entry.path().join("storage");
            if storage.join("session").exists() {
                roots.push(storage);
            }
        }
    }

    let global = root.join("global").join("storage");
    if global.join("session").exists() {
        roots.push(global);
    }
    roots
}

fn scan_opencode_storage(root: &Path) -> Result<Vec<AppSession>, Box<dyn Error>> {
    let sessions_root = root.join("session");
    if !sessions_root.exists() {
        return Ok(Vec::new());
    }
    let mut sessions = Vec::new();
    for entry in WalkDir::new(&sessions_root)
        .follow_links(false)
        .into_iter()
        .filter_map(Result::ok)
    {
        let path = entry.path();
        if !path.is_file() || path.extension().is_none_or(|ext| ext != "json") {
            continue;
        }
        if let Ok(detail) = parse_opencode_storage_session(path) {
            sessions.push(detail.session);
        }
    }
    Ok(sessions)
}

fn parse_opencode_storage_session(path: &Path) -> Result<SessionDetail, Box<dyn Error>> {
    let content = fs::read_to_string(path)?;
    let value = serde_json::from_str::<Value>(&content)?;
    if value.get("parentID").is_some() {
        return Err("OpenCode child session".into());
    }
    let id = value
        .get("id")
        .and_then(value_to_string)
        .ok_or("missing OpenCode session id")?;
    let project = value
        .get("directory")
        .and_then(value_to_string)
        .or_else(|| value.get("project").and_then(value_to_string))
        .ok_or("missing OpenCode session directory")?;
    let explicit_title = value
        .get("title")
        .and_then(value_to_string)
        .map(|title| opencode_display_title(&title))
        .and_then(|title| official_title_from_text(&title));
    let started_at = value
        .get("time")
        .and_then(|time| time.get("created"))
        .and_then(value_to_string)
        .or_else(|| value.get("created").and_then(value_to_string))
        .and_then(normalize_time);
    let updated_at = value
        .get("time")
        .and_then(|time| time.get("updated"))
        .and_then(value_to_string)
        .or_else(|| value.get("updated").and_then(value_to_string))
        .and_then(normalize_time)
        .or_else(|| started_at.clone());
    let messages = read_opencode_storage_messages(path, &id)?;
    let mut detail = detail_from_messages(
        "OpenCode",
        path,
        id,
        project,
        messages,
        Vec::new(),
        explicit_title,
    );
    detail.session.started_at = detail.session.started_at.or(started_at);
    detail.session.updated_at = updated_at.or(detail.session.updated_at);
    Ok(detail)
}

fn read_opencode_storage_messages(
    session_path: &Path,
    session_id: &str,
) -> Result<Vec<SessionMessage>, Box<dyn Error>> {
    let Some(storage_root) = opencode_storage_root_from_session_path(session_path) else {
        return Ok(Vec::new());
    };
    let message_root = storage_root.join("message").join(session_id);
    if !message_root.exists() {
        return Ok(Vec::new());
    }
    let mut messages = Vec::new();
    for entry in fs::read_dir(message_root)? {
        let path = entry?.path();
        if !path.is_file() || path.extension().is_none_or(|ext| ext != "json") {
            continue;
        }
        let content = fs::read_to_string(path)?;
        let value = serde_json::from_str::<Value>(&content)?;
        if let Some(message) = opencode_storage_message_from_value(&value) {
            messages.push(message);
        }
    }
    messages.sort_by(|a, b| a.timestamp.cmp(&b.timestamp));
    Ok(messages)
}

fn opencode_storage_root_from_session_path(path: &Path) -> Option<std::path::PathBuf> {
    let mut current = path.parent();
    while let Some(dir) = current {
        if dir.file_name().and_then(|name| name.to_str()) == Some("storage") {
            return Some(dir.to_path_buf());
        }
        current = dir.parent();
    }
    None
}

fn detail_from_messages(
    source: &str,
    path: &Path,
    id: String,
    project: String,
    messages: Vec<SessionMessage>,
    timestamps: Vec<String>,
    explicit_title: Option<String>,
) -> SessionDetail {
    let started_at = timestamps
        .iter()
        .filter_map(|t| normalize_time(t.clone()))
        .min();
    let updated_at = timestamps
        .iter()
        .filter_map(|t| normalize_time(t.clone()))
        .max();
    let title = explicit_title.unwrap_or_default();

    SessionDetail {
        session: AppSession {
            id,
            source: source.to_string(),
            title,
            project,
            path: path.display().to_string(),
            started_at,
            updated_at,
            message_count: messages.len(),
            preview: String::new(),
            message_previews: Vec::new(),
        },
        messages,
    }
}

fn opencode_storage_message_from_value(value: &Value) -> Option<SessionMessage> {
    let role = value.get("role").and_then(value_to_string)?;
    if role != "user" && role != "assistant" {
        return None;
    }
    let text = value
        .get("content")
        .and_then(opencode_storage_content_text)
        .or_else(|| value.get("text").and_then(value_to_string))?;
    let trimmed = text.trim();
    if trimmed.is_empty() {
        return None;
    }
    let timestamp = value
        .get("time")
        .and_then(value_to_string)
        .or_else(|| value.get("timestamp").and_then(value_to_string))
        .or_else(|| value.get("created").and_then(value_to_string))
        .and_then(normalize_time);
    Some(SessionMessage {
        role,
        text: trimmed.to_string(),
        timestamp,
        kind: kind::TEXT.to_string(),
        tool_use_id: None,
        exit_code: None,
    })
}

fn opencode_storage_content_text(value: &Value) -> Option<String> {
    let mut parts = Vec::new();
    match value {
        Value::String(text) => parts.push(text.clone()),
        Value::Array(items) => {
            for item in items {
                if item
                    .get("type")
                    .and_then(Value::as_str)
                    .is_some_and(|kind| kind != "text")
                {
                    continue;
                }
                if let Some(text) = item.get("text").and_then(value_to_string) {
                    parts.push(text);
                }
            }
        }
        _ => {}
    }
    let text = parts.join("\n").trim().to_string();
    if text.is_empty() {
        None
    } else {
        Some(text)
    }
}

fn opencode_display_title(title: &str) -> String {
    for prefix in ["New session", "Child session"] {
        let marker = format!("{prefix} - ");
        if let Some(timestamp) = title.strip_prefix(&marker) {
            if DateTime::parse_from_rfc3339(timestamp).is_ok() {
                return prefix.to_string();
            }
        }
    }
    title.to_string()
}

fn title_case(value: &str) -> String {
    value
        .split(['-', '_', ' '])
        .filter(|part| !part.is_empty())
        .map(|part| {
            let mut chars = part.chars();
            match chars.next() {
                Some(first) => {
                    first.to_uppercase().collect::<String>() + &chars.as_str().to_lowercase()
                }
                None => String::new(),
            }
        })
        .collect::<Vec<_>>()
        .join(" ")
}

fn claude_message_from_value(value: &Value) -> Vec<SessionMessage> {
    let mut out = Vec::new();
    if value.get("isSidechain").and_then(Value::as_bool) == Some(true) {
        return out;
    }
    let Some(entry_type) = value.get("type").and_then(Value::as_str) else {
        return out;
    };
    let timestamp = value
        .get("timestamp")
        .and_then(value_to_string)
        .and_then(normalize_time);

    match entry_type {
        "user" => {
            if value.get("isMeta").and_then(Value::as_bool) == Some(true) {
                return out;
            }
            let Some(content) = value.get("message").and_then(|m| m.get("content")) else {
                return out;
            };
            // 1. Plain text portion (skipped when empty).
            let text = claude_text_blocks(content).join("\n");
            if !text.trim().is_empty() {
                if let Some(display) = claude_display_text(&text) {
                    out.push(SessionMessage {
                        role: "user".to_string(),
                        text: display,
                        timestamp: timestamp.clone(),
                        kind: kind::TEXT.to_string(),
                        tool_use_id: None,
                        exit_code: None,
                    });
                }
            }
            // 2. Tool result blocks — surfaced as tool output messages so the
            //    transcript reflects what each prior tool call returned. Carry
            //    the tool_use_id so merge_tool_outputs can fold the result
            //    back into the matching tool_use card.
            //
            //    For Edit / MultiEdit / Write tools, Claude also stores a
            //    `toolUseResult.structuredPatch` sibling field on the JSONL
            //    line (next to `message`) — this is what Claude TUI's
            //    `StructuredDiff.tsx` renders as a unified diff with
            //    `+`/`-` lines. We prefer that diff over the brief
            //    "The file ... has been updated successfully" content
            //    when present, matching the official UI's verbosity.
            let tool_use_result = value.get("toolUseResult");
            let diff_text = tool_use_result.and_then(claude_format_structured_patch);
            for tool_result in claude_tool_results(content) {
                let body_text = diff_text
                    .clone()
                    .filter(|_| !tool_result.is_error)
                    .or_else(|| claude_display_text(&tool_result.text));
                if let Some(display) = body_text {
                    out.push(SessionMessage {
                        role: "tool".to_string(),
                        text: display,
                        timestamp: timestamp.clone(),
                        kind: if tool_result.is_error {
                            kind::TOOL_ERROR.to_string()
                        } else {
                            kind::TOOL_RESULT.to_string()
                        },
                        tool_use_id: tool_result.tool_use_id,
                        exit_code: None,
                    });
                }
            }
        }
        "assistant" => {
            let Some(content) = value.get("message").and_then(|m| m.get("content")) else {
                return out;
            };
            // 1. Extended-thinking blocks render BEFORE the visible text
            //    (matching `AssistantThinkingMessage`'s `∴ Thinking…`
            //    indented dim markdown placement in claude-code TUI).
            //    Format via the shared `format_reasoning_body` helper so
            //    Claude / Codex / OpenCode all use the same `> *content*`
            //    italic-blockquote convention.
            for thinking in claude_thinking_blocks(content) {
                let formatted = format_reasoning_body(&thinking);
                if formatted.is_empty() {
                    continue;
                }
                out.push(SessionMessage {
                    role: "assistant".to_string(),
                    text: formatted,
                    timestamp: timestamp.clone(),
                    kind: kind::REASONING.to_string(),
                    tool_use_id: None,
                    exit_code: None,
                });
            }
            // 2. Plain assistant text.
            let text = claude_assistant_text_blocks(content).join("\n");
            if !text.trim().is_empty() {
                if let Some(display) = claude_display_text(&text) {
                    out.push(SessionMessage {
                        role: "assistant".to_string(),
                        text: display,
                        timestamp: timestamp.clone(),
                        kind: kind::TEXT.to_string(),
                        tool_use_id: None,
                        exit_code: None,
                    });
                }
            }
            // 3. Tool use blocks — emitted as separate tool messages, each
            //    labeled with the canonical TUI tool name (Bash/Update/Read/...).
            //    The tool_use.id is preserved so the eventual tool_result can
            //    be paired up and merged into this card.
            for tool_use in claude_tool_uses(content) {
                out.push(SessionMessage {
                    role: "tool".to_string(),
                    text: tool_use.text,
                    timestamp: timestamp.clone(),
                    kind: kind::TOOL_USE.to_string(),
                    tool_use_id: tool_use.id,
                    exit_code: None,
                });
            }
        }
        "system" if value.get("subtype").and_then(Value::as_str) == Some("local_command") => {
            if let Some(text) = value.get("content").and_then(value_to_string) {
                if let Some(display) = claude_display_text(&text) {
                    out.push(SessionMessage {
                        role: "tool".to_string(),
                        text: display,
                        timestamp,
                        kind: kind::LOCAL_COMMAND.to_string(),
                        tool_use_id: None,
                        exit_code: None,
                    });
                }
            }
        }
        // System subtype dispatch. Branches mirror
        // `.audit-sources/claude-code/src/components/messages/SystemTextMessage.tsx`:
        //   - `turn_duration` (line 342-401): `※ Worked for {duration}` italic dim
        //   - `away_summary` (line 70-84): `※ {content}` italic dim
        //   - `compact_boundary` (Message.tsx:195-203): divider notice
        //   - `microcompact_boundary` (Message.tsx:204-207): null
        //   - `api_error`: hidden unless verbose (SystemTextMessage.tsx:139-145)
        // Other subtypes drop silently (matches verbose-only fallthrough).
        "system" => {
            if let Some(msg) = claude_system_message(value, timestamp) {
                out.push(msg);
            }
        }
        // Attachment records — dispatched by `attachment.type` per Claude
        // TUI's `AttachmentMessage.tsx`. Many subtypes are NULL_RENDERING
        // (drop silently). The rest emit a short notice line matching
        // the TUI's `<Line>` output.
        "attachment" => {
            out.extend(claude_attachment_messages(value, timestamp));
        }
        _ => {}
    }
    out
}

/// Render a Claude `system` record (non-`local_command` subtypes) to a
/// single SessionMessage. Mirrors `SystemTextMessage.tsx`'s per-subtype
/// branches; drops subtypes that Claude TUI also hides (api_error /
/// microcompact_boundary / verbose-only generic info).
fn claude_system_message(value: &Value, timestamp: Option<String>) -> Option<SessionMessage> {
    let subtype = value.get("subtype").and_then(Value::as_str)?;
    let make = |text: String| {
        Some(SessionMessage {
            role: "system".to_string(),
            text,
            timestamp: timestamp.clone(),
            kind: kind::TEXT.to_string(),
            tool_use_id: None,
            exit_code: None,
        })
    };
    match subtype {
        // SystemTextMessage.tsx:342-401 — `※ Worked for {duration}`.
        // The TUI samples a verb from TURN_COMPLETION_VERBS ("Worked",
        // "Toiled", "Labored", etc.); Termory uses a stable "Worked"
        // since session replay needs a deterministic transcript.
        "turn_duration" => {
            let ms = value.get("durationMs").and_then(Value::as_i64)?;
            make(format!("*※ Worked for {}*", format_duration_short(ms)))
        }
        // SystemTextMessage.tsx:70-84 — `※ {content}` dim italic.
        "away_summary" => {
            let content = value.get("content").and_then(Value::as_str)?;
            let trimmed = content.trim();
            if trimmed.is_empty() {
                None
            } else {
                make(format!("*※ {trimmed}*"))
            }
        }
        // SystemTextMessage.tsx:87-101 — red BLACK_CIRCLE + dim text.
        "agents_killed" => make("**Error** All background agents stopped".to_string()),
        // Message.tsx:195-203 — `<CompactBoundaryMessage>` divider.
        // The actual TUI renders a horizontal rule with a label; in
        // markdown we approximate with `---` GFM divider + italic notice.
        "compact_boundary" => {
            let content = value
                .get("content")
                .and_then(Value::as_str)
                .map(str::trim)
                .filter(|s| !s.is_empty());
            let body = match content {
                Some(c) => format!("---\n\n*{c}*\n\n---"),
                None => "---\n\n*Conversation compacted*\n\n---".to_string(),
            };
            make(body)
        }
        // microcompact_boundary → null (Message.tsx:204-207)
        // api_error → verbose-only (SystemTextMessage.tsx:139-145)
        // Other subtypes drop silently.
        _ => None,
    }
}

/// Compact human-readable duration (`45269 ms` → `45.3s`).
/// Used by `turn_duration` system messages.
fn format_duration_short(ms: i64) -> String {
    if ms < 0 {
        return "0s".to_string();
    }
    let secs = ms as f64 / 1000.0;
    if secs < 60.0 {
        format!("{:.1}s", secs)
    } else if secs < 3600.0 {
        let m = (secs / 60.0).floor();
        let s = secs - m * 60.0;
        format!("{}m {:.0}s", m as i64, s)
    } else {
        let h = (secs / 3600.0).floor();
        let m = ((secs - h * 3600.0) / 60.0).floor();
        format!("{}h {}m", h as i64, m as i64)
    }
}

/// Render a Claude `attachment` record to zero-or-one SessionMessages.
/// Branches mirror `.audit-sources/claude-code/src/components/messages/AttachmentMessage.tsx:162-450`.
/// NULL_RENDERING types (task_reminder / deferred_tools_delta /
/// command_permissions / date_change / hook_success / async_hook_response
/// / agent_setting / etc.) return an empty list to match Claude's
/// `nullRenderingAttachments.ts:14-49`.
fn claude_attachment_messages(value: &Value, timestamp: Option<String>) -> Vec<SessionMessage> {
    let Some(att) = value.get("attachment") else {
        return Vec::new();
    };
    let Some(att_type) = att.get("type").and_then(Value::as_str) else {
        return Vec::new();
    };
    let push = |text: String| {
        vec![SessionMessage {
            role: "system".to_string(),
            text,
            timestamp: timestamp.clone(),
            kind: kind::TEXT.to_string(),
            tool_use_id: None,
            exit_code: None,
        }]
    };
    let display_path = att
        .get("displayPath")
        .and_then(Value::as_str)
        .or_else(|| att.get("filename").and_then(Value::as_str))
        .unwrap_or("");
    match att_type {
        // AttachmentMessage.tsx:163-168
        "directory" => push(format!("Listed directory {}", wrap_inline_code(&format!("{display_path}/")))),
        // AttachmentMessage.tsx:169-194
        "file" | "already_read_file" => {
            let content = att.get("content");
            let inner_type = content
                .and_then(|c| c.get("type"))
                .and_then(Value::as_str)
                .unwrap_or("");
            let detail = match inner_type {
                "notebook" => {
                    let cells = content
                        .and_then(|c| c.get("file"))
                        .and_then(|f| f.get("cells"))
                        .and_then(Value::as_array)
                        .map(|c| c.len())
                        .unwrap_or(0);
                    format!("{cells} cells")
                }
                "file_unchanged" => "unchanged".to_string(),
                "text" => {
                    let num_lines = content
                        .and_then(|c| c.get("file"))
                        .and_then(|f| f.get("numLines"))
                        .and_then(Value::as_i64)
                        .unwrap_or(0);
                    let truncated = att
                        .get("truncated")
                        .and_then(Value::as_bool)
                        .unwrap_or(false);
                    let plus = if truncated { "+" } else { "" };
                    format!("{num_lines}{plus} lines")
                }
                _ => {
                    // image/pdf/parts paths use `file.originalSize`
                    let size = content
                        .and_then(|c| c.get("file"))
                        .and_then(|f| f.get("originalSize"))
                        .and_then(Value::as_i64);
                    match size {
                        Some(s) => format!("{} bytes", s),
                        None => String::new(),
                    }
                }
            };
            if detail.is_empty() {
                push(format!("Read {}", wrap_inline_code(display_path)))
            } else {
                push(format!(
                    "Read {} ({detail})",
                    wrap_inline_code(display_path)
                ))
            }
        }
        // AttachmentMessage.tsx:195-200
        "compact_file_reference" => {
            push(format!("Referenced file {}", wrap_inline_code(display_path)))
        }
        // AttachmentMessage.tsx:201-207
        "pdf_reference" => {
            let pages = att
                .get("pageCount")
                .and_then(Value::as_i64)
                .unwrap_or(0);
            push(format!(
                "Referenced PDF {} ({pages} pages)",
                wrap_inline_code(display_path)
            ))
        }
        // AttachmentMessage.tsx:208-216
        "selected_lines_in_ide" => {
            let start = att.get("lineStart").and_then(Value::as_i64).unwrap_or(0);
            let end = att.get("lineEnd").and_then(Value::as_i64).unwrap_or(0);
            let lines = end - start + 1;
            let ide = att.get("ideName").and_then(Value::as_str).unwrap_or("");
            push(format!(
                "⧉ Selected {lines} lines from {} in {ide}",
                wrap_inline_code(display_path)
            ))
        }
        // AttachmentMessage.tsx:217-222
        "nested_memory" => push(format!("Loaded {}", wrap_inline_code(display_path))),
        // AttachmentMessage.tsx:280-290 — isInitial returns null
        "skill_listing" => {
            if att.get("isInitial").and_then(Value::as_bool) == Some(true) {
                return Vec::new();
            }
            let count = att.get("skillCount").and_then(Value::as_i64).unwrap_or(0);
            let noun = if count == 1 { "skill" } else { "skills" };
            push(format!("{count} {noun} available"))
        }
        // AttachmentMessage.tsx:302-322 — queued_command wraps a prompt
        // that itself goes through the UserTextMessage XML-wrapper
        // dispatch chain (i.e. `<task-notification>` etc. apply).
        "queued_command" => {
            let prompt = att.get("prompt");
            let text = match prompt {
                Some(Value::String(s)) => s.clone(),
                Some(Value::Array(items)) => items
                    .iter()
                    .filter_map(|item| {
                        if item.get("type").and_then(Value::as_str) == Some("text") {
                            item.get("text").and_then(value_to_string)
                        } else {
                            None
                        }
                    })
                    .collect::<Vec<_>>()
                    .join("\n"),
                _ => String::new(),
            };
            if text.trim().is_empty() {
                Vec::new()
            } else {
                claude_display_text(&text)
                    .map(push)
                    .unwrap_or_default()
            }
        }
        // AttachmentMessage.tsx:324-329
        "plan_file_reference" => {
            let plan_path = att
                .get("planFilePath")
                .and_then(Value::as_str)
                .unwrap_or("");
            push(format!(
                "Plan file referenced ({})",
                wrap_inline_code(plan_path)
            ))
        }
        // AttachmentMessage.tsx:330-336
        "invoked_skills" => {
            let names: Vec<String> = att
                .get("skills")
                .and_then(Value::as_array)
                .map(|arr| {
                    arr.iter()
                        .filter_map(|s| s.get("name").and_then(value_to_string))
                        .collect()
                })
                .unwrap_or_default();
            if names.is_empty() {
                Vec::new()
            } else {
                push(format!("Skills restored ({})", names.join(", ")))
            }
        }
        // AttachmentMessage.tsx:339-345
        "mcp_resource" => {
            let name = att.get("name").and_then(Value::as_str).unwrap_or("");
            let server = att.get("server").and_then(Value::as_str).unwrap_or("");
            push(format!(
                "Read MCP resource {} from {server}",
                wrap_inline_code(name)
            ))
        }
        // NULL_RENDERING per nullRenderingAttachments.ts:14-49 and
        // explicit returns in AttachmentMessage.tsx — drop silently.
        "task_reminder"
        | "deferred_tools_delta"
        | "command_permissions"
        | "date_change"
        | "hook_success"
        | "async_hook_response"
        | "agent_setting"
        | "relevant_memories" // Usually absorbed into a CollapsedReadSearchGroup; safe to drop for now
        | "dynamic_skill"
        | "agent_listing_delta" => Vec::new(),
        // Unknown attachment type — drop (matches Claude TUI's
        // `_exhaustive` fall-through).
        _ => Vec::new(),
    }
}

struct ClaudeToolResult {
    text: String,
    is_error: bool,
    tool_use_id: Option<String>,
}

/// Build a unified-diff fence from Claude's `toolUseResult.structuredPatch`
/// array — the same data Claude TUI's `StructuredDiff.tsx` consumes.
/// Each hunk has `{oldStart, oldLines, newStart, newLines, lines: [...]}`
/// where `lines` entries are already prefixed with ` ` (context),
/// `+` (added), or `-` (removed). Termory wraps the output in a
/// ```diff fence so the markdown layer can syntax-highlight it later.
///
/// Returns None when the field is missing or empty (caller falls back
/// to the brief tool_result `content` text).
fn claude_format_structured_patch(tool_use_result: &Value) -> Option<String> {
    let hunks = tool_use_result.get("structuredPatch")?.as_array()?;
    if hunks.is_empty() {
        return None;
    }
    let mut out = String::from("```diff\n");
    for hunk in hunks {
        let old_start = hunk.get("oldStart").and_then(Value::as_i64).unwrap_or(0);
        let old_lines = hunk.get("oldLines").and_then(Value::as_i64).unwrap_or(0);
        let new_start = hunk.get("newStart").and_then(Value::as_i64).unwrap_or(0);
        let new_lines = hunk.get("newLines").and_then(Value::as_i64).unwrap_or(0);
        out.push_str(&format!(
            "@@ -{old_start},{old_lines} +{new_start},{new_lines} @@\n"
        ));
        if let Some(lines) = hunk.get("lines").and_then(Value::as_array) {
            for line in lines.iter().filter_map(Value::as_str) {
                out.push_str(line);
                out.push('\n');
            }
        }
    }
    out.push_str("```");
    Some(out)
}

fn claude_tool_results(content: &Value) -> Vec<ClaudeToolResult> {
    let Value::Array(items) = content else {
        return Vec::new();
    };
    items
        .iter()
        .filter(|item| item.get("type").and_then(Value::as_str) == Some("tool_result"))
        .filter_map(|item| {
            let text = match item.get("content") {
                Some(Value::String(s)) => s.clone(),
                Some(Value::Array(blocks)) => blocks
                    .iter()
                    .filter_map(|block| {
                        if block.get("type").and_then(Value::as_str) == Some("text") {
                            block.get("text").and_then(value_to_string)
                        } else {
                            None
                        }
                    })
                    .collect::<Vec<_>>()
                    .join("\n"),
                _ => return None,
            };
            if text.trim().is_empty() {
                return None;
            }
            Some(ClaudeToolResult {
                text,
                is_error: item
                    .get("is_error")
                    .and_then(Value::as_bool)
                    .unwrap_or(false),
                tool_use_id: item.get("tool_use_id").and_then(value_to_string),
            })
        })
        .collect()
}

struct ClaudeToolUse {
    text: String,
    id: Option<String>,
}

fn claude_tool_uses(content: &Value) -> Vec<ClaudeToolUse> {
    let Value::Array(items) = content else {
        return Vec::new();
    };
    items
        .iter()
        .filter(|item| item.get("type").and_then(Value::as_str) == Some("tool_use"))
        .filter_map(|item| {
            let name = item
                .get("name")
                .and_then(value_to_string)
                .unwrap_or_default();
            if name.is_empty() {
                return None;
            }
            let input = item.get("input").unwrap_or(&Value::Null);
            // None → tool suppressed in Claude TUI (userFacingName='' AND
            // renderToolUseMessage returns null). Skip the whole card.
            let text = claude_tool_use_text(&name, input)?;
            Some(ClaudeToolUse {
                text,
                id: item.get("id").and_then(value_to_string),
            })
        })
        .collect()
}

/// Format a Claude tool_use block as the TUI's `**{userFacingName}**(...)`
/// inline. Each branch mirrors the Tool's `renderToolUseMessage` output
/// (returns the content inside the parens) and the Tool's `userFacingName`
/// (the bold label).
///
/// Sources in .audit-sources/claude-code/src/tools/:
/// * src/components/messages/AssistantToolUseMessage.tsx:152 — wrapper
///   emits `<bold>{userFacingName}</bold>(<text>{render output}</text>)`.
/// * BashTool/UI.tsx renderToolUseMessage — returns the command.
/// * FileReadTool/UI.tsx → "Read", returns displayPath.
/// * FileWriteTool/UI.tsx → "Write", returns displayPath.
/// * FileEditTool/UI.tsx → "Update", returns displayPath.
/// * GrepTool.ts:170 → "Search", returns `pattern: "{pattern}"` (+ `, path: ...`).
/// * GlobTool/UI.tsx:13 → "Search", same `pattern: "..."` format.
/// * WebFetchTool.ts:81 → "Fetch", returns the URL.
/// * WebSearchTool.ts:160 → "Web Search", returns `"{query}"`.
/// Detect agent-output file paths used by Claude Code's TaskOutput tool.
/// Pattern: `…/tasks/{taskId}.output` where taskId is alphanumeric +
/// `_-` and ≤20 chars. Matches `FileReadTool/UI.tsx:18-33`.
fn claude_agent_output_task_id(path: &str) -> Option<String> {
    let stripped = path.strip_suffix(".output")?;
    let after_tasks = stripped.rsplit_once("/tasks/")?.1;
    if after_tasks.is_empty() || after_tasks.len() > 20 {
        return None;
    }
    if after_tasks
        .chars()
        .all(|c| c.is_ascii_alphanumeric() || c == '_' || c == '-')
    {
        Some(after_tasks.to_string())
    } else {
        None
    }
}

fn claude_tool_use_text(name: &str, input: &Value) -> Option<String> {
    let lower = name.to_ascii_lowercase();
    let obj = input.as_object();
    let get = |key: &str| obj.and_then(|o| o.get(key)).and_then(value_to_string);
    // Tools that Claude TUI suppresses entirely (`userFacingName: ''`
    // AND `renderToolUseMessage: () => null`). The entire `<bold>x</bold>(args)`
    // card is hidden. We return None so the caller skips emitting any
    // SessionMessage. Per `.audit-sources/claude-code/src/tools/`:
    //   * TodoWriteTool.ts:48,62
    //   * AskUserQuestionTool.tsx:222-278
    //   * EnterPlanModeTool.ts:52
    //   * ExitPlanModeTool.ts / ExitPlanModeV2Tool.ts:163
    //   * TaskCreateTool.ts:64,77 + sibling TaskUpdate/Get/List/Stop/Output
    //   * ToolSearchTool.ts:438
    match lower.as_str() {
        "todowrite" | "todo_write" | "askuserquestion" | "ask_user_question" | "enterplanmode"
        | "enter_plan_mode" | "exitplanmode" | "exit_plan_mode" | "exitplanmodev2"
        | "taskcreate" | "task_create" | "taskupdate" | "task_update" | "taskget" | "task_get"
        | "tasklist" | "task_list" | "taskstop" | "task_stop" | "taskoutput" | "task_output"
        | "toolsearch" | "tool_search" => return None,
        _ => {}
    }
    // All dynamic arguments (commands, paths, URLs, patterns) are wrapped
    // with `wrap_inline_code` so any markdown special characters inside the
    // user payload (backticks, `*`, `_`, `()`) don't leak into the rendered
    // output. The TUI version (Claude Code Ink) doesn't need this because
    // it bypasses markdown entirely.
    let text = match lower.as_str() {
        "bash" => {
            let command = get("command").unwrap_or_default();
            if command.is_empty() {
                "**Bash**".to_string()
            } else {
                format!("**Bash**({})", wrap_inline_code(&command))
            }
        }
        "read" | "view" => {
            let path = get("file_path").unwrap_or_default();
            if path.is_empty() {
                return Some("**Read**".to_string());
            }
            // FileReadTool/UI.tsx userFacingName (l.179-187): the verb
            // varies by file path. Agent-output reads (path matches
            // `{tempDir}/tasks/{taskId}.output`) use verb
            // "Read agent output" and show just the taskId as args.
            // Plans-directory variant is skipped — `getPlansDirectory()`
            // depends on session config Termory doesn't have access to.
            if let Some(task_id) = claude_agent_output_task_id(&path) {
                return Some(format!(
                    "**Read agent output**({})",
                    wrap_inline_code(&task_id)
                ));
            }
            let pages = get("pages");
            let offset = get("offset");
            let limit = get("limit");
            let mut text = format!("**Read**({}", wrap_inline_code(&path));
            if let Some(p) = pages {
                text.push_str(&format!(" · pages {p}"));
            } else if let Some(off) = offset.as_deref() {
                match limit.as_deref() {
                    Some(lim) => text.push_str(&format!(" · lines {off}-{lim}")),
                    None => text.push_str(&format!(" · from line {off}")),
                }
            } else if let Some(lim) = limit {
                text.push_str(&format!(" · limit {lim}"));
            }
            text.push(')');
            text
        }
        "write" => {
            let path = get("file_path").unwrap_or_default();
            if path.is_empty() {
                "**Write**".to_string()
            } else {
                format!("**Write**({})", wrap_inline_code(&path))
            }
        }
        "edit" | "multiedit" | "str_replace" | "str_replace_editor" => {
            let path = get("file_path").unwrap_or_default();
            // FileEditTool/UI.tsx:28-87 — verb defaults to "Update", but
            // becomes "Create" when `old_string === ''` (new file being
            // written via edit). MultiEdit checks the first edit.
            let creating = match lower.as_str() {
                "multiedit" => input
                    .get("edits")
                    .and_then(Value::as_array)
                    .and_then(|arr| arr.first())
                    .and_then(|e| e.get("old_string"))
                    .and_then(Value::as_str)
                    .is_some_and(str::is_empty),
                _ => get("old_string").is_some_and(|s| s.is_empty()),
            };
            let verb = if creating { "Create" } else { "Update" };
            if path.is_empty() {
                format!("**{verb}**")
            } else {
                format!("**{verb}**({})", wrap_inline_code(&path))
            }
        }
        "grep" => {
            let pattern = get("pattern").unwrap_or_default();
            if pattern.is_empty() && get("path").is_none() {
                return Some("**Search**".to_string());
            }
            let mut parts: Vec<String> = Vec::new();
            if !pattern.is_empty() {
                parts.push(format!("pattern: {}", wrap_inline_code(&pattern)));
            }
            if let Some(path) = get("path") {
                parts.push(format!("path: {}", wrap_inline_code(&path)));
            }
            format!("**Search**({})", parts.join(", "))
        }
        "glob" => {
            let pattern = get("pattern").unwrap_or_default();
            let path = get("path");
            match (pattern.is_empty(), path.as_deref()) {
                (true, None) => "**Search**".to_string(),
                (false, Some(p)) => format!(
                    "**Search**(pattern: {}, path: {})",
                    wrap_inline_code(&pattern),
                    wrap_inline_code(p)
                ),
                (false, None) => format!("**Search**(pattern: {})", wrap_inline_code(&pattern)),
                (true, Some(p)) => format!("**Search**(path: {})", wrap_inline_code(p)),
            }
        }
        "websearch" | "web_search" => {
            let query = get("query").unwrap_or_default();
            if query.is_empty() {
                "**Web Search**".to_string()
            } else {
                format!("**Web Search**({})", wrap_inline_code(&query))
            }
        }
        "webfetch" | "web_fetch" => {
            let url = get("url").unwrap_or_default();
            if url.is_empty() {
                "**Fetch**".to_string()
            } else {
                format!("**Fetch**({})", wrap_inline_code(&url))
            }
        }
        // AgentTool userFacingName depends on `subagent_type`
        // (AgentTool/UI.tsx export userFacingName):
        //   * `worker` → "Agent"
        //   * `general-purpose` (or missing) → "Agent" (description as args)
        //   * other subagent_type → use it verbatim
        // renderToolUseMessage emits the description.
        "task" | "agent" => {
            let description = get("description").unwrap_or_default();
            let subagent_type = get("subagent_type").unwrap_or_default();
            let verb = if subagent_type.is_empty()
                || subagent_type == "general-purpose"
                || subagent_type == "worker"
            {
                "Agent".to_string()
            } else {
                subagent_type
            };
            if description.is_empty() {
                format!("**{verb}**")
            } else {
                format!("**{verb}**({})", wrap_inline_code(&description))
            }
        }
        // SkillTool/UI.tsx — emits `skill` or `/skill` (legacy commands).
        // Termory just shows `**Skill**({name})` to keep it predictable.
        "skill" => {
            let skill = get("skill").or_else(|| get("name")).unwrap_or_default();
            if skill.is_empty() {
                "**Skill**".to_string()
            } else {
                format!("**Skill**({})", wrap_inline_code(&skill))
            }
        }
        // NotebookEditTool/NotebookEditTool.ts userFacingName → "Edit Notebook".
        // Renders `**Edit Notebook**({notebook_path})`.
        "notebookedit" | "notebook_edit" => {
            let path = get("notebook_path")
                .or_else(|| get("file_path"))
                .unwrap_or_default();
            if path.is_empty() {
                "**Edit Notebook**".to_string()
            } else {
                format!("**Edit Notebook**({})", wrap_inline_code(&path))
            }
        }
        // ReadMcpResourceTool/UI.tsx userFacingName → literal 'readMcpResource'
        // (camelCase, matches Claude TUI exactly).
        "readmcpresource" | "read_mcp_resource" => {
            let uri = get("uri").or_else(|| get("url")).unwrap_or_default();
            if uri.is_empty() {
                "**readMcpResource**".to_string()
            } else {
                format!("**readMcpResource**({})", wrap_inline_code(&uri))
            }
        }
        // ListMcpResourcesTool userFacingName → literal 'listMcpResources'.
        "listmcpresources" | "list_mcp_resources" => {
            let server = get("server").unwrap_or_default();
            if server.is_empty() {
                "**listMcpResources**".to_string()
            } else {
                format!("**listMcpResources**({})", wrap_inline_code(&server))
            }
        }
        // McpAuthTool userFacingName → '{server} - authenticate (MCP)'.
        // Whole label IS the verb (bold).
        "mcpauth" | "mcp_auth" => {
            let server = get("server").unwrap_or_default();
            if server.is_empty() {
                "**authenticate (MCP)**".to_string()
            } else {
                format!("**{server} - authenticate (MCP)**")
            }
        }
        _ => {
            // MCP tools follow the canonical name pattern
            // `mcp__{server}__{tool}` (Claude Code's MCP client
            // strips the prefix in display). Surface them as
            // `**MCP**({server}/{tool})` so they look the same as
            // the Codex `EventMsg::McpToolCallEnd` rendering.
            if let Some(rest) = name.strip_prefix("mcp__") {
                if let Some((server, tool)) = rest.split_once("__") {
                    return Some(format!("**MCP**({server}/{tool})"));
                }
            }
            // Otherwise: bold raw name + compact JSON args.
            let json = serde_json::to_string(input).unwrap_or_default();
            if json == "null" || json == "{}" {
                format!("**{name}**")
            } else {
                format!("**{name}**({})", compact(&json, 200))
            }
        }
    };
    Some(text)
}

fn claude_text_blocks(value: &Value) -> Vec<String> {
    match value {
        Value::String(text) => vec![text.clone()],
        Value::Array(items) => items
            .iter()
            .filter_map(|item| match item.get("type").and_then(Value::as_str)? {
                "text" => item.get("text").and_then(value_to_string),
                // Anthropic image content blocks
                // (`{"type":"image","source":{"type":"base64"|"url",...}}`):
                // surface as an italic notice with the mime type / URL so
                // the transcript shows that an image was attached. The
                // base64 payload is too large to render inline.
                "image" => Some(claude_image_part_label(item.get("source")?)),
                _ => None,
            })
            .collect(),
        _ => Vec::new(),
    }
}

fn claude_image_part_label(source: &Value) -> String {
    let kind = source.get("type").and_then(Value::as_str).unwrap_or("");
    match kind {
        "base64" => {
            let mime = source
                .get("media_type")
                .and_then(Value::as_str)
                .unwrap_or("application/octet-stream");
            format!("*Image ({mime})*")
        }
        "url" => {
            let url = source.get("url").and_then(Value::as_str).unwrap_or("");
            if url.is_empty() {
                "*Image*".to_string()
            } else {
                format!("*Image: {url}*")
            }
        }
        _ => "*Image*".to_string(),
    }
}

fn claude_assistant_text_blocks(value: &Value) -> Vec<String> {
    match value {
        Value::String(text) => vec![text.clone()],
        Value::Array(items) => items
            .iter()
            .filter_map(|item| {
                if item.get("type").and_then(Value::as_str) == Some("text") {
                    item.get("text").and_then(value_to_string)
                } else {
                    None
                }
            })
            .collect(),
        _ => Vec::new(),
    }
}

/// Extract Claude extended-thinking blocks
/// (`{"type":"thinking","thinking":"..."}` and `redacted_thinking`).
/// Claude Code's TUI renders these via `AssistantThinkingMessage` as
/// `∴ Thinking…` + dim markdown body. We surface them as separate
/// reasoning messages (italic blockquote via `format_reasoning_body`)
/// instead of dropping the content.
fn claude_thinking_blocks(value: &Value) -> Vec<String> {
    let Value::Array(items) = value else {
        return Vec::new();
    };
    items
        .iter()
        .filter_map(|item| match item.get("type").and_then(Value::as_str)? {
            "thinking" => item.get("thinking").and_then(value_to_string),
            "redacted_thinking" => Some("[redacted thinking]".to_string()),
            _ => None,
        })
        .collect()
}

fn claude_user_content_is_tool_result_only(value: &Value) -> bool {
    let Some(items) = value.as_array() else {
        return false;
    };
    !items.is_empty()
        && items
            .iter()
            .all(|item| item.get("type").and_then(Value::as_str) == Some("tool_result"))
}

fn claude_display_text(text: &str) -> Option<String> {
    // Claude's internal `<tool_use_error>...</tool_use_error>` wrapper is
    // not meaningful to humans — it's an Anthropic-internal marker. Strip
    // it to show just the inner error message.
    if let Some(inner) = extract_xml_tag_value(text, "tool_use_error") {
        let trimmed = inner.trim();
        if !trimmed.is_empty() {
            return Some(trimmed.to_string());
        }
    }
    // Bash tool_result wrappers: `<bash-stdout>...</bash-stdout>` and
    // `<bash-stderr>...</bash-stderr>`. Claude TUI's `UserBashOutputMessage`
    // extracts both and renders stdout + stderr. We concatenate stdout
    // first, then stderr (prefixed) so the unified card carries both
    // streams. Also unwrap the inner `<persisted-output>` wrapper that
    // claude-code adds for long captured bash output.
    if text.contains("<bash-stdout>") || text.contains("<bash-stderr>") {
        let stdout = extract_xml_tag_value(text, "bash-stdout")
            .map(|s| extract_xml_tag_value(&s, "persisted-output").unwrap_or(s))
            .map(|s| s.trim().to_string())
            .filter(|s| !s.is_empty());
        let stderr = extract_xml_tag_value(text, "bash-stderr")
            .map(|s| s.trim().to_string())
            .filter(|s| !s.is_empty());
        let combined = match (stdout, stderr) {
            (Some(o), Some(e)) => format!("{o}\n\n{e}"),
            (Some(o), None) => o,
            (None, Some(e)) => e,
            (None, None) => return None,
        };
        return Some(combined);
    }
    // Background-task notifications (`<task-notification>...`) emitted by
    // LocalShellTask / LocalAgentTask / RemoteAgentTask. Claude Code's
    // `UserAgentNotificationMessage.tsx` extracts only `<summary>` and
    // shows it as `⏺ {summary}` (with status-derived color). Termory
    // emits the same `⏺ {summary}` prefix; the other tags (task-id /
    // output-file / event / status) are dropped to match Claude TUI.
    if text.contains("<task-notification>") || text.contains("<task-notification ") {
        if let Some(summary) = extract_xml_tag_value(text, "summary") {
            let trimmed = summary.trim();
            if !trimmed.is_empty() {
                return Some(format!("{}{trimmed}", status_marker(false)));
            }
        }
    }
    if text.contains("<command-message>") {
        let command_message = extract_xml_tag_value(text, "command-message")?;
        let args = extract_xml_tag_value(text, "command-args");
        let content = [Some(command_message), args]
            .into_iter()
            .flatten()
            .collect::<Vec<_>>()
            .join(" ");
        return non_empty_xml_value(format!("/{}", content.trim()));
    }
    if let Some(bash_input) = extract_xml_tag_value(text, "bash-input") {
        return non_empty_xml_value(format!("! {bash_input}"));
    }
    if let Some(stdout) = extract_xml_tag_value(text, "local-command-stdout") {
        return Some(non_empty_xml_value(stdout).unwrap_or_else(|| "(no content)".to_string()));
    }
    if let Some(stderr) = extract_xml_tag_value(text, "local-command-stderr") {
        return Some(non_empty_xml_value(stderr).unwrap_or_else(|| "(no content)".to_string()));
    }
    let trimmed = text.trim();
    if trimmed.is_empty() {
        None
    } else {
        let stripped = strip_display_tags_allow_empty(trimmed);
        let display = if stripped.is_empty() {
            trimmed
        } else {
            &stripped
        };
        Some(display.to_string())
    }
}

fn extract_xml_tag_value(text: &str, tag: &str) -> Option<String> {
    let start_tag = format!("<{tag}>");
    let end_tag = format!("</{tag}>");
    let start = text.find(&start_tag)? + start_tag.len();
    let end = text[start..].find(&end_tag)? + start;
    Some(text[start..end].trim().to_string())
}

fn non_empty_xml_value(value: String) -> Option<String> {
    if value.is_empty() {
        None
    } else {
        Some(value)
    }
}

fn codex_message_from_value(value: &Value) -> Option<SessionMessage> {
    let item_type = value.get("type").and_then(Value::as_str)?;
    let payload = value.get("payload")?;
    let timestamp = value
        .get("timestamp")
        .and_then(value_to_string)
        .and_then(normalize_time);

    // `event_msg` records are what Codex's TUI uses to render replayed sessions
    // (see `ThreadHistoryBuilder::handle_event` in
    // codex-rs/app-server-protocol/src/protocol/thread_history.rs). For shell
    // tool calls Codex pulls raw `aggregated_output` from
    // `EventMsg::ExecCommandEnd`, NOT from `ResponseItem::FunctionCallOutput`
    // (which only contains the model-facing `Chunk ID: ...Output: ...` format
    // and is a no-op during replay).
    if item_type == "event_msg" {
        return codex_event_msg_to_message(payload, timestamp);
    }

    if item_type != "response_item" {
        return None;
    }
    let payload_type = payload
        .get("type")
        .and_then(Value::as_str)
        .unwrap_or("event");

    // For Codex CLI sessions the default rollout mode is `Limited`
    // (codex-rs/tui/src/app_server_session.rs: `persist_extended_history:
    // false`). In Limited mode the `EventMsg::ExecCommandEnd` event is NOT
    // persisted (see codex-rs/rollout/src/policy.rs:166 — it's
    // Extended-only). So the only source for shell command + output in
    // these sessions is `ResponseItem::FunctionCall` +
    // `ResponseItem::FunctionCallOutput`. We process them here as a
    // fallback. When a session DOES have `EventMsg::ExecCommandEnd` (the
    // canonical source — Extended mode), `merge_tool_outputs` will dedupe
    // by `call_id` and the EventMsg-derived message wins.
    //
    // We do skip `Message` and `Reasoning` ResponseItems unconditionally
    // because their EventMsg counterparts (`UserMessage`, `AgentMessage`,
    // `AgentReasoning`) are persisted in BOTH modes (policy.rs:137-153).
    if payload_type == "function_call" {
        return codex_function_call_message(payload, timestamp);
    }
    if payload_type == "function_call_output" {
        return codex_function_call_output_message(payload, timestamp);
    }
    // `custom_tool_call` / `custom_tool_call_output` are the modern Codex
    // shape for apply_patch (and other "custom" tools — see
    // `codex-rs/protocol/src/models.rs ResponseItem::CustomToolCall`). The
    // structural difference vs `function_call` is:
    //   * input arrives in an `input` field (raw text), NOT `arguments`
    //     (which is reserved for JSON-encoded structured args).
    //   * output is wrapped in a JSON envelope `{"output": "..."}`.
    // Reusing `codex_function_call_message` / `_output_message` after a
    // field rename keeps apply_patch / patch-diff rendering identical.
    if payload_type == "custom_tool_call" {
        return codex_custom_tool_call_message(payload, timestamp);
    }
    if payload_type == "custom_tool_call_output" {
        return codex_custom_tool_call_output_message(payload, timestamp);
    }
    if payload_type == "command_execution" {
        return codex_command_execution_message(payload, timestamp);
    }
    if payload_type == "plan" {
        return codex_plan_message(payload, timestamp);
    }
    if payload_type == "reasoning" {
        // Reasoning content is delivered via `EventMsg::AgentReasoning` in
        // both Limited and Extended modes — skip the ResponseItem variant
        // to avoid duplicate reasoning cards.
        return None;
    }
    if let Some(message) = codex_fallback_event_message(payload_type, payload, timestamp.clone()) {
        return Some(message);
    }
    if payload_type != "message" {
        return None;
    }
    // `Message` ResponseItems are also covered by `EventMsg::UserMessage` /
    // `AgentMessage` in both persistence modes. Skip them to avoid
    // duplicates. (Hook prompts use a different parser path, not handled
    // here.)
    None
}

fn codex_content_text(value: &Value) -> Option<String> {
    let mut parts = Vec::new();
    match value {
        Value::Array(items) => {
            for item in items {
                if let Some(text) = item.get("text").and_then(value_to_string) {
                    parts.push(text);
                }
            }
        }
        Value::String(text) => parts.push(text.clone()),
        _ => {}
    }
    let text = parts.join("\n").trim().to_string();
    if text.is_empty() {
        None
    } else {
        Some(text)
    }
}

fn codex_visible_assistant_markdown(markdown: &str) -> String {
    let mut visible_lines = markdown
        .lines()
        .map(|line| strip_codex_git_directives(line).trim_end().to_string())
        .collect::<Vec<_>>();
    while visible_lines.last().is_some_and(String::is_empty) {
        visible_lines.pop();
    }
    visible_lines.join("\n")
}

fn strip_codex_git_directives(line: &str) -> String {
    let mut visible = String::new();
    let mut remaining = line;
    while let Some(start) = remaining.find("::git-") {
        visible.push_str(&remaining[..start]);
        let directive = &remaining[start + 2..];
        let Some(open_brace) = directive.find('{') else {
            visible.push_str(&remaining[start..]);
            return visible;
        };
        let Some(close_brace) = directive[open_brace + 1..].find('}') else {
            visible.push_str(&remaining[start..]);
            return visible;
        };
        let close_brace = open_brace + 1 + close_brace;
        remaining = &directive[close_brace + 1..];
    }
    visible.push_str(remaining);
    visible
}

fn is_codex_contextual_user_text(text: &str) -> bool {
    let trimmed = text.trim_start();
    [
        "<environment_context>",
        "<user_instructions>",
        "<skill_instructions>",
        "<user_shell_command>",
        "<turn_aborted>",
        "<subagent_notification>",
        "<goal_context>",
        "<legacy_unified_exec_process_limit_warning>",
        "<legacy_apply_patch_exec_command_warning>",
        "<legacy_model_mismatch_warning>",
    ]
    .iter()
    .any(|tag| trimmed.starts_with(tag))
}

fn codex_plan_message(payload: &Value, timestamp: Option<String>) -> Option<SessionMessage> {
    let text = payload.get("text").and_then(value_to_string)?;
    if text.trim().is_empty() {
        return None;
    }
    Some(SessionMessage {
        role: "assistant".to_string(),
        text: text.trim().to_string(),
        timestamp,
        kind: kind::PLAN.to_string(),
        tool_use_id: None,
        exit_code: None,
    })
}

fn codex_reasoning_message(payload: &Value, timestamp: Option<String>) -> Option<SessionMessage> {
    let summary = payload
        .get("summary")
        .and_then(Value::as_array)
        .map(|items| {
            items
                .iter()
                .filter_map(value_to_string)
                .collect::<Vec<_>>()
                .join("\n\n")
        })
        .or_else(|| payload.get("summary").and_then(value_to_string));
    let content = payload
        .get("content")
        .and_then(Value::as_array)
        .map(|items| {
            items
                .iter()
                .filter_map(value_to_string)
                .collect::<Vec<_>>()
                .join("\n\n")
        })
        .or_else(|| payload.get("content").and_then(value_to_string));
    let text = summary.or(content)?;
    if text.trim().is_empty() {
        return None;
    }
    Some(SessionMessage {
        role: "assistant".to_string(),
        text: text.trim().to_string(),
        timestamp,
        kind: kind::REASONING.to_string(),
        tool_use_id: None,
        exit_code: None,
    })
}

fn codex_function_call_message(
    payload: &Value,
    timestamp: Option<String>,
) -> Option<SessionMessage> {
    // Each branch produces the unified Termory header markdown — bold verb
    // + inline-code argument (or a fenced block for content that doesn't
    // fit on one line). Output, when present, is appended later by
    // `merge_tool_outputs` in a 4-backtick fence. Codex's official TUI
    // uses bold verbs (`"Ran"` / `"You ran"`) per exec_cell/render.rs:381.
    let name = payload
        .get("name")
        .and_then(value_to_string)
        .unwrap_or_else(|| "tool".to_string());
    let arguments = payload.get("arguments").and_then(value_to_string);
    let body = match name.as_str() {
        // Codex treats these 4 names as shell-exec calls, per
        // .audit-sources/codex/codex-rs/rollout-trace/src/tool_dispatch.rs:263:
        //   `"exec_command" | "local_shell" | "shell" | "shell_command"`
        // The TUI verb is `"Ran"` (or `"You ran"` for user-initiated shells)
        // — exec_cell/render.rs:381-385.
        "exec_command" | "shell" | "shell_command" | "local_shell" => {
            // Unified shell verb across Codex / Claude — both render as
            // `**Bash**({wrap_inline_code(command)})` so the user sees the
            // same shape regardless of platform. Codex's TUI uses "Ran"
            // (exec_cell/render.rs:381-385) but the per-platform card
            // badge already disambiguates the source; using one verb
            // makes scanning the transcript easier.
            let command = codex_command_from_arguments(arguments.as_deref())
                .or_else(|| arguments.clone())
                .unwrap_or_default();
            let trimmed = command.trim();
            if trimmed.is_empty() {
                "**Bash**".to_string()
            } else {
                format!("**Bash**({})", wrap_inline_code(trimmed))
            }
        }
        "apply_patch" => {
            let patch = codex_apply_patch_text(arguments.as_deref())
                .or_else(|| arguments.clone())
                .unwrap_or_default();
            let header = codex_patch_header(&codex_parse_patch_actions(&patch));
            format!("{header}\n\n```diff\n{}\n```", patch.trim())
        }
        // Codex `update_plan` tool — TUI shows a checkbox-style plan via
        // PlanUpdateCell (history_cell/plans.rs:138-194 emits `✔` for
        // completed, `□` for in_progress / pending). We map to GFM task
        // lists which remark-gfm renders with checkbox UI:
        //   - [x] completed
        //   - [~] in_progress  (`[~]` is a common extension marker)
        //   - [ ] pending
        "update_plan" => codex_update_plan_message(arguments.as_deref()),
        // Codex `view_image` tool — TUI shows `• Viewed Image` + indented
        // path (history_cell/patches.rs:63-72). We map to `**Viewed image**
        // \`{path}\`` so the path stays monospaced.
        "view_image" => {
            let path = arguments
                .as_deref()
                .and_then(|args| {
                    serde_json::from_str::<Value>(args)
                        .ok()
                        .and_then(|v| v.get("path").and_then(value_to_string))
                })
                .unwrap_or_default();
            if path.is_empty() {
                "**Viewed image**".to_string()
            } else {
                format!("**Viewed image**({})", wrap_inline_code(&path))
            }
        }
        _ => {
            // Generic fallback: `{name}({compact args})` as plain text.
            let args_summary = arguments
                .as_deref()
                .map(|args| compact(args, 200))
                .unwrap_or_default();
            if args_summary.is_empty() {
                name.clone()
            } else {
                format!("{name}({args_summary})")
            }
        }
    };
    Some(SessionMessage {
        role: "tool".to_string(),
        text: body,
        timestamp,
        kind: kind::TOOL_USE.to_string(),
        tool_use_id: payload.get("call_id").and_then(value_to_string),
        exit_code: None,
    })
}

/// Build the unified-markdown body for a Codex `update_plan` tool call.
/// Matches the structure of `PlanUpdateCell.display_lines`:
///   * `**Updated plan**` header
///   * optional explanation in italic
///   * each plan step rendered as a GFM task list entry with status marker
fn codex_update_plan_message(arguments: Option<&str>) -> String {
    let mut text = String::from("**Updated plan**");
    let Some(args) = arguments else {
        return text;
    };
    let Ok(value) = serde_json::from_str::<Value>(args.trim()) else {
        return text;
    };
    if let Some(explanation) = value
        .get("explanation")
        .and_then(Value::as_str)
        .map(str::trim)
        .filter(|s| !s.is_empty())
    {
        text.push_str("\n\n*");
        // Escape stray `*` so italic span isn't broken mid-line.
        for c in explanation.chars() {
            if c == '*' || c == '_' {
                text.push('\\');
            }
            text.push(c);
        }
        text.push('*');
    }
    let plan = value.get("plan").and_then(Value::as_array);
    let Some(items) = plan else {
        return text;
    };
    if items.is_empty() {
        return text;
    }
    text.push_str("\n");
    for item in items {
        let step = item
            .get("step")
            .and_then(Value::as_str)
            .unwrap_or("")
            .trim();
        if step.is_empty() {
            continue;
        }
        let status = item.get("status").and_then(Value::as_str).unwrap_or("");
        let marker = match status {
            "completed" => "- [x]",
            "in_progress" => "- [~]",
            _ => "- [ ]",
        };
        text.push('\n');
        text.push_str(marker);
        text.push(' ');
        text.push_str(step);
    }
    text
}

fn codex_apply_patch_text(arguments: Option<&str>) -> Option<String> {
    let arguments = arguments?.trim();
    if arguments.is_empty() {
        return None;
    }
    let value = serde_json::from_str::<Value>(arguments).ok()?;
    value
        .get("input")
        .and_then(value_to_string)
        .or_else(|| value.get("patch").and_then(value_to_string))
}

/// Codex apply_patch verb per file action. Mirrors
/// `.audit-sources/codex/codex-rs/tui/src/diff_render.rs:421-436` which
/// picks "Added" / "Deleted" / "Edited" depending on the FileChange
/// variant. We don't have the structured FileChange in Limited-mode
/// rollouts, so we scan the patch text for `*** Add File:` /
/// `*** Delete File:` / `*** Update File:` markers from Codex's
/// apply_patch grammar.
#[derive(Debug, PartialEq, Eq)]
struct CodexPatchAction {
    verb: &'static str,
    path: String,
    move_to: Option<String>,
}

fn codex_parse_patch_actions(patch_text: &str) -> Vec<CodexPatchAction> {
    let mut actions: Vec<CodexPatchAction> = Vec::new();
    let mut lines = patch_text.lines().peekable();
    while let Some(line) = lines.next() {
        let trimmed = line.trim_start();
        let (verb, rest) = if let Some(rest) = trimmed.strip_prefix("*** Add File:") {
            ("Added", rest)
        } else if let Some(rest) = trimmed.strip_prefix("*** Delete File:") {
            ("Deleted", rest)
        } else if let Some(rest) = trimmed.strip_prefix("*** Update File:") {
            ("Edited", rest)
        } else {
            continue;
        };
        let path = rest.trim().to_string();
        if path.is_empty() {
            continue;
        }
        let mut move_to = None;
        if verb == "Edited" {
            if let Some(next) = lines.peek() {
                if let Some(rest) = next.trim_start().strip_prefix("*** Move to:") {
                    move_to = Some(rest.trim().to_string());
                    lines.next();
                }
            }
        }
        actions.push(CodexPatchAction {
            verb,
            path,
            move_to,
        });
    }
    actions
}

/// Build the unified-markdown header for an apply_patch call. Matches
/// Codex's `render_changes_block` (diff_render.rs:415-437) for the verb
/// choice, but uses the unified `**Verb**(args)` shape so the card
/// reads the same as `**Bash**(cmd)` / `**Edit Notebook**(path)`:
///   * single-file patch → `**{Verb}**(\`{path}\`)` (with optional `→ {move_to}`)
///   * multi-file patch  → `**Edited**({N} files)`
fn codex_patch_header(actions: &[CodexPatchAction]) -> String {
    if actions.is_empty() {
        return "**Applied patch**".to_string();
    }
    if let [single] = actions {
        let path_segment = match &single.move_to {
            Some(target) => format!("{} → {}", single.path, target),
            None => single.path.clone(),
        };
        return format!("**{}**({})", single.verb, wrap_inline_code(&path_segment));
    }
    let count = actions.len();
    let noun = if count == 1 { "file" } else { "files" };
    format!("**Edited**({count} {noun})")
}

/// Codex `custom_tool_call` (`payload.type = "custom_tool_call"`) is the
/// modern shape for apply_patch and similar tools. Differs from
/// `function_call` in:
///   * `input` (raw text) instead of `arguments` (JSON-encoded args)
///   * No structured args wrapper around the patch body
/// Real-world example (`/Users/john/.codex/sessions/.../*.jsonl`):
///   `{"type":"custom_tool_call","name":"apply_patch","input":"*** Begin Patch\n*** Update File: ...","call_id":"..."}`
fn codex_custom_tool_call_message(
    payload: &Value,
    timestamp: Option<String>,
) -> Option<SessionMessage> {
    let name = payload.get("name").and_then(value_to_string)?;
    let input = payload.get("input").and_then(value_to_string);
    let body = match name.as_str() {
        "apply_patch" => {
            let patch = input.clone().unwrap_or_default();
            let header = codex_patch_header(&codex_parse_patch_actions(&patch));
            format!("{header}\n\n```diff\n{}\n```", patch.trim())
        }
        _ => {
            // Generic custom tool: bold name + compact input.
            let compact_input = input
                .as_deref()
                .map(|s| compact(s, 200))
                .unwrap_or_default();
            if compact_input.is_empty() {
                format!("**{name}**")
            } else {
                format!("**{name}**({compact_input})")
            }
        }
    };
    Some(SessionMessage {
        role: "tool".to_string(),
        text: body,
        timestamp,
        kind: kind::TOOL_USE.to_string(),
        tool_use_id: payload.get("call_id").and_then(value_to_string),
        exit_code: None,
    })
}

/// Codex `custom_tool_call_output` — paired with `custom_tool_call`.
/// The `output` field is a JSON-encoded string like
/// `{"output":"Success. Updated:\n..."}` (extracted from the same source
/// file referenced above). We unwrap the JSON envelope and emit a
/// tool_result message that `merge_tool_outputs` will fold back into
/// the matching tool_use card.
fn codex_custom_tool_call_output_message(
    payload: &Value,
    timestamp: Option<String>,
) -> Option<SessionMessage> {
    let raw_output = payload
        .get("output")
        .and_then(value_to_string)
        .or_else(|| payload.get("content").and_then(value_to_string))?;
    // Try JSON unwrap: many custom tool outputs use `{"output":"..."}`
    // / `{"text":"..."}` envelopes. Fall back to the raw string.
    let unwrapped = serde_json::from_str::<Value>(&raw_output)
        .ok()
        .and_then(|value| {
            value
                .get("output")
                .or_else(|| value.get("text"))
                .or_else(|| value.get("result"))
                .and_then(value_to_string)
        })
        .unwrap_or(raw_output);
    let trimmed = unwrapped.trim();
    if trimmed.is_empty() {
        return None;
    }
    Some(SessionMessage {
        role: "tool".to_string(),
        text: trimmed.to_string(),
        timestamp,
        kind: kind::TOOL_RESULT.to_string(),
        tool_use_id: payload.get("call_id").and_then(value_to_string),
        exit_code: None,
    })
}

fn codex_function_call_output_message(
    payload: &Value,
    timestamp: Option<String>,
) -> Option<SessionMessage> {
    let text = payload
        .get("output")
        .and_then(value_to_string)
        .or_else(|| payload.get("content").and_then(value_to_string))?;
    let extracted = codex_parse_exec_output(&text);
    if extracted.raw.is_empty() && extracted.exit_code.is_none() {
        return None;
    }
    let is_error = extracted.exit_code.is_some_and(|code| code != 0);
    Some(SessionMessage {
        role: "tool".to_string(),
        text: extracted.raw,
        timestamp,
        kind: if is_error {
            kind::TOOL_ERROR.to_string()
        } else {
            kind::TOOL_RESULT.to_string()
        },
        tool_use_id: payload.get("call_id").and_then(value_to_string),
        exit_code: extracted.exit_code,
    })
}

/// Strip Codex's `ExecCommandToolOutput.response_text()` metadata wrapper.
/// The wrapper format (codex-rs/core/src/tools/context.rs:409-434) is:
///
/// ```text
/// [Chunk ID: X]
/// Wall time: X seconds
/// [Process exited with code X]
/// [Process running with session ID X]
/// [Original token count: X]
/// Output:
/// {raw command output}
/// ```
///
/// All sections are joined with single `\n`. The metadata is only useful
/// to the model; Codex's TUI never displays it in the main viewport (it
/// uses `aggregated_output` directly from `EventMsg::ExecCommandEnd`).
/// In Limited-mode rollouts the EventMsg is not persisted, so this
/// wrapper string is our only handle on the raw output.
///
/// Detection is line-by-line so we tolerate trailing whitespace on the
/// `Output:` delimiter and don't depend on an exact `\nOutput:\n`
/// substring match. Also pulls the exit code out of the metadata lines
/// (`Process exited with code N` / `Exit code: N`) so the caller can
/// surface a `**Error**` footer on non-zero exits.
#[derive(Debug, PartialEq, Eq, Default)]
struct CodexExecOutput {
    raw: String,
    exit_code: Option<i64>,
}

fn codex_parse_exec_output(text: &str) -> CodexExecOutput {
    let trimmed = text.trim();
    if trimmed.is_empty() {
        return CodexExecOutput::default();
    }
    let first_line = trimmed.lines().next().unwrap_or("");
    let looks_like_exec_wrapper = first_line.starts_with("Chunk ID:")
        || first_line.starts_with("Wall time:")
        || first_line.starts_with("Exit code:")
        || first_line.starts_with("Script ");
    if !looks_like_exec_wrapper {
        return CodexExecOutput {
            raw: trimmed.to_string(),
            exit_code: None,
        };
    }
    // The wrapper format guarantees a single `Output:` delimiter line
    // before the actual raw output. Split on it.
    let mut output_started = false;
    let mut output_lines: Vec<&str> = Vec::new();
    let mut exit_code: Option<i64> = None;
    for line in trimmed.lines() {
        if output_started {
            output_lines.push(line);
            continue;
        }
        // Extract exit code from one of the canonical metadata lines.
        // Codex's context.rs:420 emits `"Process exited with code {N}"`;
        // the older `unified_exec` format uses `"Exit code: {N}"`.
        if exit_code.is_none() {
            let trimmed_line = line.trim_start();
            if let Some(rest) = trimmed_line.strip_prefix("Process exited with code ") {
                exit_code = rest.trim().parse().ok();
            } else if let Some(rest) = trimmed_line.strip_prefix("Exit code:") {
                exit_code = rest.trim().parse().ok();
            }
        }
        if line.trim_end() == "Output:" {
            output_started = true;
        }
    }
    if !output_started {
        // Wrapper-looking content but missing the `Output:` marker
        // (truncated stream, etc.) — keep the original text and exit code.
        return CodexExecOutput {
            raw: trimmed.to_string(),
            exit_code,
        };
    }
    CodexExecOutput {
        raw: output_lines.join("\n").trim_end().to_string(),
        exit_code,
    }
}

fn codex_command_execution_message(
    payload: &Value,
    timestamp: Option<String>,
) -> Option<SessionMessage> {
    let command = payload.get("command").and_then(value_to_string)?;
    let mut lines = vec![format!("$ {command}")];
    if let Some(status) = payload.get("status").and_then(value_to_string) {
        let mut status_line = format!("status: {status}");
        if let Some(exit_code) = payload.get("exit_code").and_then(value_to_string) {
            status_line.push_str(" · exit ");
            status_line.push_str(&exit_code);
        }
        lines.push(status_line);
    }
    if let Some(output) = payload.get("aggregated_output").and_then(value_to_string) {
        let output = output.trim();
        if !output.is_empty() {
            lines.extend(output.lines().map(|line| format!("  {}", line.trim_end())));
        }
    }
    Some(SessionMessage {
        role: "tool".to_string(),
        text: lines.join("\n"),
        timestamp,
        kind: kind::COMMAND_EXECUTION.to_string(),
        tool_use_id: None,
        exit_code: None,
    })
}

/// Dispatch by the inner `type` of a Codex `RolloutItem::EventMsg` payload.
/// Mirrors `ThreadHistoryBuilder::handle_event` (in
/// codex-rs/app-server-protocol/src/protocol/thread_history.rs:163-225) — the
/// same reducer Codex's TUI uses to rehydrate replayed sessions.
fn codex_event_msg_to_message(
    payload: &Value,
    timestamp: Option<String>,
) -> Option<SessionMessage> {
    let event_type = payload.get("type").and_then(Value::as_str)?;
    match event_type {
        "user_message" => codex_user_message_event(payload, timestamp),
        "agent_message" => codex_agent_message_event(payload, timestamp),
        "agent_reasoning" => codex_agent_reasoning_event(payload, timestamp),
        // Raw chain-of-thought (`AgentReasoningRawContentEvent`,
        // protocol.rs:2157). Codex's TUI renders this with the same
        // `ReasoningSummaryCell` style when `show_raw_agent_reasoning`
        // is on — and `replay_thread_item` ALSO emits it via
        // `on_agent_reasoning_delta` (replay.rs:118-121). Termory
        // surfaces it as a reasoning message using the same italic
        // blockquote format so it's distinguishable from regular text.
        "agent_reasoning_raw_content" => codex_agent_reasoning_event(payload, timestamp),
        "web_search_end" => codex_web_search_end_event(payload, timestamp),
        "mcp_tool_call_end" => codex_mcp_tool_call_end_event(payload, timestamp),
        "image_generation_end" => codex_image_generation_end_event(payload, timestamp),
        // `EventMsg::ViewImageToolCall` ({call_id, path}) — separate from
        // the `function_call` shape; Codex emits this when the agent
        // attaches a local image. patches.rs:63-72 renders it as
        // `• Viewed Image\n  └ {path}`. Map to the same markdown form
        // we use for the function_call variant for consistency.
        "view_image_tool_call" => codex_view_image_tool_call_event(payload, timestamp),
        "plan_update" => codex_plan_update_event(payload, timestamp),
        // `EventMsg::PatchApplyEnd` (Extended-mode rollouts only) carries
        // the structured `changes: HashMap<PathBuf, FileChange>` map and a
        // `success: bool`. Codex's TUI renders this via PatchHistoryCell
        // → `create_diff_summary` (per-file colored summary). We approximate
        // by listing each file with its verb (Added/Deleted/Edited) and
        // marking the apply as failed when `success: false`.
        "patch_apply_end" => codex_patch_apply_end_event(payload, timestamp),
        "context_compacted" => codex_context_compacted_event(payload, timestamp),
        // Lifecycle notices — Codex's TUI emits these as small one-line
        // info cards. Termory surfaces them as plain text so the
        // transcript carries the signal but they don't masquerade as
        // tool messages.
        "error" => codex_error_event(payload, timestamp),
        "turn_aborted" => codex_turn_aborted_event(payload, timestamp),
        "thread_rolled_back" => codex_thread_rolled_back_event(payload, timestamp),
        "entered_review_mode" => codex_entered_review_mode_event(payload, timestamp),
        "exited_review_mode" => codex_exited_review_mode_event(payload, timestamp),
        // `exec_command_end` is intentionally NOT handled yet. In Extended
        // mode it would supply better data than the ResponseItem path
        // (raw `aggregated_output` instead of `Chunk ID:...Output:...`),
        // but in Limited mode (the CLI default) it's simply not persisted,
        // so the ResponseItem path is authoritative. Mixing the two
        // currently produces duplicate tool cards for Extended-mode
        // sessions. Handle when we add proper call_id-based deduplication.
        _ => None,
    }
}

/// `EventMsg::WebSearchEnd` — Codex renders this via WebSearchCell with a
/// `Searched ...` header showing the query (or the structured action).
/// Unified-markdown form: `**Searched** "{query}"`.
fn codex_web_search_end_event(
    payload: &Value,
    timestamp: Option<String>,
) -> Option<SessionMessage> {
    let query = payload
        .get("query")
        .and_then(Value::as_str)
        .filter(|s| !s.trim().is_empty())?;
    Some(SessionMessage {
        role: "tool".to_string(),
        text: format!("**Searched**({})", wrap_inline_code(query)),
        timestamp,
        kind: kind::TOOL_USE.to_string(),
        tool_use_id: payload.get("call_id").and_then(value_to_string),
        exit_code: None,
    })
}

/// `EventMsg::McpToolCallEnd` — Codex renders via McpToolCallCell. The
/// invocation has `server`/`tool`/`arguments`; the result is either the
/// successful CallToolResult or an error string. Unified-markdown form:
/// `**MCP** {server}/{tool}` + result body (4-backtick fence) + optional
/// `**Error**` footer (added by merge_tool_outputs when kind=tool_error).
fn codex_mcp_tool_call_end_event(
    payload: &Value,
    timestamp: Option<String>,
) -> Option<SessionMessage> {
    let invocation = payload.get("invocation")?;
    let server = invocation
        .get("server")
        .and_then(Value::as_str)
        .unwrap_or("");
    let tool = invocation.get("tool").and_then(Value::as_str).unwrap_or("");
    if server.is_empty() && tool.is_empty() {
        return None;
    }
    let header = format!("**MCP**({server}/{tool})");
    // `result` is either `{"Ok": CallToolResult}` or `{"Err": "..."}`
    // (Rust `Result` serialization). Inspect both arms.
    let result = payload.get("result");
    let (body, is_error) = match result {
        Some(r) if r.get("Err").is_some() => {
            let err = r.get("Err").and_then(Value::as_str).unwrap_or("");
            (err.to_string(), true)
        }
        Some(r) if r.get("Ok").is_some() => {
            let ok = r.get("Ok").expect("Ok arm");
            let is_err = ok.get("is_error").and_then(Value::as_bool).unwrap_or(false);
            let content_text = mcp_call_tool_result_text(ok);
            (content_text, is_err)
        }
        _ => (String::new(), false),
    };
    let trimmed = body.trim();
    let mut text = header;
    if !trimmed.is_empty() {
        text.push_str("\n\n````\n");
        text.push_str(trimmed);
        text.push_str("\n````");
    }
    Some(SessionMessage {
        role: "tool".to_string(),
        text,
        timestamp,
        kind: if is_error {
            kind::TOOL_ERROR.to_string()
        } else {
            kind::TOOL_RESULT.to_string()
        },
        tool_use_id: payload.get("call_id").and_then(value_to_string),
        exit_code: None,
    })
}

/// Extract human-readable text from an MCP `CallToolResult`. Joins all
/// `content[].text` entries with newlines; image / resource items are
/// skipped because they aren't representable in plain markdown.
fn mcp_call_tool_result_text(result: &Value) -> String {
    let Some(items) = result.get("content").and_then(Value::as_array) else {
        return String::new();
    };
    let mut parts: Vec<String> = Vec::new();
    for item in items {
        if let Some(text) = item.get("text").and_then(Value::as_str) {
            parts.push(text.to_string());
        }
    }
    parts.join("\n")
}

/// `EventMsg::ImageGenerationEnd` — Codex shows "Generated Image:"
/// with the (optional) revised prompt and saved file URL.
/// Unified-markdown form: `**Generated image** {revised_prompt}` +
/// optional `\nSaved: {path}` line.
fn codex_image_generation_end_event(
    payload: &Value,
    timestamp: Option<String>,
) -> Option<SessionMessage> {
    let revised = payload
        .get("revised_prompt")
        .and_then(Value::as_str)
        .map(str::trim)
        .filter(|s| !s.is_empty());
    let saved = payload
        .get("saved_path")
        .and_then(Value::as_str)
        .map(str::trim)
        .filter(|s| !s.is_empty());
    if revised.is_none() && saved.is_none() {
        return None;
    }
    let mut text = String::from("**Generated image**");
    if let Some(prompt) = revised {
        text.push('(');
        text.push_str(&wrap_inline_code(prompt));
        text.push(')');
    }
    if let Some(path) = saved {
        text.push_str("\n\nSaved: ");
        text.push_str(&wrap_inline_code(path));
    }
    Some(SessionMessage {
        role: "tool".to_string(),
        text,
        timestamp,
        kind: kind::TOOL_USE.to_string(),
        tool_use_id: payload.get("call_id").and_then(value_to_string),
        exit_code: None,
    })
}

/// `EventMsg::ContextCompacted` — Codex shows a brief "Context compacted"
/// info line in the transcript. Unified-markdown form: italic notice.
fn codex_context_compacted_event(
    _payload: &Value,
    timestamp: Option<String>,
) -> Option<SessionMessage> {
    Some(SessionMessage {
        role: "system".to_string(),
        text: "*Context compacted*".to_string(),
        timestamp,
        kind: kind::COMPACTION.to_string(),
        tool_use_id: None,
        exit_code: None,
    })
}

/// `EventMsg::Error` — Codex renders error notices via the chat widget.
/// `ErrorEvent.message` is human-readable; we surface it inline as a
/// `**Error**: {message}` system notice (bold marker + plain message
/// so the transcript shows what went wrong without losing the body).
fn codex_error_event(payload: &Value, timestamp: Option<String>) -> Option<SessionMessage> {
    let message = payload.get("message").and_then(Value::as_str)?;
    let trimmed = message.trim();
    if trimmed.is_empty() {
        return None;
    }
    Some(SessionMessage {
        role: "system".to_string(),
        text: format!("**Error**: {trimmed}"),
        timestamp,
        kind: kind::TEXT.to_string(),
        tool_use_id: None,
        exit_code: None,
    })
}

/// `EventMsg::TurnAborted` — `reason` is one of `interrupted` / `budget_limited`.
/// Codex shows this as a small dim notice in the transcript when a turn
/// stops mid-flight.
fn codex_turn_aborted_event(payload: &Value, timestamp: Option<String>) -> Option<SessionMessage> {
    let reason = payload.get("reason").and_then(Value::as_str).unwrap_or("");
    let text = match reason {
        "interrupted" => "*Turn interrupted by user*".to_string(),
        "budget_limited" => "*Turn stopped — budget limit reached*".to_string(),
        other if !other.is_empty() => format!("*Turn aborted — {other}*"),
        _ => "*Turn aborted*".to_string(),
    };
    Some(SessionMessage {
        role: "system".to_string(),
        text,
        timestamp,
        kind: kind::TEXT.to_string(),
        tool_use_id: None,
        exit_code: None,
    })
}

/// `EventMsg::ThreadRolledBack` — Codex shows this when the conversation
/// has been rolled back N user turns.
fn codex_thread_rolled_back_event(
    payload: &Value,
    timestamp: Option<String>,
) -> Option<SessionMessage> {
    let dropped = payload
        .get("dropped_turn_count")
        .or_else(|| payload.get("count"))
        .and_then(Value::as_i64);
    let text = match dropped {
        Some(1) => "*Rolled back 1 turn*".to_string(),
        Some(n) if n > 0 => format!("*Rolled back {n} turns*"),
        _ => "*Conversation rolled back*".to_string(),
    };
    Some(SessionMessage {
        role: "system".to_string(),
        text,
        timestamp,
        kind: kind::TEXT.to_string(),
        tool_use_id: None,
        exit_code: None,
    })
}

/// `EventMsg::PatchApplyEnd` ({call_id, changes, success, stdout,
/// stderr}). Renders each FileChange entry as a `**{Verb}** {path}`
/// line and surfaces stderr inline when `success: false` so failed
/// patches show why they failed.
fn codex_patch_apply_end_event(
    payload: &Value,
    timestamp: Option<String>,
) -> Option<SessionMessage> {
    let changes = payload.get("changes").and_then(Value::as_object);
    let mut lines: Vec<String> = Vec::new();
    if let Some(changes) = changes {
        let mut entries: Vec<(&String, &Value)> = changes.iter().collect();
        entries.sort_by(|a, b| a.0.cmp(b.0));
        for (path, change) in entries {
            // `FileChange` is `#[serde(tag = "type")]`. The type discriminant
            // is `add` / `delete` / `update` (snake_case enum).
            let kind = change.get("type").and_then(Value::as_str).unwrap_or("");
            let verb = match kind {
                "add" => "Added",
                "delete" => "Deleted",
                "update" => "Edited",
                _ => "Edited",
            };
            let display_path = match (kind, change.get("move_path").and_then(Value::as_str)) {
                ("update", Some(target)) if !target.is_empty() => {
                    format!("{path} → {target}")
                }
                _ => path.clone(),
            };
            lines.push(format!("**{verb}** {display_path}"));
        }
    }
    let success = payload
        .get("success")
        .and_then(Value::as_bool)
        .unwrap_or(true);
    let mut text = if lines.is_empty() {
        "**Applied patch**".to_string()
    } else {
        lines.join("\n")
    };
    if !success {
        let err = payload
            .get("stderr")
            .and_then(Value::as_str)
            .map(str::trim)
            .filter(|s| !s.is_empty());
        text.push_str("\n\n");
        if let Some(e) = err {
            text.push_str("````\n");
            text.push_str(e);
            text.push_str("\n````\n\n");
        }
        text.push_str("**Error**");
    }
    Some(SessionMessage {
        role: "tool".to_string(),
        text,
        timestamp,
        kind: if success {
            kind::TOOL_USE.to_string()
        } else {
            kind::TOOL_ERROR.to_string()
        },
        tool_use_id: payload.get("call_id").and_then(value_to_string),
        exit_code: None,
    })
}

/// `EventMsg::ViewImageToolCall` — `{call_id, path}` payload. Same
/// surface as the `function_call::view_image` ResponseItem variant
/// (codex_function_call_message): `**Viewed image** \`{path}\``.
fn codex_view_image_tool_call_event(
    payload: &Value,
    timestamp: Option<String>,
) -> Option<SessionMessage> {
    let path = payload.get("path").and_then(Value::as_str)?;
    if path.is_empty() {
        return None;
    }
    Some(SessionMessage {
        role: "tool".to_string(),
        text: format!("**Viewed image**({})", wrap_inline_code(path)),
        timestamp,
        kind: kind::TOOL_USE.to_string(),
        tool_use_id: payload.get("call_id").and_then(value_to_string),
        exit_code: None,
    })
}

/// `EventMsg::PlanUpdate` — the plan EventMsg variant (carries
/// `UpdatePlanArgs` directly, not nested in `arguments`). Same surface
/// as the `update_plan` function_call but the payload IS the
/// UpdatePlanArgs.
fn codex_plan_update_event(payload: &Value, timestamp: Option<String>) -> Option<SessionMessage> {
    let args = serde_json::to_string(payload).ok()?;
    let text = codex_update_plan_message(Some(&args));
    Some(SessionMessage {
        role: "tool".to_string(),
        text,
        timestamp,
        kind: kind::PLAN.to_string(),
        tool_use_id: None,
        exit_code: None,
    })
}

/// `EventMsg::EnteredReviewMode` — start of an automated code review.
fn codex_entered_review_mode_event(
    _payload: &Value,
    timestamp: Option<String>,
) -> Option<SessionMessage> {
    Some(SessionMessage {
        role: "system".to_string(),
        text: "*Entered review mode*".to_string(),
        timestamp,
        kind: kind::TEXT.to_string(),
        tool_use_id: None,
        exit_code: None,
    })
}

/// `EventMsg::ExitedReviewMode` — end of an automated review. The
/// payload's `review_output` carries the structured findings; we leave
/// it to a future iteration to render the structured form and only
/// emit the lifecycle notice here.
fn codex_exited_review_mode_event(
    _payload: &Value,
    timestamp: Option<String>,
) -> Option<SessionMessage> {
    Some(SessionMessage {
        role: "system".to_string(),
        text: "*Exited review mode*".to_string(),
        timestamp,
        kind: kind::TEXT.to_string(),
        tool_use_id: None,
        exit_code: None,
    })
}

fn codex_user_message_event(payload: &Value, timestamp: Option<String>) -> Option<SessionMessage> {
    let message = payload.get("message").and_then(Value::as_str)?;
    if message.trim().is_empty() {
        return None;
    }
    // Codex hides specific contextual user fragments from the visible
    // transcript (environment_context / user_instructions / ...). Apply the
    // same filter we use for ResponseItem-derived user messages.
    if is_codex_contextual_user_text(message) {
        return None;
    }
    Some(SessionMessage {
        role: "user".to_string(),
        text: message.to_string(),
        timestamp,
        kind: kind::TEXT.to_string(),
        tool_use_id: None,
        exit_code: None,
    })
}

fn codex_agent_message_event(payload: &Value, timestamp: Option<String>) -> Option<SessionMessage> {
    let message = payload.get("message").and_then(Value::as_str)?;
    let visible = codex_visible_assistant_markdown(message);
    if visible.is_empty() {
        return None;
    }
    Some(SessionMessage {
        role: "assistant".to_string(),
        text: visible,
        timestamp,
        kind: kind::TEXT.to_string(),
        tool_use_id: None,
        exit_code: None,
    })
}

fn codex_agent_reasoning_event(
    payload: &Value,
    timestamp: Option<String>,
) -> Option<SessionMessage> {
    let text = payload.get("text").and_then(Value::as_str)?;
    let formatted = format_reasoning_body(text);
    if formatted.is_empty() {
        return None;
    }
    Some(SessionMessage {
        role: "assistant".to_string(),
        text: formatted,
        timestamp,
        kind: kind::REASONING.to_string(),
        tool_use_id: None,
        exit_code: None,
    })
}

/// Build a tool message from `EventMsg::ExecCommandEnd` (the canonical source
/// for shell-tool replay rendering in Codex). Codex's TUI feeds these to
/// `ExecCell` and renders:
///   `$ {command}` (bash-highlighted, magenta `$`)
///   {aggregated_output}     (dim, `└ ` / `    ` prefixed)
///   `✓` / `✗ ({exit})` • {duration}
/// We approximate that in markdown — the raw output is the actual command
/// output (no `Chunk ID:` / `Wall time:` metadata).
fn codex_exec_command_end_message(
    payload: &Value,
    timestamp: Option<String>,
) -> Option<SessionMessage> {
    let command = payload
        .get("command")
        .and_then(Value::as_array)
        .map(|args| {
            args.iter()
                .filter_map(Value::as_str)
                .collect::<Vec<_>>()
                .join(" ")
        })
        .filter(|s| !s.is_empty())?;
    let aggregated_output = payload
        .get("aggregated_output")
        .and_then(Value::as_str)
        .unwrap_or("");
    let exit_code = payload.get("exit_code").and_then(Value::as_i64);

    let mut text = String::new();
    text.push_str("```bash\n$ ");
    text.push_str(&command);
    text.push_str("\n```");

    let output_trimmed = aggregated_output.trim_end();
    if !output_trimmed.is_empty() {
        text.push_str("\n\n````\n");
        text.push_str(output_trimmed);
        text.push_str("\n````");
    }

    if let Some(code) = exit_code {
        text.push_str("\n\n");
        if code == 0 {
            text.push('✓');
        } else {
            text.push_str(&format!("✗ exit {code}"));
        }
    }

    Some(SessionMessage {
        role: "tool".to_string(),
        text,
        timestamp,
        kind: kind::COMMAND_EXECUTION.to_string(),
        tool_use_id: payload.get("call_id").and_then(value_to_string),
        exit_code: None,
    })
}

fn codex_command_from_arguments(arguments: Option<&str>) -> Option<String> {
    let arguments = arguments?.trim();
    if arguments.is_empty() {
        return None;
    }
    let value = serde_json::from_str::<Value>(arguments).ok()?;
    value
        .get("cmd")
        .and_then(value_to_string)
        .or_else(|| value.get("command").and_then(value_to_string))
        .or_else(|| {
            value
                .get("argv")
                .and_then(Value::as_array)
                .map(|items| {
                    items
                        .iter()
                        .filter_map(value_to_string)
                        .collect::<Vec<_>>()
                        .join(" ")
                })
                .filter(|command| !command.trim().is_empty())
        })
}

fn codex_fallback_event_message(
    payload_type: &str,
    payload: &Value,
    timestamp: Option<String>,
) -> Option<SessionMessage> {
    let text = match payload_type {
        "hook_prompt" => {
            let fragments = payload.get("fragments").and_then(Value::as_array);
            if let Some(fragments) = fragments {
                let lines = fragments
                    .iter()
                    .filter_map(|fragment| fragment.get("text").and_then(value_to_string))
                    .map(|text| format!("hook prompt: {}", text.trim()))
                    .collect::<Vec<_>>();
                (!lines.is_empty()).then(|| lines.join("\n"))
            } else {
                payload
                    .get("text")
                    .and_then(value_to_string)
                    .map(|text| format!("hook prompt: {}", text.trim()))
            }
        }
        "file_change" => {
            let status = payload
                .get("status")
                .and_then(value_to_string)
                .unwrap_or_else(|| "unknown".to_string());
            let change_count = payload
                .get("changes")
                .and_then(Value::as_array)
                .map(|changes| changes.len())
                .unwrap_or(0);
            Some(format!("file changes: {status} · {change_count} changes"))
        }
        "mcp_tool_call" => {
            let server = payload
                .get("server")
                .and_then(value_to_string)
                .unwrap_or_default();
            let tool = payload
                .get("tool")
                .and_then(value_to_string)
                .or_else(|| payload.get("name").and_then(value_to_string))
                .unwrap_or_default();
            let status = payload
                .get("status")
                .and_then(value_to_string)
                .unwrap_or_else(|| "unknown".to_string());
            Some(format!("mcp tool: {server}/{tool} · {status}"))
        }
        "dynamic_tool_call" => {
            let tool = payload
                .get("tool")
                .and_then(value_to_string)
                .or_else(|| payload.get("name").and_then(value_to_string))
                .unwrap_or_else(|| "tool".to_string());
            let namespace = payload.get("namespace").and_then(value_to_string);
            let status = payload
                .get("status")
                .and_then(value_to_string)
                .unwrap_or_else(|| "unknown".to_string());
            let label = namespace
                .map(|namespace| format!("{namespace}/{tool}"))
                .unwrap_or(tool);
            Some(format!("tool: {label} · {status}"))
        }
        "collab_agent_tool_call" => {
            let tool = payload
                .get("tool")
                .and_then(value_to_string)
                .unwrap_or_else(|| "tool".to_string());
            let status = payload
                .get("status")
                .and_then(value_to_string)
                .unwrap_or_else(|| "unknown".to_string());
            Some(format!("agent tool: {tool} · {status}"))
        }
        "web_search" => payload
            .get("query")
            .and_then(value_to_string)
            .map(|query| format!("web search: {query}")),
        "image_view" => payload
            .get("path")
            .and_then(value_to_string)
            .map(|path| format!("image: {path}")),
        "image_generation" => {
            let status = payload
                .get("status")
                .and_then(value_to_string)
                .unwrap_or_else(|| "unknown".to_string());
            let mut text = format!("image generation: {status}");
            if let Some(path) = payload.get("saved_path").and_then(value_to_string) {
                text.push_str(" · ");
                text.push_str(&path);
            }
            Some(text)
        }
        "context_compaction" => Some("context compacted".to_string()),
        "entered_review_mode" => payload
            .get("review")
            .and_then(value_to_string)
            .map(|review| format!("review started: {review}")),
        "exited_review_mode" => payload
            .get("review")
            .and_then(value_to_string)
            .map(|review| format!("review finished: {review}")),
        _ => None,
    }?;
    Some(SessionMessage {
        role: "tool".to_string(),
        text,
        timestamp,
        kind: payload_type.to_string(),
        tool_use_id: None,
        exit_code: None,
    })
}

fn value_to_string(value: &Value) -> Option<String> {
    match value {
        Value::String(s) if !s.trim().is_empty() => Some(s.to_string()),
        Value::Number(n) => Some(n.to_string()),
        _ => None,
    }
}

fn official_title_from_text(text: &str) -> Option<String> {
    let cleaned = text
        .lines()
        .map(str::trim)
        .find(|line| !line.is_empty())?
        .trim_matches('"')
        .trim();
    if cleaned.len() < 2 {
        None
    } else {
        Some(cleaned.to_string())
    }
}

fn codex_display_title(text: &str) -> Option<String> {
    let mut lines = text
        .lines()
        .map(str::trim)
        .filter(|line| !line.is_empty())
        .filter(|line| !line.starts_with('{') && !line.starts_with('['))
        .filter(|line| !line.starts_with("cwd=") && !line.starts_with("session"))
        .filter(|line| !looks_like_codex_metadata(line));

    let first = lines.next()?;
    let cleaned = first.trim_matches('"').trim();
    if cleaned.len() < 2 {
        return None;
    }
    Some(compact(cleaned, 1_000))
}

fn looks_like_codex_metadata(text: &str) -> bool {
    text.starts_with("msg_")
        || text.starts_with("sess_")
        || text.len() > 40 && text.chars().all(|c| c.is_ascii_hexdigit() || c == '-')
}

fn estimate_codex_message_count(path: &Path) -> usize {
    let Ok(content) = fs::read_to_string(path) else {
        return 0;
    };
    content
        .lines()
        .filter(|line| {
            let Ok(value) = serde_json::from_str::<Value>(line) else {
                return false;
            };
            codex_message_from_value(&value).is_some()
        })
        .count()
}

// `compact` collapses whitespace + truncates with an ellipsis. Used only
// for session list title previews (where the platforms themselves
// truncate at display time); message bodies are not truncated anywhere
// in the unified format pipeline.
fn compact(value: &str, max: usize) -> String {
    let normalized = value.split_whitespace().collect::<Vec<_>>().join(" ");
    if normalized.chars().count() <= max {
        normalized
    } else {
        format!("{}...", normalized.chars().take(max).collect::<String>())
    }
}

fn extract_json_string_field(text: &str, key: &str) -> Option<String> {
    extract_json_string_field_from(text, key, false)
}

fn extract_last_json_string_field(text: &str, key: &str) -> Option<String> {
    extract_json_string_field_from(text, key, true)
}

fn extract_json_string_field_from(text: &str, key: &str, last: bool) -> Option<String> {
    let patterns = [format!("\"{key}\":\""), format!("\"{key}\": \"")];
    let mut result = None;
    for pattern in patterns {
        let mut search_from = 0;
        while let Some(index) = text[search_from..].find(&pattern) {
            let value_start = search_from + index + pattern.len();
            let mut cursor = value_start;
            while cursor < text.len() {
                let byte = text.as_bytes()[cursor];
                if byte == b'\\' {
                    cursor += 2;
                    continue;
                }
                if byte == b'"' {
                    let raw = &text[value_start..cursor];
                    let value = serde_json::from_str::<String>(&format!("\"{raw}\"")).ok();
                    if !last {
                        return value;
                    }
                    result = value;
                    break;
                }
                cursor += 1;
            }
            search_from = cursor.saturating_add(1);
        }
    }
    result
}

fn normalize_time(value: String) -> Option<String> {
    if value.trim().is_empty() {
        return None;
    }
    if let Ok(ts) = value.parse::<i64>() {
        let millis = if ts > 10_000_000_000 { ts } else { ts * 1000 };
        return Local
            .timestamp_millis_opt(millis)
            .single()
            .map(|dt| dt.to_rfc3339());
    }
    DateTime::parse_from_rfc3339(&value)
        .map(|dt| dt.with_timezone(&Utc).to_rfc3339())
        .ok()
        .or(Some(value))
}

fn first_existing_table(conn: &Connection, names: &[&str]) -> Result<String, Box<dyn Error>> {
    for name in names {
        let exists: i64 = conn.query_row(
            "select count(*) from sqlite_master where type='table' and name=?1",
            [name],
            |row| row.get(0),
        )?;
        if exists > 0 {
            return Ok((*name).to_string());
        }
    }
    Err(format!("none of these tables exist: {}", names.join(", ")).into())
}

fn table_exists(conn: &Connection, name: &str) -> Result<bool, Box<dyn Error>> {
    let exists: i64 = conn.query_row(
        "select count(*) from sqlite_master where type='table' and name=?1",
        [name],
        |row| row.get(0),
    )?;
    Ok(exists > 0)
}

fn column_exists(conn: &Connection, table: &str, column: &str) -> Result<bool, Box<dyn Error>> {
    let mut stmt = conn.prepare(&format!("pragma table_info({table})"))?;
    let mut rows = stmt.query([])?;
    while let Some(row) = rows.next()? {
        let name: String = row.get(1)?;
        if name == column {
            return Ok(true);
        }
    }
    Ok(false)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::PathBuf;
    use std::time::{SystemTime, UNIX_EPOCH};

    struct TestDir(PathBuf);

    impl TestDir {
        fn new(name: &str) -> Self {
            let nanos = SystemTime::now()
                .duration_since(UNIX_EPOCH)
                .unwrap()
                .as_nanos();
            let path = std::env::temp_dir().join(format!("termory-{name}-{nanos}"));
            fs::create_dir_all(&path).unwrap();
            Self(path)
        }

        fn path(&self) -> &Path {
            &self.0
        }
    }

    impl Drop for TestDir {
        fn drop(&mut self) {
            let _ = fs::remove_dir_all(&self.0);
        }
    }

    #[test]
    fn codex_scans_threads_from_state_db_only() {
        let dir = TestDir::new("codex");
        let rollout = dir.path().join("rollout.jsonl");
        // User and assistant text both come from EventMsg variants in
        // Codex Limited-mode rollouts (policy.rs:137-153 — UserMessage /
        // AgentMessage are persisted in Limited mode). The duplicate
        // ResponseItem::Message records are present but ignored.
        fs::write(
            &rollout,
            r#"{"type":"session_meta","payload":{"id":"thread-1","cwd":"/workspace/project"}}"# .to_string()
                + "\n"
                + r#"{"type":"event_msg","timestamp":"2026-05-01T00:00:00Z","payload":{"type":"user_message","message":"Review backend changes"}}"#
                + "\n"
                + r#"{"type":"response_item","timestamp":"2026-05-01T00:00:00Z","payload":{"type":"message","role":"user","content":[{"type":"input_text","text":"Review backend changes"}]}}"#
                + "\n"
                + r#"{"type":"event_msg","timestamp":"2026-05-01T00:00:00Z","payload":{"type":"agent_message","message":"Done"}}"#
                + "\n"
                + r#"{"type":"response_item","timestamp":"2026-05-01T00:00:00Z","payload":{"type":"message","role":"assistant","content":[{"text":"Done"}]}}"#,
        )
        .unwrap();

        let db = dir.path().join("state_5.sqlite");
        let conn = Connection::open(&db).unwrap();
        // Schema mirrors codex-rs/state/migrations: id PK, rollout_path,
        // timestamps, source, cwd, title, first_user_message, preview,
        // archived. `preview` was added in migration 0032 and is part of the
        // official non-archived filter (push_thread_filters requires
        // `preview <> ''`).
        conn.execute_batch(
            "create table threads (
                id text,
                rollout_path text,
                created_at integer,
                updated_at integer,
                source text,
                cwd text,
                title text,
                first_user_message text,
                preview text,
                archived integer
            );",
        )
        .unwrap();
        conn.execute(
            "insert into threads values (?1, ?2, 1714521600000, 1714521700000, 'cli', ?3, ?4, ?5, 'preview', 0)",
            (
                "thread-1",
                rollout.display().to_string(),
                "/workspace/project",
                "Review backend changes",
                "fallback message",
            ),
        )
        .unwrap();
        // Same shape, source='atlas' — must also be picked up because Codex's
        // INTERACTIVE_SESSION_SOURCES includes 'atlas'.
        let rollout_atlas = dir.path().join("rollout-atlas.jsonl");
        fs::write(
            &rollout_atlas,
            r#"{"type":"session_meta","payload":{"id":"thread-atlas","cwd":"/workspace/project"}}"#,
        )
        .unwrap();
        conn.execute(
            "insert into threads values (?1, ?2, 1714521800000, 1714521900000, 'atlas', '/workspace/project', 'Atlas thread', 'first', 'preview', 0)",
            (
                "thread-atlas",
                rollout_atlas.display().to_string(),
            ),
        )
        .unwrap();
        // Same shape, but preview is empty — must be filtered out per
        // push_thread_filters' `preview <> ''` clause.
        conn.execute(
            "insert into threads values ('thread-no-preview', '/nonexistent', 1714521600000, 1714521700000, 'cli', '/workspace/project', 'No preview', 'first', '', 0)",
            (),
        )
        .unwrap();

        let sessions = scan_codex_state_db(&db).unwrap();
        // thread-1 (cli + non-empty preview) and thread-atlas (atlas) appear;
        // thread-no-preview is filtered out by `preview <> ''`.
        let ids: Vec<&str> = sessions.iter().map(|s| s.id.as_str()).collect();
        assert!(ids.contains(&"thread-1"));
        assert!(
            ids.contains(&"thread-atlas"),
            "atlas-source threads must be surfaced (matches Codex INTERACTIVE_SESSION_SOURCES)"
        );
        assert!(
            !ids.contains(&"thread-no-preview"),
            "threads with empty preview must be filtered (matches push_thread_filters preview <> '')"
        );
        let main = sessions.iter().find(|s| s.id == "thread-1").unwrap();
        assert_eq!(main.title, "Review backend changes");
        assert_eq!(main.project, "/workspace/project");
        assert_eq!(main.message_count, 2);
    }

    #[test]
    fn codex_scans_legacy_state_db_without_preview_column() {
        // Older state_5.sqlite files predate migration 0032_threads_preview.sql.
        // Termory should still list their threads — it must detect the missing
        // column and skip the `preview <> ''` clause instead of returning a
        // SQLite "no such column" error to the user.
        let dir = TestDir::new("codex-legacy");
        let rollout = dir.path().join("rollout-legacy.jsonl");
        fs::write(
            &rollout,
            r#"{"type":"session_meta","payload":{"id":"thread-legacy","cwd":"/workspace/legacy"}}"#,
        )
        .unwrap();

        let db = dir.path().join("state_5.sqlite");
        let conn = Connection::open(&db).unwrap();
        // Schema WITHOUT the preview column.
        conn.execute_batch(
            "create table threads (
                id text,
                rollout_path text,
                created_at integer,
                updated_at integer,
                source text,
                cwd text,
                title text,
                first_user_message text,
                archived integer
            );",
        )
        .unwrap();
        conn.execute(
            "insert into threads values ('thread-legacy', ?1, 1714521600000, 1714521700000, 'cli', '/workspace/legacy', 'Legacy thread', 'first', 0)",
            [rollout.display().to_string()],
        )
        .unwrap();

        let sessions = scan_codex_state_db(&db).unwrap();
        assert_eq!(sessions.len(), 1);
        assert_eq!(sessions[0].id, "thread-legacy");
    }

    #[test]
    fn codex_keeps_absolute_path_titles_from_state() {
        assert_eq!(
            codex_display_title(
                "/Applications/QClaw.app 请帮我逆向分析这个应用，移除InviteCodeModel前端邀请码验证"
            )
            .as_deref(),
            Some(
                "/Applications/QClaw.app 请帮我逆向分析这个应用，移除InviteCodeModel前端邀请码验证"
            )
        );
    }

    #[test]
    fn codex_parses_visible_thread_messages_only() {
        let dir = TestDir::new("codex-messages");
        let path = dir.path().join("rollout.jsonl");
        // Codex CLI persists rollout in Limited mode by default
        // (codex-rs/tui/src/app_server_session.rs: `persist_extended_history:
        // false`). In Limited mode the `EventMsg::ExecCommandEnd` event is
        // NOT persisted, so shell command + output come only from
        // `ResponseItem::FunctionCall` + `FunctionCallOutput`. Other
        // content (user / assistant / reasoning) comes from `EventMsg`
        // variants in BOTH modes — we prefer those and skip the
        // duplicate ResponseItem::Message / Reasoning entries.
        //
        // The `FunctionCallOutput.output` field carries Codex's
        // `ExecCommandToolOutput.response_text()` formatted string
        // (Chunk ID / Wall time / Output: / ...). Termory strips that
        // wrapper via `codex_extract_raw_command_output` so users see the
        // same raw command output Codex's TUI does.
        fs::write(
            &path,
            r#"{"type":"session_meta","payload":{"id":"thread-2","cwd":"/workspace/project"}}"#.to_string()
                + "\n"
                + r#"{"type":"event_msg","timestamp":"2026-05-01T00:00:00Z","payload":{"type":"user_message","message":"Fix the app"}}"#
                + "\n"
                + r#"{"type":"response_item","timestamp":"2026-05-01T00:00:00Z","payload":{"type":"message","role":"user","content":[{"type":"input_text","text":"Fix the app"}]}}"#
                + "\n"
                + r#"{"type":"response_item","timestamp":"2026-05-01T00:00:01Z","payload":{"type":"function_call","call_id":"call_1","name":"shell","arguments":"{\"cmd\":\"ls\"}"}}"#
                + "\n"
                + r#"{"type":"response_item","timestamp":"2026-05-01T00:00:02Z","payload":{"type":"function_call_output","call_id":"call_1","output":"Chunk ID: 209810\nWall time: 0.0000 seconds\nProcess exited with code 0\nOriginal token count: 1\nOutput:\nsrc"}}"#
                + "\n"
                + r#"{"type":"event_msg","timestamp":"2026-05-01T00:00:03Z","payload":{"type":"agent_message","message":"Done"}}"#
                + "\n"
                + r#"{"type":"response_item","timestamp":"2026-05-01T00:00:03Z","payload":{"type":"message","role":"assistant","content":[{"type":"output_text","text":"Done"}]}}"#,
        )
        .unwrap();

        let detail = parse_codex_session(&path, "thread-2").unwrap();
        assert_eq!(detail.session.title, "");
        // Visible flow: user (from event_msg) → tool (merged from
        // FunctionCall + FunctionCallOutput) → assistant (from
        // event_msg). The duplicate ResponseItem::Message records are
        // skipped.
        assert_eq!(detail.messages.len(), 3);
        assert_eq!(detail.messages[0].role, "user");
        assert_eq!(detail.messages[0].text, "Fix the app");
        assert_eq!(detail.messages[1].role, "tool");
        // Unified tool card: `**Ran** \`<cmd>\`` header (Codex's "Ran" verb
        // from exec_cell/render.rs:381-385) + raw output extracted from the
        // `Chunk ID:...Output:\nsrc` formatted wrapper (so the metadata
        // noise is gone, matching Codex TUI which uses aggregated_output).
        assert_eq!(
            detail.messages[1].text,
            "⏺ **Bash**(`ls`)\n\n````\nsrc\n````"
        );
        assert_eq!(detail.messages[2].role, "assistant");
        assert_eq!(detail.messages[2].text, "Done");
    }

    #[test]
    fn codex_view_image_function_call_uses_inline_path() {
        // function_call name="view_image" → `**Viewed image** \`{path}\``,
        // matching Codex `new_view_image_tool_call` (patches.rs:63-72)
        // which renders `• Viewed Image\n  └ {display_path}`.
        let body = codex_function_call_message(
            &serde_json::json!({
                "type": "function_call",
                "name": "view_image",
                "arguments": "{\"path\":\"/tmp/shot.png\"}",
                "call_id": "img_1"
            }),
            None,
        )
        .unwrap();
        assert_eq!(body.text, "**Viewed image**(`/tmp/shot.png`)");
        assert_eq!(body.kind, kind::TOOL_USE);
    }

    #[test]
    fn codex_update_plan_emits_gfm_task_list() {
        // Arguments shape matches Codex's `UpdatePlanArgs`
        // (plan_tool::PlanItemArg with snake_case status).
        let body = codex_update_plan_message(Some(
            r#"{"explanation":"Sequenced work","plan":[
                {"step":"Read repo layout","status":"completed"},
                {"step":"Fix tests","status":"in_progress"},
                {"step":"Ship release","status":"pending"}
            ]}"#,
        ));
        assert_eq!(
            body,
            "**Updated plan**\n\n*Sequenced work*\n\n- [x] Read repo layout\n- [~] Fix tests\n- [ ] Ship release"
        );

        // No explanation, no steps — bare header.
        assert_eq!(codex_update_plan_message(Some("{}")), "**Updated plan**");
    }

    #[test]
    fn gemini_thoughts_array_emits_reasoning_messages() {
        // Real Gemini JSONL format: `thoughts: [{subject, description, timestamp}, ...]`
        // sibling to `content`. Gemini TUI renders each via
        // `ThinkingMessage`. Termory surfaces them as separate reasoning
        // messages (italic-blockquote) — same convention as Claude /
        // Codex / OpenCode.
        let messages = gemini_thought_messages_from_value(&serde_json::json!({
            "type": "gemini",
            "timestamp": "2026-04-30T06:49:18Z",
            "content": "",
            "thoughts": [
                {
                    "subject": "Examining Workspace State",
                    "description": "Reviewing git status.\n\nWill check diff next.",
                    "timestamp": "2026-04-30T06:49:14Z"
                },
                {
                    "subject": "...",
                    "description": "noise only"
                }
            ]
        }));
        assert_eq!(messages.len(), 2);
        assert_eq!(messages[0].kind, kind::REASONING);
        assert_eq!(
            messages[0].text,
            "> *Examining Workspace State*\n> *Reviewing git status.*\n> *Will check diff next.*"
        );
        // Second thought has noise-only subject (`...`) but a real
        // description — subject is dropped, description rendered.
        assert_eq!(messages[1].text, "> *noise only*");
    }

    #[test]
    fn codex_lifecycle_event_messages_render_inline_notices() {
        let make = |payload: serde_json::Value| codex_event_msg_to_message(&payload, None).unwrap();

        // Error event — surface the human-readable message.
        let msg = make(serde_json::json!({
            "type": "error",
            "message": "Network timeout"
        }));
        assert_eq!(msg.role, "system");
        assert_eq!(msg.text, "**Error**: Network timeout");

        // TurnAborted with reason=interrupted.
        let msg = make(serde_json::json!({
            "type": "turn_aborted",
            "turn_id": "t1",
            "reason": "interrupted"
        }));
        assert_eq!(msg.text, "*Turn interrupted by user*");

        // TurnAborted with budget reason.
        let msg = make(serde_json::json!({
            "type": "turn_aborted",
            "reason": "budget_limited"
        }));
        assert_eq!(msg.text, "*Turn stopped — budget limit reached*");

        // ThreadRolledBack with a count.
        let msg = make(serde_json::json!({
            "type": "thread_rolled_back",
            "dropped_turn_count": 3
        }));
        assert_eq!(msg.text, "*Rolled back 3 turns*");

        let msg = make(serde_json::json!({
            "type": "thread_rolled_back",
            "dropped_turn_count": 1
        }));
        assert_eq!(msg.text, "*Rolled back 1 turn*");

        // Review mode lifecycle.
        let msg = make(serde_json::json!({"type": "entered_review_mode"}));
        assert_eq!(msg.text, "*Entered review mode*");

        let msg = make(serde_json::json!({"type": "exited_review_mode"}));
        assert_eq!(msg.text, "*Exited review mode*");
    }

    #[test]
    fn claude_tool_use_with_error_result_shows_message_above_cross() {
        // Real-world Claude error pattern (e.g., InputValidationError
        // wrapped in `<tool_use_error>...`). The unified format flips the
        // status marker to `✗` at the front and adds an `Error:` prefix to
        // the fence body. Claude has no exit_code so the prefix is just
        // `Error:` without a number.
        let messages = parse_claude_messages_for_test(
            r#"{"type":"assistant","message":{"role":"assistant","content":[{"type":"tool_use","id":"tu_1","name":"Bash","input":{"command":"ls"}}]},"timestamp":"2026-05-01T00:00:00Z"}"#,
            r#"{"type":"user","message":{"role":"user","content":[{"type":"tool_result","tool_use_id":"tu_1","is_error":true,"content":"<tool_use_error>InputValidationError: Write failed due to missing parameter file_path</tool_use_error>"}]},"timestamp":"2026-05-01T00:00:01Z"}"#,
        );
        let tool = messages
            .iter()
            .find(|m| m.role == "tool")
            .expect("tool card present");
        assert!(
            tool.text.starts_with("✗ **Bash**"),
            "failed tool card should start with ✗ marker; got: {}",
            tool.text
        );
        assert!(
            tool.text.contains("Error: InputValidationError"),
            "fence body should be `Error: ` + the message inline; got: {}",
            tool.text
        );
    }

    #[test]
    #[ignore = "manual diagnostic — dump Codex apply_patch fence text"]
    fn codex_real_session_dump_diff_fence() {
        let path = std::path::Path::new(
            "/Users/john/.codex/sessions/2026/04/28/rollout-2026-04-28T12-00-14-019dd23e-a50a-7c33-8df5-09eb6ae0f7e8.jsonl",
        );
        if !path.exists() {
            return;
        }
        let detail = parse_codex_session(path, "019dd23e-a50a-7c33-8df5-09eb6ae0f7e8").unwrap();
        for (i, m) in detail.messages.iter().enumerate() {
            if m.text.contains("```diff") {
                println!("--- msg #{i} kind={} ---\n{}\n", m.kind, m.text);
                if i > 30 {
                    break;
                }
            }
        }
    }

    #[test]
    #[ignore = "manual diagnostic — reads the user's real session file"]
    fn claude_real_session_dump_tool_messages() {
        let path = std::path::Path::new(
            "/Users/john/.claude/projects/-Users-john-Documents-termory/0d6abfa5-8939-4bd8-8437-30fbb3bc6f09.jsonl",
        );
        if !path.exists() {
            return;
        }
        let detail = parse_claude_session(path).unwrap();
        for (i, m) in detail.messages.iter().enumerate() {
            if m.text
                .contains("**Update**(`/Users/john/Documents/termory/src/main.tsx`)")
            {
                println!("--- msg #{i} kind={} ---", m.kind);
                println!("{}", &m.text[..m.text.len().min(400)]);
                println!();
            }
        }
    }

    fn parse_claude_messages_for_test(line1: &str, line2: &str) -> Vec<SessionMessage> {
        let mut out: Vec<SessionMessage> = Vec::new();
        for line in [line1, line2] {
            let v: Value = serde_json::from_str(line).unwrap();
            out.extend(claude_message_from_value(&v));
        }
        merge_tool_outputs(out)
    }

    #[test]
    fn claude_task_notification_collapses_to_summary_line() {
        // Background task notifications wrap a summary + extra metadata
        // (task-id / event / output-file). Claude Code's UI renders ONLY
        // the summary via `UserAgentNotificationMessage.tsx`. Termory
        // mirrors: `⏺ {summary}` and drops the rest.
        let formatted = claude_display_text(
            "<task-notification>\n<task-id>bd21wlkuh</task-id>\n<summary>Monitor event: \"Tauri dev startup progress\"</summary>\n<event>VITE v6.4.2 ready in 179 ms</event>\n</task-notification>",
        );
        assert_eq!(
            formatted.as_deref(),
            Some("⏺ Monitor event: \"Tauri dev startup progress\"")
        );
    }

    #[test]
    fn claude_image_content_block_emits_italic_notice() {
        // base64 source — surface mime type so the user knows an image
        // was attached (we can't render the actual bytes inline).
        let text = claude_text_blocks(&serde_json::json!([
            {"type":"text","text":"Look:"},
            {"type":"image","source":{"type":"base64","media_type":"image/png","data":"AAAA"}},
            {"type":"text","text":"end"}
        ]))
        .join("\n");
        assert_eq!(text, "Look:\n*Image (image/png)*\nend");

        // url source.
        let text = claude_text_blocks(&serde_json::json!([
            {"type":"image","source":{"type":"url","url":"https://x.example/a.png"}}
        ]))
        .join("\n");
        assert_eq!(text, "*Image: https://x.example/a.png*");
    }

    #[test]
    fn claude_extra_tool_names_render_aligned_verbs() {
        // NotebookEdit (NotebookEditTool.ts userFacingName="Edit Notebook").
        assert_eq!(
            claude_tool_use_text(
                "NotebookEdit",
                &serde_json::json!({"notebook_path": "notes/a.ipynb"})
            ),
            Some("**Edit Notebook**(`notes/a.ipynb`)".to_string())
        );

        // ExitPlanMode / EnterPlanMode / AskUserQuestion / TodoWrite /
        // Task* / ToolSearch — Claude TUI suppresses these via
        // `userFacingName: ''` + `renderToolUseMessage: null`. Termory
        // returns None so no card is emitted.
        assert_eq!(
            claude_tool_use_text("ExitPlanMode", &serde_json::json!({"plan": "..."})),
            None
        );
        assert_eq!(
            claude_tool_use_text(
                "AskUserQuestion",
                &serde_json::json!({"questions": [{"question": "Which path?"}]})
            ),
            None
        );
        assert_eq!(
            claude_tool_use_text("TodoWrite", &serde_json::json!({"todos": []})),
            None
        );
        assert_eq!(
            claude_tool_use_text("TaskCreate", &serde_json::json!({})),
            None
        );
        assert_eq!(
            claude_tool_use_text("ToolSearch", &serde_json::json!({})),
            None
        );

        // ReadMcpResource — verb is literal `readMcpResource` (camelCase
        // from Claude TUI userFacingName).
        assert_eq!(
            claude_tool_use_text(
                "ReadMcpResource",
                &serde_json::json!({"uri": "mcp://figma/file/x"})
            ),
            Some("**readMcpResource**(`mcp://figma/file/x`)".to_string())
        );

        // Generic MCP tool name `mcp__{server}__{tool}` → `**MCP**(s/t)`.
        assert_eq!(
            claude_tool_use_text("mcp__figma__inspect", &serde_json::json!({})),
            Some("**MCP**(figma/inspect)".to_string())
        );
    }

    #[test]
    fn claude_extended_thinking_emits_reasoning_message() {
        // Assistant content with a `thinking` block followed by `text`
        // and `tool_use`. Claude Code TUI renders thinking via
        // AssistantThinkingMessage (`∴ Thinking…` + dim markdown).
        // Termory surfaces it as a separate reasoning message using the
        // shared `> *content*` italic-blockquote format.
        let messages = claude_message_from_value(&serde_json::json!({
            "type": "assistant",
            "message": {
                "role": "assistant",
                "content": [
                    {"type": "thinking", "thinking": "Let me check the tests"},
                    {"type": "text", "text": "Done."},
                    {"type": "tool_use", "id": "tu_1", "name": "Bash", "input": {"command":"ls"}}
                ]
            },
            "timestamp": "2026-05-01T00:00:00Z"
        }));
        assert_eq!(messages.len(), 3);
        assert_eq!(messages[0].kind, kind::REASONING);
        assert_eq!(messages[0].text, "> *Let me check the tests*");
        assert_eq!(messages[1].kind, kind::TEXT);
        assert_eq!(messages[1].text, "Done.");
        assert_eq!(messages[2].kind, kind::TOOL_USE);
        assert!(messages[2].text.starts_with("**Bash**"));

        // Redacted thinking → placeholder text (not silently dropped).
        let messages = claude_message_from_value(&serde_json::json!({
            "type": "assistant",
            "message": {
                "role": "assistant",
                "content": [{"type": "redacted_thinking", "data": "encrypted-blob"}]
            },
            "timestamp": "2026-05-01T00:00:00Z"
        }));
        assert_eq!(messages.len(), 1);
        assert_eq!(messages[0].kind, kind::REASONING);
        assert_eq!(messages[0].text, "> *[redacted thinking]*");
    }

    #[test]
    fn codex_event_msg_renders_web_search_mcp_image_compaction() {
        // EventMsg::WebSearchEnd → `**Searched** \`{query}\`` (call_id
        // surfaces as tool_use_id so the merge can match a potential
        // future tool_result, though web search doesn't have one).
        let msg = codex_event_msg_to_message(
            &serde_json::json!({
                "type": "web_search_end",
                "call_id": "ws_1",
                "query": "react markdown rendering",
                "action": {"type": "search"}
            }),
            None,
        )
        .unwrap();
        assert_eq!(msg.role, "tool");
        assert_eq!(msg.text, "**Searched**(`react markdown rendering`)");
        assert_eq!(msg.tool_use_id.as_deref(), Some("ws_1"));

        // EventMsg::McpToolCallEnd → header `**MCP** {server}/{tool}`
        // and body from CallToolResult.content[].text fenced in
        // 4-backticks. Successful result keeps kind=tool_result.
        let msg = codex_event_msg_to_message(
            &serde_json::json!({
                "type": "mcp_tool_call_end",
                "call_id": "mcp_1",
                "invocation": {
                    "server": "figma",
                    "tool": "inspect",
                    "arguments": {}
                },
                "duration": {"secs":0,"nanos":0},
                "result": {
                    "Ok": {
                        "content": [{"type":"text","text":"frame: 12px"}],
                        "is_error": false
                    }
                }
            }),
            None,
        )
        .unwrap();
        assert_eq!(
            msg.text,
            "**MCP**(figma/inspect)\n\n````\nframe: 12px\n````"
        );
        assert_eq!(msg.kind, kind::TOOL_RESULT);

        // McpToolCallEnd with Err(_) → kind=tool_error so merge_tool_outputs
        // can fold the `**Error**` footer in.
        let msg = codex_event_msg_to_message(
            &serde_json::json!({
                "type": "mcp_tool_call_end",
                "call_id": "mcp_fail",
                "invocation": {"server":"x","tool":"y","arguments":{}},
                "duration":{"secs":0,"nanos":0},
                "result": {"Err":"connection refused"}
            }),
            None,
        )
        .unwrap();
        assert_eq!(msg.kind, kind::TOOL_ERROR);
        assert!(msg.text.contains("connection refused"));

        // ImageGenerationEnd → `**Generated image** {prompt}` + saved path.
        let msg = codex_event_msg_to_message(
            &serde_json::json!({
                "type": "image_generation_end",
                "call_id": "img_1",
                "status": "completed",
                "revised_prompt": "A red cat",
                "result": "ok",
                "saved_path": "/tmp/img.png"
            }),
            None,
        )
        .unwrap();
        assert_eq!(
            msg.text,
            "**Generated image**(`A red cat`)\n\nSaved: `/tmp/img.png`"
        );

        // ContextCompacted → system notice.
        let msg =
            codex_event_msg_to_message(&serde_json::json!({"type": "context_compacted"}), None)
                .unwrap();
        assert_eq!(msg.role, "system");
        assert_eq!(msg.text, "*Context compacted*");
        assert_eq!(msg.kind, kind::COMPACTION);
    }

    #[test]
    fn codex_failed_exec_appends_error_footer_below_output() {
        // Non-zero `Process exited with code` flips the leading status
        // marker to `✗` and prefixes the fence body with `Error: Exit
        // code N`. Aligns with Claude Code's TUI `⎿ Error: Exit code N`
        // line — same shape for both platforms.
        let dir = TestDir::new("codex-failed-exec");
        let path = dir.path().join("rollout.jsonl");
        fs::write(
            &path,
            r#"{"type":"session_meta","payload":{"id":"thread-fail","cwd":"/workspace/project"}}"#.to_string()
                + "\n"
                + r#"{"type":"event_msg","timestamp":"2026-05-01T00:00:00Z","payload":{"type":"user_message","message":"Run tests"}}"#
                + "\n"
                + r#"{"type":"response_item","timestamp":"2026-05-01T00:00:01Z","payload":{"type":"function_call","call_id":"call_1","name":"shell","arguments":"{\"cmd\":\"npm test\"}"}}"#
                + "\n"
                + r#"{"type":"response_item","timestamp":"2026-05-01T00:00:02Z","payload":{"type":"function_call_output","call_id":"call_1","output":"Wall time: 0.5 seconds\nProcess exited with code 1\nOutput:\nFAIL\ntrace"}}"#,
        )
        .unwrap();

        let detail = parse_codex_session(&path, "thread-fail").unwrap();
        let tool = detail
            .messages
            .iter()
            .find(|m| m.role == "tool")
            .expect("tool message present");
        assert_eq!(
            tool.text,
            "✗ **Bash**(`npm test`)\n\n````\nError: Exit code 1\nFAIL\ntrace\n````"
        );
    }

    #[test]
    fn codex_failed_exec_with_empty_output_still_shows_exit_code() {
        // User-reported scenario (rollout JSONL line copied verbatim from
        // .codex/sessions/...): an `rg` call that finds no matches
        // emits `Process exited with code 1` and an empty Output: section.
        // Termory must report `✗ (1)` so the user knows the failure mode
        // even when stdout/stderr were empty.
        let dir = TestDir::new("codex-empty-error");
        let path = dir.path().join("rollout.jsonl");
        let function_call_payload = "exec_command";
        let function_call_args = "{\"cmd\":\"rg --files -g '*release-airouter.yml' -g '!*node_modules*' -g '!web/**/node_modules/**'\",\"workdir\":\"/Users/john/Documents/airouter\",\"yield_time_ms\":1000,\"max_output_tokens\":4000}";
        let function_call_output = "Chunk ID: 9593aa\\nWall time: 0.0000 seconds\\nProcess exited with code 1\\nOriginal token count: 0\\nOutput:\\n";
        fs::write(
            &path,
            r#"{"type":"session_meta","payload":{"id":"thread-empty","cwd":"/workspace/project"}}"#.to_string()
                + "\n"
                + &format!(
                    r#"{{"type":"response_item","timestamp":"2026-05-20T13:39:48.676Z","payload":{{"type":"function_call","name":"{function_call_payload}","arguments":{:?},"call_id":"call_X"}}}}"#,
                    function_call_args
                )
                + "\n"
                + &format!(
                    r#"{{"type":"response_item","timestamp":"2026-05-20T13:39:48.834Z","payload":{{"type":"function_call_output","call_id":"call_X","output":"{function_call_output}"}}}}"#
                ),
        )
        .unwrap();

        let detail = parse_codex_session(&path, "thread-empty").unwrap();
        let tool = detail
            .messages
            .iter()
            .find(|m| m.role == "tool")
            .expect("tool message present");
        assert_eq!(
            tool.text,
            "✗ **Bash**(`rg --files -g '*release-airouter.yml' -g '!*node_modules*' -g '!web/**/node_modules/**'`)\n\n````\nError: Exit code 1\n````"
        );
    }

    #[test]
    fn codex_hides_environment_context_from_visible_messages() {
        let dir = TestDir::new("codex-environment-context");
        let path = dir.path().join("rollout.jsonl");
        // EventMsg::UserMessage carries the user input verbatim — Codex's
        // TUI never displays environment_context wrappers because they are
        // synthetic prefixes added before sending to the model, not real
        // user text. Termory applies the same filter (see
        // `is_codex_contextual_user_text` invoked from
        // `codex_user_message_event`).
        fs::write(
            &path,
            r#"{"type":"session_meta","payload":{"id":"thread-env","cwd":"/workspace/project"}}"#.to_string()
                + "\n"
                + r#"{"type":"event_msg","timestamp":"2026-05-01T00:00:00Z","payload":{"type":"user_message","message":"<environment_context>\n  <cwd>/Users/john/Documents</cwd>\n  <shell>zsh</shell>\n  <current_date>2026-05-15</current_date>\n  <timezone>Asia/Shanghai</timezone>\n</environment_context>"}}"#
                + "\n"
                + r#"{"type":"event_msg","timestamp":"2026-05-01T00:00:01Z","payload":{"type":"user_message","message":"Build the app"}}"#
                + "\n"
                + r#"{"type":"event_msg","timestamp":"2026-05-01T00:00:02Z","payload":{"type":"agent_message","message":"Done"}}"#,
        )
        .unwrap();

        let detail = parse_codex_session(&path, "thread-env").unwrap();
        assert_eq!(detail.session.title, "");
        assert_eq!(detail.session.message_count, 2);
        assert_eq!(detail.messages.len(), 2);
        assert_eq!(detail.messages[0].text, "Build the app");
        assert_eq!(detail.messages[1].text, "Done");
    }

    #[test]
    fn codex_hides_official_contextual_user_fragments() {
        let dir = TestDir::new("codex-contextual-fragments");
        let path = dir.path().join("rollout.jsonl");
        // Same contextual-fragment filter applied to EventMsg::UserMessage:
        // synthetic prefixes (`<user_instructions>`, `<skill_instructions>`,
        // etc.) never reach the display, while normal user input does.
        fs::write(
            &path,
            r#"{"type":"session_meta","payload":{"id":"thread-fragments","cwd":"/workspace/project"}}"#.to_string()
                + "\n"
                + r#"{"type":"event_msg","timestamp":"2026-05-01T00:00:00Z","payload":{"type":"user_message","message":"<user_instructions>\nUse repo conventions.\n</user_instructions>"}}"#
                + "\n"
                + r#"{"type":"event_msg","timestamp":"2026-05-01T00:00:01Z","payload":{"type":"user_message","message":"<skill_instructions>\nTest carefully.\n</skill_instructions>"}}"#
                + "\n"
                + r#"{"type":"event_msg","timestamp":"2026-05-01T00:00:02Z","payload":{"type":"user_message","message":"Fix contextual filtering"}}"#
                + "\n"
                + r#"{"type":"event_msg","timestamp":"2026-05-01T00:00:03Z","payload":{"type":"agent_message","message":"Done"}}"#,
        )
        .unwrap();

        let detail = parse_codex_session(&path, "thread-fragments").unwrap();
        assert_eq!(detail.session.title, "");
        assert_eq!(detail.session.message_count, 2);
        assert_eq!(detail.messages.len(), 2);
        assert_eq!(detail.messages[0].text, "Fix contextual filtering");
        assert_eq!(detail.messages[1].text, "Done");
    }

    // Removed: `codex_renders_resume_picker_tool_events`. The fixture used
    // `command_execution` and `mcp_tool_call` payload types under
    // `response_item` — neither exists in Codex's real `ResponseItem` enum
    // (codex-rs/protocol/src/models.rs: variants are `Message`,
    // `Reasoning`, `FunctionCall`, `FunctionCallOutput`, `LocalShellCall`,
    // `CustomToolCall`, `WebSearchCall`, `ImageGenerationCall`, ...).
    // Tool calls reach the TUI via `EventMsg` variants (PatchApplyEnd /
    // McpToolCallEnd / WebSearchEnd / ExecCommandEnd), not as
    // ResponseItem subtypes. The old test asserted a Termory-invented
    // format that never appears in real Codex sessions.

    #[test]
    fn codex_parses_exec_output_extracts_raw_and_exit_code() {
        // Exact shape Codex emits via ExecCommandToolOutput.response_text()
        // (codex-rs/core/src/tools/context.rs:409-434). Termory must strip
        // everything up to and including the `Output:` delimiter line, AND
        // record the exit code from the `Process exited with code N` line
        // so a non-zero exit can surface as a `**Error**` footer.
        let parsed = codex_parse_exec_output(
            "Chunk ID: 79f6dd\nWall time: 0.0000 seconds\nProcess exited with code 0\nOriginal token count: 0\nOutput:\ndiff --git a/foo b/foo\n+changed",
        );
        assert_eq!(parsed.raw, "diff --git a/foo b/foo\n+changed");
        assert_eq!(parsed.exit_code, Some(0));

        let parsed = codex_parse_exec_output(
            "Wall time: 0.5000 seconds\nProcess exited with code 1\nOutput:\nboom",
        );
        assert_eq!(parsed.raw, "boom");
        assert_eq!(parsed.exit_code, Some(1));

        // Older single-chunk wrapper with `Exit code:` prefix.
        let parsed = codex_parse_exec_output("Exit code: 2\nWall time: 0.1\nOutput:\nnope");
        assert_eq!(parsed.raw, "nope");
        assert_eq!(parsed.exit_code, Some(2));

        // Non-exec output (e.g., raw mcp result) passes through unchanged
        // and reports no exit code.
        let parsed = codex_parse_exec_output("mcp call returned: ok");
        assert_eq!(parsed.raw, "mcp call returned: ok");
        assert_eq!(parsed.exit_code, None);

        // Empty output after the delimiter — keep the delimiter trim.
        let parsed = codex_parse_exec_output("Chunk ID: 1\nWall time: 0.0000 seconds\nOutput:\n");
        assert_eq!(parsed.raw, "");
    }

    #[test]
    fn codex_apply_patch_header_uses_per_file_verb() {
        // Single-file add → `**Added** {path}` (matches Codex's
        // render_changes_block, diff_render.rs:421-431).
        let actions = codex_parse_patch_actions(
            "*** Begin Patch\n*** Add File: src/new.rs\n+content\n*** End Patch",
        );
        assert_eq!(
            actions,
            vec![CodexPatchAction {
                verb: "Added",
                path: "src/new.rs".to_string(),
                move_to: None,
            }]
        );
        assert_eq!(codex_patch_header(&actions), "**Added**(`src/new.rs`)");

        // Single-file edit with move → `**Edited** old → new`.
        let actions = codex_parse_patch_actions(
            "*** Update File: a/old.rs\n*** Move to: a/new.rs\n@@\n-x\n+y",
        );
        assert_eq!(
            actions,
            vec![CodexPatchAction {
                verb: "Edited",
                path: "a/old.rs".to_string(),
                move_to: Some("a/new.rs".to_string()),
            }]
        );
        assert_eq!(
            codex_patch_header(&actions),
            "**Edited**(`a/old.rs → a/new.rs`)"
        );

        // Delete.
        let actions = codex_parse_patch_actions("*** Delete File: tmp/out.txt");
        assert_eq!(codex_patch_header(&actions), "**Deleted**(`tmp/out.txt`)");

        // Multi-file → counts collapse to a single `**Edited** N files`.
        let actions = codex_parse_patch_actions(
            "*** Update File: a.rs\n@@\n*** Add File: b.rs\n*** Delete File: c.rs",
        );
        assert_eq!(actions.len(), 3);
        assert_eq!(codex_patch_header(&actions), "**Edited**(3 files)");

        // No recognized markers → generic fallback.
        let actions = codex_parse_patch_actions("diff --git a/x b/x\n...");
        assert!(actions.is_empty());
        assert_eq!(codex_patch_header(&actions), "**Applied patch**");
    }

    #[test]
    fn wrap_inline_code_picks_safe_delimiter_per_commonmark() {
        // No backticks in content — single backticks suffice.
        assert_eq!(wrap_inline_code("ls -la"), "`ls -la`");
        // Content has a backtick run of length 1, AND ends with a backtick,
        // so CommonMark 6.1 forces both: a length-2 delimiter AND padding
        // spaces between delimiter and content.
        assert_eq!(wrap_inline_code("echo `date`"), "`` echo `date` ``");
        // Content starting/ending with backtick gets padding spaces per
        // CommonMark spec section 6.1.
        assert_eq!(wrap_inline_code("`code`"), "`` `code` ``");
    }

    #[test]
    fn codex_strips_official_git_action_directives_from_assistant_markdown() {
        let text = codex_visible_assistant_markdown(
            "Done ::git-push{cwd=\"/workspace/project\" branch=\"main\"} next",
        );
        assert_eq!(text, "Done  next");

        let text = codex_visible_assistant_markdown(
            "::git-diff{cwd=\"/workspace/project\"}\nVisible\n::git-commit{message=\"ship\"}",
        );
        assert_eq!(text, "\nVisible");
    }

    #[test]
    fn claude_parses_project_jsonl_session() {
        let dir = TestDir::new("claude");
        let path = dir
            .path()
            .join("12345678-1234-1234-1234-123456789abc.jsonl");
        fs::write(
            &path,
            r#"{"sessionId":"12345678-1234-1234-1234-123456789abc","cwd":"/workspace/claude","timestamp":"2026-05-01T00:00:00Z","type":"user","message":{"role":"user","content":"Fix the login flow"}}"#,
        )
        .unwrap();

        let detail = parse_claude_session(&path).unwrap();
        assert_eq!(detail.session.id, "12345678-1234-1234-1234-123456789abc");
        assert_eq!(detail.session.project, "/workspace/claude");
        assert_eq!(detail.session.title, "Fix the login flow");
        assert_eq!(detail.messages.len(), 1);
        assert_eq!(detail.session.message_count, 1);
    }

    #[test]
    fn claude_merges_tool_use_with_matching_tool_result() {
        // tool_use is in an assistant message; the matching tool_result lives
        // in the next user message and references the original tool_use.id.
        // Termory should fold the result back into the tool_use card so the
        // user sees one "Bash(cmd) + output" block instead of two cards.
        let dir = TestDir::new("claude-tool-merge");
        let path = dir
            .path()
            .join("12345678-1234-1234-1234-123456789ac1.jsonl");
        fs::write(
            &path,
            r#"{"sessionId":"12345678-1234-1234-1234-123456789ac1","cwd":"/workspace/claude","timestamp":"2026-05-01T00:00:00Z","type":"user","message":{"role":"user","content":"list files"}}"#.to_string()
                + "\n"
                + r#"{"sessionId":"12345678-1234-1234-1234-123456789ac1","timestamp":"2026-05-01T00:00:01Z","type":"assistant","message":{"role":"assistant","content":[{"type":"text","text":"Running."},{"type":"tool_use","id":"toolu_1","name":"Bash","input":{"command":"ls"}}]}}"#
                + "\n"
                + r#"{"sessionId":"12345678-1234-1234-1234-123456789ac1","timestamp":"2026-05-01T00:00:02Z","type":"user","message":{"role":"user","content":[{"type":"tool_result","tool_use_id":"toolu_1","content":"src\nREADME.md"}]}}"#,
        )
        .unwrap();

        let detail = parse_claude_session(&path).unwrap();
        // user prompt + assistant text + merged tool card = 3 messages
        assert_eq!(detail.messages.len(), 3);
        let tool_card = detail
            .messages
            .iter()
            .find(|m| m.kind == kind::TOOL_USE)
            .expect("merged tool card should exist");
        // Unified tool card: `⏺` status marker on success (matches Claude
        // TUI's BLACK_CIRCLE prefix in constants/figures.ts:4) + bold verb
        // + inline-code argument + 4-backtick fence with the merged
        // tool_result body. Commands are wrapped in inline backticks for
        // markdown-injection safety.
        assert_eq!(
            tool_card.text,
            "⏺ **Bash**(`ls`)\n\n````\nsrc\nREADME.md\n````"
        );
    }

    #[test]
    fn claude_keeps_session_when_mid_session_sidechain_present() {
        // Regression test for the over-restrictive "any line has isSidechain"
        // whole-session hide. videcoding/cli only filters at LIST time on the
        // first line (sessionStorage.ts enrichLog), and per-message in the
        // transcript chain (findLatestMessage). Mid-session sidechain entries
        // (e.g. sub-agent invocations) must NOT make the session disappear.
        let dir = TestDir::new("claude-midsession-sidechain");
        let path = dir
            .path()
            .join("12345678-1234-1234-1234-123456789abf.jsonl");
        fs::write(
            &path,
            // First line: NOT sidechain (passes the LIST filter)
            r#"{"sessionId":"12345678-1234-1234-1234-123456789abf","cwd":"/workspace/claude","timestamp":"2026-05-01T00:00:00Z","type":"user","message":{"role":"user","content":"Build the auth flow"},"isSidechain":false}"#.to_string()
                + "\n"
                // Mid-session sub-agent sidechain entry — historically caused
                // Termory to hide the whole session.
                + r#"{"sessionId":"12345678-1234-1234-1234-123456789abf","timestamp":"2026-05-01T00:00:05Z","type":"user","message":{"role":"user","content":"sub-agent prompt"},"isSidechain":true}"#
                + "\n"
                + r#"{"sessionId":"12345678-1234-1234-1234-123456789abf","timestamp":"2026-05-01T00:00:10Z","type":"assistant","message":{"role":"assistant","content":[{"type":"text","text":"Built."}]}}"#,
        )
        .unwrap();

        let detail = parse_claude_session(&path).unwrap();
        assert_eq!(detail.session.id, "12345678-1234-1234-1234-123456789abf");
        // Sidechain entry filtered per-message; main thread (user + assistant)
        // remains.
        let texts: Vec<&str> = detail.messages.iter().map(|m| m.text.as_str()).collect();
        assert!(texts.iter().any(|t| t.contains("Build the auth flow")));
        assert!(texts.iter().any(|t| t.contains("Built.")));
        assert!(
            !texts.iter().any(|t| t.contains("sub-agent prompt")),
            "mid-session sidechain entry should be filtered per-message"
        );
    }

    #[test]
    fn claude_displays_local_command_metadata_like_resume_preview() {
        let dir = TestDir::new("claude-command");
        let path = dir
            .path()
            .join("12345678-1234-1234-1234-123456789abd.jsonl");
        fs::write(
            &path,
            r#"{"type":"user","message":{"role":"user","content":"<command-name>/doctor</command-name>\n<command-message>doctor</command-message>\n<command-args></command-args>"},"sessionId":"12345678-1234-1234-1234-123456789abd","timestamp":"2026-05-01T00:00:00Z","cwd":"/workspace/copilot.is"}"#.to_string()
                + "\n"
                + r#"{"type":"system","subtype":"local_command","content":"<local-command-stdout>Help me fix the issues reported by /doctor below.</local-command-stdout>","sessionId":"12345678-1234-1234-1234-123456789abd","timestamp":"2026-05-01T00:00:01Z","cwd":"/workspace/copilot.is"}"#,
        )
        .unwrap();

        let detail = parse_claude_session(&path).unwrap();
        assert_eq!(detail.session.title, "/doctor");
        assert_eq!(detail.messages[0].text, "/doctor");
        assert_eq!(
            detail.messages[1].text,
            "Help me fix the issues reported by /doctor below."
        );
        assert_eq!(detail.session.message_count, 1);
    }

    #[test]
    fn claude_displays_empty_local_command_output_without_xml_tags() {
        let dir = TestDir::new("claude-empty-command");
        let path = dir
            .path()
            .join("12345678-1234-1234-1234-123456789ac0.jsonl");
        fs::write(
            &path,
            r#"{"type":"user","message":{"role":"user","content":"config"},"sessionId":"12345678-1234-1234-1234-123456789ac0","timestamp":"2026-05-01T00:00:00Z","cwd":"/workspace/copilot.is"}"#.to_string()
                + "\n"
                + r#"{"type":"system","subtype":"local_command","content":"<local-command-stdout></local-command-stdout>","sessionId":"12345678-1234-1234-1234-123456789ac0","timestamp":"2026-05-01T00:00:01Z","cwd":"/workspace/copilot.is"}"#,
        )
        .unwrap();

        let detail = parse_claude_session(&path).unwrap();
        assert_eq!(detail.messages[1].text, "(no content)");
        assert!(!detail.messages[1].text.contains("local-command-stdout"));
    }

    #[test]
    fn claude_uses_ai_title_and_skips_slash_command_first_prompt_like_official_list() {
        let dir = TestDir::new("claude-ai-title");
        let path = dir
            .path()
            .join("803df946-490d-43fc-aaf2-124944d6a968.jsonl");
        fs::write(
            &path,
            r#"{"type":"user","message":{"role":"user","content":"<local-command-caveat>ignore local commands</local-command-caveat>"},"isMeta":true,"sessionId":"803df946-490d-43fc-aaf2-124944d6a968","timestamp":"2026-05-17T11:15:48.257Z","cwd":"/Users/john/Documents/ip125"}"#.to_string()
                + "\n"
                + r#"{"type":"user","message":{"role":"user","content":"<command-name>/clear</command-name>\n<command-message>clear</command-message>\n<command-args></command-args>"},"sessionId":"803df946-490d-43fc-aaf2-124944d6a968","timestamp":"2026-05-17T11:15:48.252Z","cwd":"/Users/john/Documents/ip125"}"#
                + "\n"
                + r#"{"type":"user","message":{"role":"user","content":"ai-image-editor 页面是否可以在默认页面添加对应演示图片"},"sessionId":"803df946-490d-43fc-aaf2-124944d6a968","timestamp":"2026-05-17T11:18:39.104Z","cwd":"/Users/john/Documents/ip125"}"#
                + "\n"
                + r#"{"type":"ai-title","aiTitle":"Add demo images for AI image editor features","sessionId":"803df946-490d-43fc-aaf2-124944d6a968"}"#,
        )
        .unwrap();

        let detail = parse_claude_session(&path).unwrap();
        assert_eq!(
            detail.session.title,
            "Add demo images for AI image editor features"
        );
        assert_ne!(detail.session.title, "803df946");
    }

    #[test]
    fn claude_hides_metadata_only_sessions_like_official_list() {
        let dir = TestDir::new("claude-metadata-only");
        let path = dir
            .path()
            .join("22345678-1234-1234-1234-123456789ac0.jsonl");
        fs::write(
            &path,
            r#"{"type":"user","message":{"role":"user","content":"<local-command-caveat>ignore local commands</local-command-caveat>"},"isMeta":true,"sessionId":"22345678-1234-1234-1234-123456789ac0","timestamp":"2026-05-17T11:15:48.257Z","cwd":"/Users/john/Documents/ip125"}"#.to_string()
                + "\n"
                + r#"{"type":"system","subtype":"local_command","content":"<local-command-stdout></local-command-stdout>","sessionId":"22345678-1234-1234-1234-123456789ac0","timestamp":"2026-05-17T11:15:49.257Z","cwd":"/Users/john/Documents/ip125"}"#,
        )
        .unwrap();

        assert!(parse_claude_session(&path).is_err());
    }

    #[test]
    fn claude_list_uses_head_tail_only_like_official_lite_reader() {
        let dir = TestDir::new("claude-lite");
        let path = dir
            .path()
            .join("32345678-1234-1234-1234-123456789ac0.jsonl");
        let filler = "x".repeat(CLAUDE_LITE_READ_BUF_SIZE + 128);
        fs::write(
            &path,
            r#"{"type":"user","message":{"role":"user","content":"<local-command-caveat>ignore local commands</local-command-caveat>"},"isMeta":true,"sessionId":"32345678-1234-1234-1234-123456789ac0","timestamp":"2026-05-17T11:15:48.257Z","cwd":"/Users/john/Documents/ip125"}"#.to_string()
                + "\n"
                + &filler
                + "\n"
                + r#"{"type":"ai-title","aiTitle":"Middle title should not be visible","sessionId":"32345678-1234-1234-1234-123456789ac0"}"#
                + "\n"
                + &filler,
        )
        .unwrap();

        assert!(parse_claude_lite_session(&path, None).is_err());
    }

    #[test]
    fn claude_scans_resume_sessions_like_videcoding_cli() {
        let dir = TestDir::new("claude-scan");
        let root = dir.path().join("projects");
        let project = root.join("-workspace-claude");
        let subagents = project.join("subagents");
        fs::create_dir_all(&subagents).unwrap();
        let visible = project.join("12345678-1234-1234-1234-123456789abe.jsonl");
        fs::write(
            &visible,
            r#"{"type":"user","message":{"role":"user","content":"<command-name>/doctor</command-name>\n<command-message>doctor</command-message>\n<command-args></command-args>"},"sessionId":"12345678-1234-1234-1234-123456789abe","timestamp":"2026-05-01T00:00:00Z","cwd":"/workspace/claude","isSidechain":false}"#.to_string()
                + "\n"
                + r#"{"type":"assistant","message":{"role":"assistant","content":[{"type":"thinking","thinking":"hidden"},{"type":"text","text":"Use npm test."},{"type":"tool_use","name":"Bash","input":{}}]},"sessionId":"12345678-1234-1234-1234-123456789abe","timestamp":"2026-05-01T00:00:02Z","cwd":"/workspace/claude"}"#
                + "\n"
                + r#"{"type":"last-prompt","lastPrompt":"Review failing tests","sessionId":"12345678-1234-1234-1234-123456789abe"}"#,
        )
        .unwrap();
        fs::write(
            subagents.join("12345678-1234-1234-1234-123456789abf.jsonl"),
            r#"{"type":"user","message":{"role":"user","content":"hidden"},"sessionId":"12345678-1234-1234-1234-123456789abf","timestamp":"2026-05-01T00:00:00Z","cwd":"/workspace/claude"}"#,
        )
        .unwrap();
        fs::write(
            project.join("not-a-session.jsonl"),
            r#"{"type":"user","message":{"role":"user","content":"hidden"},"sessionId":"not-a-session","timestamp":"2026-05-01T00:00:00Z","cwd":"/workspace/claude"}"#,
        )
        .unwrap();

        let sessions = scan_claude_projects(&root).unwrap();
        assert_eq!(sessions.len(), 1);
        assert_eq!(sessions[0].id, "12345678-1234-1234-1234-123456789abe");
        assert_eq!(sessions[0].title, "Review failing tests");
        assert_eq!(sessions[0].project, "/workspace/claude");
        assert_eq!(sessions[0].message_count, 2);
    }

    #[test]
    fn gemini_parses_project_chat_jsonl() {
        let dir = TestDir::new("gemini");
        let project_dir = dir.path().join("tmp").join("ip125");
        let chats_dir = project_dir.join("chats");
        fs::create_dir_all(&chats_dir).unwrap();
        fs::write(
            project_dir.join(".project_root"),
            "/Users/john/Documents/ip125",
        )
        .unwrap();
        let path = chats_dir.join("session-2026-04-30T06-47-0686029a.jsonl");
        fs::write(
            &path,
            r#"{"sessionId":"0686029a-fc18-4f5c-9a35-1942f63575d0","projectHash":"hash","startTime":"2026-04-30T06:47:46.299Z","lastUpdated":"2026-04-30T06:47:46.299Z","kind":"main"}"#.to_string()
                + "\n"
                + r#"{"id":"user-1","timestamp":"2026-04-30T06:49:11.414Z","type":"user","content":[{"text":"Review code"}]}"#
                + "\n"
                + r#"{"id":"assistant-1","timestamp":"2026-04-30T06:49:18.370Z","type":"gemini","content":"Looks good"}"#,
        )
        .unwrap();

        let detail = parse_gemini_jsonl_session(&path).unwrap();
        assert_eq!(detail.session.id, "0686029a-fc18-4f5c-9a35-1942f63575d0");
        assert_eq!(detail.session.project, "/Users/john/Documents/ip125");
        assert_eq!(detail.session.title, "Review code");
        assert_eq!(detail.messages.len(), 2);
        assert_eq!(detail.messages[1].role, "assistant");
    }

    #[test]
    fn gemini_scans_official_session_files_like_list_sessions() {
        let dir = TestDir::new("gemini-scan");
        let project_dir = dir.path().join("tmp").join("project-hash");
        let chats_dir = project_dir.join("chats");
        fs::create_dir_all(&chats_dir).unwrap();
        fs::write(project_dir.join(".project_root"), "/workspace/gemini").unwrap();
        fs::write(
            chats_dir.join("session-2026-05-01T00-00-aaaaaaaa.jsonl"),
            r#"{"sessionId":"main-session","projectHash":"project-hash","startTime":"2026-05-01T00:00:00Z","lastUpdated":"2026-05-01T00:02:00Z","kind":"main","summary":"Summarized title"}"#.to_string()
                + "\n"
                + r#"{"id":"msg-1","timestamp":"2026-05-01T00:01:00Z","type":"user","content":[{"text":"First prompt"}]}"#
                + "\n"
                + r#"{"id":"msg-2","timestamp":"2026-05-01T00:02:00Z","type":"gemini","content":[{"text":"Reply"}],"toolCalls":[{"id":"tool-1","name":"run_shell_command","displayName":"Shell","description":"Ran tests","status":"success","resultDisplay":"ok"}]}"#,
        )
        .unwrap();
        fs::write(
            chats_dir.join("session-2026-05-01T00-03-bbbbbbbb.jsonl"),
            r#"{"sessionId":"subagent-session","projectHash":"project-hash","startTime":"2026-05-01T00:03:00Z","lastUpdated":"2026-05-01T00:04:00Z","kind":"subagent"}"#.to_string()
                + "\n"
                + r#"{"id":"msg-3","type":"user","content":[{"text":"Hidden subagent"}]}"#,
        )
        .unwrap();
        fs::write(
            chats_dir.join("session-2026-05-01T00-05-cccccccc.jsonl"),
            r#"{"sessionId":"empty-session","projectHash":"project-hash","startTime":"2026-05-01T00:05:00Z","lastUpdated":"2026-05-01T00:05:00Z","kind":"main"}"#.to_string()
                + "\n"
                + r#"{"id":"msg-4","type":"info","content":"Only info"}"#,
        )
        .unwrap();

        let sessions = scan_gemini_chats_dir(&chats_dir).unwrap();
        assert_eq!(sessions.len(), 1);
        assert_eq!(sessions[0].id, "main-session");
        assert_eq!(sessions[0].project, "/workspace/gemini");
        assert_eq!(sessions[0].title, "Summarized title");
        assert_eq!(sessions[0].message_count, 2);
        assert_eq!(sessions[0].preview, "");

        let detail =
            parse_gemini_jsonl_session(&chats_dir.join("session-2026-05-01T00-00-aaaaaaaa.jsonl"))
                .unwrap();
        assert_eq!(detail.messages.len(), 3);
        assert_eq!(detail.messages[0].role, "user");
        assert_eq!(detail.messages[1].role, "assistant");
        assert_eq!(detail.messages[2].role, "tool");
        assert!(detail.messages[2].text.contains("Ran tests"));
    }

    #[test]
    fn gemini_falls_back_to_file_mtime_when_timestamps_missing() {
        // packages/cli/src/utils/sessionUtils.ts:getAllSessionFiles falls back
        // to fs.stat(filePath).mtime when content.startTime/lastUpdated are
        // missing. Termory should match that — sessions without timestamps
        // must still be listed, not silently dropped.
        let dir = TestDir::new("gemini-no-timestamps");
        let project_dir = dir.path().join("tmp").join("p-no-ts");
        let chats_dir = project_dir.join("chats");
        fs::create_dir_all(&chats_dir).unwrap();
        fs::write(project_dir.join(".project_root"), "/workspace/no-ts").unwrap();
        let path = chats_dir.join("session-2026-05-21T00-00-dddddddd.jsonl");
        fs::write(
            &path,
            // Note: no startTime, no lastUpdated.
            r#"{"sessionId":"no-ts","projectHash":"p-no-ts","kind":"main"}"#.to_string()
                + "\n"
                + r#"{"id":"u","type":"user","content":[{"text":"hi"}]}"#,
        )
        .unwrap();

        let detail = parse_gemini_jsonl_session(&path).unwrap();
        assert_eq!(detail.session.id, "no-ts");
        // Must have non-empty timestamps; the value came from file mtime
        // (or fell through to "now"), so we only check non-empty.
        assert!(
            detail
                .session
                .started_at
                .as_deref()
                .is_some_and(|s| !s.is_empty()),
            "started_at should fall back to file mtime / now"
        );
        assert!(
            detail
                .session
                .updated_at
                .as_deref()
                .is_some_and(|s| !s.is_empty()),
            "updated_at should fall back to file mtime / now"
        );
    }

    #[test]
    fn gemini_formats_parts_like_official_part_to_string() {
        // inlineData parts carry base64 binary content (images / audio /
        // etc.). We can't show them in markdown, so emit a small italic
        // marker that tells the user what was attached.
        let text = gemini_content_text_raw(&serde_json::json!([
            {"text":"before"},
            {"inlineData":{"mimeType":"image/png","data":"AAAA"}},
            {"text":"after"}
        ]))
        .unwrap();
        assert_eq!(text, "before*Inline data (image/png)*after");

        // executableCode → fenced block tagged with the language so
        // highlight.js can pick it up later if syntax highlighting is
        // re-enabled (Gemini TUI renders these in `ExecutableCodeRenderer`).
        let text = gemini_content_text_raw(&serde_json::json!([
            {"executableCode":{"code":"print('hi')","language":"PYTHON"}}
        ]))
        .unwrap();
        assert_eq!(text, "```python\nprint('hi')\n```");

        // codeExecutionResult → 4-backtick output block, with an italic
        // outcome footer only when the outcome is non-OK.
        let text = gemini_content_text_raw(&serde_json::json!([
            {"codeExecutionResult":{"output":"hi","outcome":"OUTCOME_OK"}}
        ]))
        .unwrap();
        assert_eq!(text, "````\nhi\n````");

        let text = gemini_content_text_raw(&serde_json::json!([
            {"codeExecutionResult":{"output":"boom","outcome":"OUTCOME_FAILED"}}
        ]))
        .unwrap();
        assert_eq!(text, "````\nboom\n````\n\n*Outcome: OUTCOME_FAILED*");
    }

    #[test]
    fn opencode_parses_storage_session_and_messages() {
        let dir = TestDir::new("opencode");
        let storage = dir.path().join("storage");
        let session_dir = storage.join("session").join("project-1");
        let message_dir = storage.join("message").join("ses_123");
        fs::create_dir_all(&session_dir).unwrap();
        fs::create_dir_all(&message_dir).unwrap();
        let session_path = session_dir.join("ses_123.json");
        fs::write(
            &session_path,
            r#"{"id":"ses_123","projectID":"project-1","directory":"/workspace/opencode","title":"Implement feature","time":{"created":1714521600000,"updated":1714521700000}}"#,
        )
        .unwrap();
        fs::write(
            message_dir.join("msg_1.json"),
            r#"{"id":"msg_1","role":"user","time":1714521600000,"content":[{"text":"Implement feature"}]}"#,
        )
        .unwrap();
        fs::write(
            message_dir.join("msg_2.json"),
            r#"{"id":"msg_2","role":"assistant","time":1714521700000,"content":[{"text":"Done"}]}"#,
        )
        .unwrap();

        let detail = parse_opencode_storage_session(&session_path).unwrap();
        assert_eq!(detail.session.id, "ses_123");
        assert_eq!(detail.session.project, "/workspace/opencode");
        assert_eq!(detail.session.title, "Implement feature");
        assert_eq!(detail.messages.len(), 2);
    }

    #[test]
    fn opencode_reads_official_sqlite_sessions_messages_and_parts() {
        let dir = TestDir::new("opencode-db");
        let db = dir.path().join("opencode.db");
        let conn = Connection::open(&db).unwrap();
        conn.execute_batch(
            "create table session (
                id text primary key,
                project_id text not null,
                workspace_id text,
                parent_id text,
                slug text not null,
                directory text not null,
                path text,
                title text not null,
                version text not null,
                time_created integer not null,
                time_updated integer not null,
                time_archived integer
            );
            create table message (
                id text primary key,
                session_id text not null,
                time_created integer not null,
                time_updated integer not null,
                data text not null
            );
            create table part (
                id text primary key,
                message_id text not null,
                session_id text not null,
                time_created integer not null,
                time_updated integer not null,
                data text not null
            );",
        )
        .unwrap();
        conn.execute(
            "insert into session values (?1, 'project-1', null, null, 'slug', ?2, null, ?3, '0.1.0', 1714521600000, 1714521700000, null)",
            ("ses_123", "/workspace/opencode", "Implement feature"),
        )
        .unwrap();
        conn.execute(
            "insert into session values ('child', 'project-1', null, 'ses_123', 'child', '/workspace/opencode', null, 'Child', '0.1.0', 1714521600000, 1714521700000, null)",
            [],
        )
        .unwrap();
        conn.execute(
            "insert into session values ('archived', 'project-1', null, null, 'archived', '/workspace/opencode', null, 'Archived', '0.1.0', 1714521600000, 1714521700000, 1714521800000)",
            [],
        )
        .unwrap();
        conn.execute(
            "insert into message values ('msg_1', 'ses_123', 1714521600000, 1714521600000, ?1)",
            [r#"{"role":"user","agent":"build","time":{"created":1714521600000}}"#],
        )
        .unwrap();
        conn.execute(
            "insert into part values ('part_1', 'msg_1', 'ses_123', 1714521600000, 1714521600000, ?1)",
            [r#"{"type":"text","text":"Implement feature"}"#],
        )
        .unwrap();
        conn.execute(
            "insert into message values ('msg_2', 'ses_123', 1714521700000, 1714521700000, ?1)",
            [r#"{"role":"assistant","time":{"created":1714521700000},"modelID":"claude"}"#],
        )
        .unwrap();
        conn.execute(
            "insert into part values ('part_2', 'msg_2', 'ses_123', 1714521700000, 1714521700000, ?1)",
            [r#"{"type":"text","text":"Done"}"#],
        )
        .unwrap();
        conn.execute(
            "insert into part values ('part_3', 'msg_2', 'ses_123', 1714521700001, 1714521700001, ?1)",
            [r#"{"type":"tool","title":"edited src/main.ts","output":"hidden"}"#],
        )
        .unwrap();

        let sessions = scan_opencode_db(&db).unwrap();
        assert_eq!(sessions.len(), 1);
        assert_eq!(sessions[0].id, "ses_123");
        assert_eq!(sessions[0].project, "/workspace/opencode");
        assert_eq!(sessions[0].title, "Implement feature");
        assert_eq!(sessions[0].message_count, 2);

        let detail = parse_opencode_db_session(&db, "ses_123").unwrap();
        assert_eq!(detail.messages.len(), 2);
        assert_eq!(detail.messages[0].role, "user");
        assert_eq!(detail.messages[0].text, "Implement feature");
        assert_eq!(detail.messages[1].role, "assistant");
        assert_eq!(detail.messages[1].text, "Done");
        assert_eq!(detail.session.message_count, 2);
    }

    #[test]
    fn opencode_skips_synthetic_text_parts() {
        // packages/opencode/src/cli/cmd/tui/util/transcript.ts formatPart
        // gates text rendering on `!part.synthetic` — synthetic text parts
        // are prompt-builder-injected (env block, tool acks, etc.) and
        // intentionally hidden from the user-facing transcript.
        let dir = TestDir::new("opencode-synthetic");
        let db = dir.path().join("opencode.db");
        let conn = Connection::open(&db).unwrap();
        conn.execute_batch(
            "create table session (
                id text primary key,
                project_id text not null,
                workspace_id text,
                parent_id text,
                slug text not null,
                directory text not null,
                path text,
                title text not null,
                version text not null,
                time_created integer not null,
                time_updated integer not null,
                time_archived integer
            );
            create table message (
                id text primary key,
                session_id text not null,
                time_created integer not null,
                time_updated integer not null,
                data text not null
            );
            create table part (
                id text primary key,
                message_id text not null,
                session_id text not null,
                time_created integer not null,
                time_updated integer not null,
                data text not null
            );",
        )
        .unwrap();
        conn.execute(
            "insert into session values ('ses_syn', 'project-1', null, null, 'slug', '/w/oc', null, 'Synthetic test', '0.1.0', 1714521600000, 1714521700000, null)",
            [],
        )
        .unwrap();
        conn.execute(
            "insert into message values ('msg_1', 'ses_syn', 1714521600000, 1714521600000, ?1)",
            [r#"{"role":"user","time":{"created":1714521600000}}"#],
        )
        .unwrap();
        // Synthetic env block — should be skipped.
        conn.execute(
            "insert into part values ('part_synth', 'msg_1', 'ses_syn', 1714521600000, 1714521600000, ?1)",
            [r#"{"type":"text","text":"<env>cwd=/w/oc</env>","synthetic":true}"#],
        )
        .unwrap();
        // Real user prompt — should be included.
        conn.execute(
            "insert into part values ('part_real', 'msg_1', 'ses_syn', 1714521600001, 1714521600001, ?1)",
            [r#"{"type":"text","text":"Real prompt"}"#],
        )
        .unwrap();

        let detail = parse_opencode_db_session(&db, "ses_syn").unwrap();
        assert_eq!(detail.messages.len(), 1);
        let text = &detail.messages[0].text;
        assert!(
            text.contains("Real prompt"),
            "non-synthetic text part must remain"
        );
        assert!(
            !text.contains("<env>"),
            "synthetic text part must be filtered"
        );
    }

    #[test]
    fn opencode_reads_v2_session_message_table_and_titles_like_official() {
        let dir = TestDir::new("opencode-v2-db");
        let db = dir.path().join("opencode.db");
        let conn = Connection::open(&db).unwrap();
        conn.execute_batch(
            "create table session (
                id text primary key,
                project_id text not null,
                workspace_id text,
                parent_id text,
                slug text not null,
                directory text not null,
                path text,
                title text not null,
                version text not null,
                time_created integer not null,
                time_updated integer not null,
                time_archived integer
            );
            create table session_message (
                id text primary key,
                session_id text not null,
                type text not null,
                time_created integer not null,
                time_updated integer not null,
                data text not null
            );",
        )
        .unwrap();
        conn.execute(
            "insert into session values (?1, 'project-1', null, null, 'slug', ?2, null, ?3, '0.1.0', 1714521600000, 1714521700000, null)",
            (
                "ses_v2",
                "/workspace/opencode",
                "New session - 2026-05-01T00:00:00.000Z",
            ),
        )
        .unwrap();
        conn.execute(
            "insert into session_message values ('msg_1', 'ses_v2', 'user', 1714521600000, 1714521600000, ?1)",
            [r#"{"text":"Implement feature","time":{"created":1714521600000}}"#],
        )
        .unwrap();
        conn.execute(
            "insert into session_message values ('msg_2', 'ses_v2', 'assistant', 1714521700000, 1714521700000, ?1)",
            [r#"{"content":[{"type":"text","text":"Done"},{"type":"reasoning","text":"hidden"}],"time":{"created":1714521700000}}"#],
        )
        .unwrap();

        let sessions = scan_opencode_db(&db).unwrap();
        assert_eq!(sessions[0].title, "New session");
        assert_eq!(sessions[0].message_count, 2);

        let detail = parse_opencode_db_session(&db, "ses_v2").unwrap();
        assert_eq!(detail.session.title, "New session");
        assert_eq!(detail.messages.len(), 2);
        assert_eq!(detail.messages[0].text, "Implement feature");
        // Reasoning content is rendered via the unified italic-blockquote
        // pattern (`> *...*`) so it visually separates from the regular
        // assistant text above.
        assert_eq!(detail.messages[1].text, "Done\n\n> *hidden*");
    }

    #[test]
    fn opencode_formats_v2_special_tool_parts_like_official_preview() {
        // OpenCode tools all use the unified `**Verb**(args)` shape now —
        // same as Codex / Claude / Gemini. The verb still comes from
        // session-v2.tsx but the OpenCode-specific decorations (`\#` H1
        // escape, `←` arrow, `↳ Loaded`, `"pattern" in path` quoted form)
        // are dropped in favor of the unified shape.

        // Bash — completed with output goes in 4-backtick fence;
        // `description` field restored as `\# {description}` between the
        // header and the fence (preserving the OpenCode BlockTool title
        // semantics).
        let bash = opencode_v2_tool_part_text(&serde_json::json!({
            "type":"tool",
            "name":"bash",
            "state":{
                "status":"completed",
                "input":{"command":"npm test","description":"Run tests"},
                "content":[{"type":"text","text":"ok"}]
            }
        }))
        .unwrap();
        assert_eq!(
            bash,
            "**Shell**(`npm test`)\n\n\\# Run tests\n\n```bash\n$ npm test\nok\n```"
        );

        // Read — `[other=...]` continues to display the other input fields
        // verbatim; metadata.loaded entries dropped (TUI-only `↳ Loaded`).
        let read = opencode_v2_tool_part_text(&serde_json::json!({
            "type":"tool",
            "name":"read",
            "state":{"input":{"filePath":"src/main.ts","offset":10,"limit":20}},
            "provider":{"metadata":{"loaded":["src/main.ts"]}}
        }))
        .unwrap();
        assert_eq!(
            read,
            "**Read**(`src/main.ts` [limit=20, offset=10])\\\n↳ Loaded src/main.ts"
        );

        // TodoWrite — todos rendered as a list with status icons.
        let todos = opencode_v2_tool_part_text(&serde_json::json!({
            "type":"tool",
            "name":"todowrite",
            "state":{"input":{"todos":[
                {"status":"completed","content":"Read code"},
                {"status":"in_progress","content":"Patch parser"},
                {"status":"pending","content":"Run tests"}
            ]},"status":"completed"}
        }))
        .unwrap();
        assert_eq!(
            todos,
            "**Todos**\n\n✓ Read code\n~ Patch parser\n☐ Run tests"
        );

        // Glob — `**Glob**(pattern: ..., path: ... — N matches)`.
        let glob = opencode_v2_tool_part_text(&serde_json::json!({
            "type":"tool",
            "name":"glob",
            "state":{"input":{"pattern":"**/*.ts","path":"src"}},
            "metadata":{"count":12}
        }))
        .unwrap();
        assert_eq!(
            glob,
            "**Glob**(pattern: `**/*.ts`, path: `src` — 12 matches)"
        );

        // Grep — singular/plural still respected in the trailing summary.
        let grep = opencode_v2_tool_part_text(&serde_json::json!({
            "type":"tool",
            "name":"grep",
            "state":{"input":{"pattern":"TODO","path":"src/lib.rs"}},
            "metadata":{"matches":1}
        }))
        .unwrap();
        assert_eq!(
            grep,
            "**Grep**(pattern: `TODO`, path: `src/lib.rs` — 1 match)"
        );

        // Edit — diff content in ```diff fence; `←` arrow dropped.
        let edit = opencode_v2_tool_part_text(&serde_json::json!({
            "type":"tool",
            "name":"edit",
            "state":{"input":{"filePath":"src/main.ts"}},
            "metadata":{"diff":"- old\n+ new"}
        }))
        .unwrap();
        assert_eq!(
            edit,
            "**Edit**(`src/main.ts`)\n\n```diff\n- old\n+ new\n```"
        );
    }

    #[test]
    fn opencode_reads_official_115_message_parts_with_tool_field() {
        let dir = TestDir::new("opencode-115-parts");
        let db = dir.path().join("opencode.db");
        let conn = Connection::open(&db).unwrap();
        conn.execute_batch(
            "create table session (
                id text primary key,
                project_id text not null,
                workspace_id text,
                parent_id text,
                slug text not null,
                directory text not null,
                path text,
                title text not null,
                version text not null,
                time_created integer not null,
                time_updated integer not null,
                time_archived integer
            );
            create table message (
                id text primary key,
                session_id text not null,
                time_created integer not null,
                time_updated integer not null,
                data text not null
            );
            create table part (
                id text primary key,
                message_id text not null,
                session_id text not null,
                time_created integer not null,
                time_updated integer not null,
                data text not null
            );
            create table session_message (
                id text primary key,
                session_id text not null,
                type text not null,
                time_created integer not null,
                time_updated integer not null,
                data text not null
            );",
        )
        .unwrap();
        conn.execute(
            "insert into session values ('ses_115', 'project-1', null, null, 'slug', '/workspace/opencode', null, 'Implement feature', '1.15.1', 1714521600000, 1714521700000, null)",
            [],
        )
        .unwrap();
        conn.execute(
            "insert into message values ('msg_1', 'ses_115', 1714521600000, 1714521600000, ?1)",
            [r#"{"role":"assistant","time":{"created":1714521600000},"agent":"build","modelID":"claude"}"#],
        )
        .unwrap();
        conn.execute(
            "insert into part values ('part_1', 'msg_1', 'ses_115', 1714521600000, 1714521600000, ?1)",
            [r#"{"type":"text","text":"Running tests"}"#],
        )
        .unwrap();
        conn.execute(
            "insert into part values ('part_2', 'msg_1', 'ses_115', 1714521600001, 1714521600001, ?1)",
            [r#"{"type":"tool","tool":"shell","state":{"status":"completed","input":{"command":"npm test","description":"Run tests"},"output":"ok","metadata":{"output":"ok"}}}"#],
        )
        .unwrap();

        let detail = parse_opencode_db_session(&db, "ses_115").unwrap();
        // Each part now produces its own SessionMessage so tool calls carry
        // their TUI label (Bash) on a separate entry instead of being joined
        // into the assistant text.
        assert_eq!(detail.messages.len(), 2);
        assert_eq!(detail.messages[0].role, "assistant");
        assert_eq!(detail.messages[0].text, "Running tests");
        assert_eq!(detail.messages[1].role, "tool");
        assert_eq!(detail.messages[1].kind, "tool_use");
        assert_eq!(
            detail.messages[1].text,
            "**Shell**(`npm test`)\n\n\\# Run tests\n\n```bash\n$ npm test\nok\n```"
        );
    }

    // ------------------------------------------------------------------
    // Memory scanning tests
    // ------------------------------------------------------------------

    #[test]
    fn decode_claude_project_slug_translates_dashes_to_slashes() {
        assert_eq!(
            decode_claude_project_slug("-Users-john-Documents-foo"),
            "/Users/john/Documents/foo"
        );
        assert_eq!(decode_claude_project_slug("foo"), "foo");
        assert_eq!(decode_claude_project_slug(""), "");
    }

    #[test]
    fn split_memory_frontmatter_extracts_yaml_fields_and_body() {
        let raw = "---\nname: user-role\ndescription: notes about the user\nmetadata:\n  type: user\n---\n\nbody line one\nbody line two\n";
        let (front, body) = split_memory_frontmatter(raw);
        let front = front.expect("frontmatter map");
        assert_eq!(front.get("name").map(String::as_str), Some("user-role"));
        assert_eq!(
            front.get("description").map(String::as_str),
            Some("notes about the user")
        );
        // `type:` is nested under metadata; the simple parser flattens to the key it finds last
        assert_eq!(front.get("type").map(String::as_str), Some("user"));
        assert!(body.starts_with("body line one"));
    }

    #[test]
    fn split_memory_frontmatter_returns_none_when_no_frontmatter() {
        let raw = "just markdown\nno frontmatter";
        let (front, body) = split_memory_frontmatter(raw);
        assert!(front.is_none());
        assert_eq!(body, raw);
    }

    #[test]
    fn memory_session_from_file_marks_empty_body_with_zero_message_count() {
        let dir = TestDir::new("memory-empty");
        let empty = dir.path().join("empty.md");
        fs::write(&empty, "").unwrap();
        let only_frontmatter = dir.path().join("only-front.md");
        fs::write(&only_frontmatter, "---\nname: foo\n---\n").unwrap();
        let with_body = dir.path().join("with-body.md");
        fs::write(&with_body, "some content").unwrap();

        let s_empty = memory_session_from_file(&empty, "label").unwrap();
        assert_eq!(s_empty.message_count, 0);
        let s_front = memory_session_from_file(&only_frontmatter, "label").unwrap();
        assert_eq!(s_front.message_count, 0);
        let s_body = memory_session_from_file(&with_body, "label").unwrap();
        assert_eq!(s_body.message_count, 1);
    }

    #[test]
    fn memory_tool_for_file_classifies_known_filenames() {
        assert_eq!(memory_tool_for_file("CLAUDE.md"), "claude");
        assert_eq!(memory_tool_for_file("CLAUDE.local.md"), "claude");
        assert_eq!(memory_tool_for_file("AGENTS.md"), "agents");
        assert_eq!(memory_tool_for_file("GEMINI.md"), "gemini");
        assert_eq!(memory_tool_for_file("default.rules"), "rules");
        assert_eq!(memory_tool_for_file("unknown.md"), "memory");
    }

    #[test]
    fn gemini_project_label_reads_project_root_file_or_falls_back() {
        let dir = TestDir::new("gemini-label");
        let with_root = dir.path().join("with");
        fs::create_dir_all(&with_root).unwrap();
        fs::write(with_root.join(".project_root"), "/some/cwd\n").unwrap();
        assert_eq!(gemini_project_label(&with_root), "/some/cwd");

        let no_root = dir.path().join("hash-id");
        fs::create_dir_all(&no_root).unwrap();
        assert_eq!(gemini_project_label(&no_root), "hash-id");
    }

    #[test]
    fn push_memory_files_recursive_skips_dot_entries_and_tags_with_tool() {
        let dir = TestDir::new("recurse");
        let base = dir.path();
        fs::write(base.join("top.md"), "top body").unwrap();
        fs::create_dir_all(base.join("skills/skill-a")).unwrap();
        fs::write(base.join("skills/skill-a/SKILL.md"), "skill body").unwrap();
        fs::create_dir_all(base.join(".git")).unwrap();
        fs::write(base.join(".git/HEAD"), "ref: refs/heads/main").unwrap();
        fs::create_dir_all(base.join(".inbox")).unwrap();
        fs::write(base.join(".inbox/extraction.patch"), "patch").unwrap();
        fs::write(base.join("ignored.txt"), "not md").unwrap();

        let mut out = Vec::new();
        push_memory_files_recursive(base, base, "label", "codex", &mut out);
        let mut paths: Vec<String> = out.iter().map(|s| s.id.clone()).collect();
        paths.sort();
        assert_eq!(paths, vec!["skills/skill-a/SKILL.md", "top.md"]);
        for entry in &out {
            assert_eq!(entry.preview, "codex");
            assert_eq!(entry.source, "Memory");
            assert_eq!(entry.project, "label");
        }
    }

    #[test]
    fn push_tagged_instruction_file_joins_multi_tool_tags_into_preview() {
        let dir = TestDir::new("tagged");
        let path = dir.path().join("AGENTS.md");
        fs::write(&path, "body").unwrap();
        let mut out = Vec::new();
        push_tagged_instruction_file(&path, "/some/project", &["codex", "opencode"], &mut out);
        assert_eq!(out.len(), 1);
        assert_eq!(out[0].preview, "codex,opencode");
        assert_eq!(out[0].project, "/some/project");
    }

    #[test]
    fn push_tagged_instruction_file_silently_skips_missing_files() {
        let dir = TestDir::new("tagged-missing");
        let mut out = Vec::new();
        push_tagged_instruction_file(&dir.path().join("nope.md"), "label", &["claude"], &mut out);
        assert!(out.is_empty());
    }

    fn memory_test_cwds(cwd: &str, _tools: &[&str]) -> HashSet<String> {
        // Plant a .git directory in the cwd so the ancestor-walk implemented in
        // scan_memory stops at the test cwd and does not ascend into /tmp (or
        // wherever the test root sits). `_tools` is unused now that scan_memory
        // no longer takes per-tool cwd sets — kept on the signature so existing
        // call sites read naturally.
        let _ = fs::create_dir_all(Path::new(cwd).join(".git"));
        [cwd.to_string()].into_iter().collect()
    }

    #[test]
    fn scan_memory_shows_project_agents_md_with_both_tags_when_no_session_evidence() {
        let dir = TestDir::new("agents-fallback");
        let project = dir.path();
        let agents = project.join("AGENTS.md");
        fs::write(&agents, "body").unwrap();
        let claude_md = project.join("CLAUDE.md");
        fs::write(&claude_md, "body").unwrap();
        let gemini_md = project.join("GEMINI.md");
        fs::write(&gemini_md, "body").unwrap();

        let cwd = project.to_string_lossy().to_string();
        let project_cwds = memory_test_cwds(&cwd, &["Claude"]);
        let sessions = scan_memory(&project_cwds).unwrap();

        let agents_path = agents.to_string_lossy().to_string();
        let agents_entry = sessions
            .iter()
            .find(|s| s.path == agents_path)
            .expect("AGENTS.md should still be shown (file exists on disk)");
        assert_eq!(
            agents_entry.preview, "codex,opencode",
            "AGENTS.md falls back to both tags when neither tool has a session in this cwd"
        );
        let claude_path = claude_md.to_string_lossy().to_string();
        assert!(
            sessions
                .iter()
                .any(|s| s.path == claude_path && s.preview == "claude,opencode"),
            "project CLAUDE.md should be tagged claude,opencode (OpenCode reads it as fallback)"
        );
        let gemini_path = gemini_md.to_string_lossy().to_string();
        assert!(
            sessions
                .iter()
                .any(|s| s.path == gemini_path && s.preview == "gemini"),
            "GEMINI.md should always be shown with gemini tag"
        );
    }

    #[test]
    fn scan_memory_always_tags_project_agents_md_with_both_codex_and_opencode() {
        // The AGENTS.md spec is tool-neutral (always usable by both Codex and
        // OpenCode), so we ignore session evidence and always tag with both.
        for (case, tools) in [
            ("agents-codex-only", &["Codex"][..]),
            ("agents-opencode-only", &["OpenCode"][..]),
            ("agents-no-sessions", &[][..]),
        ] {
            let dir = TestDir::new(case);
            let project = dir.path();
            let agents = project.join("AGENTS.md");
            fs::write(&agents, "body").unwrap();
            let cwd = project.to_string_lossy().to_string();
            let project_cwds = memory_test_cwds(&cwd, tools);
            let sessions = scan_memory(&project_cwds).unwrap();
            let entry = sessions
                .iter()
                .find(|s| s.path == agents.to_string_lossy())
                .unwrap_or_else(|| panic!("AGENTS.md missing for case {case}"));
            assert_eq!(
                entry.preview, "codex,opencode",
                "AGENTS.md always shows both tags (case: {case})"
            );
        }
    }

    #[test]
    fn scan_memory_tags_project_agents_md_with_both_codex_and_opencode_sessions() {
        // AGENTS.local.md is not in any official tool's spec; the supported
        // overrides come from AGENTS.override.md (Codex). This test only
        // covers AGENTS.md now.
        let dir = TestDir::new("agents-both");
        let project = dir.path();
        let agents = project.join("AGENTS.md");
        fs::write(&agents, "body").unwrap();
        let cwd = project.to_string_lossy().to_string();
        let project_cwds = memory_test_cwds(&cwd, &["Codex", "OpenCode"]);
        let sessions = scan_memory(&project_cwds).unwrap();

        let main = sessions
            .iter()
            .find(|s| s.path == agents.to_string_lossy())
            .expect("AGENTS.md should be present");
        assert_eq!(main.preview, "codex,opencode");
    }

    #[test]
    fn scan_memory_picks_up_dot_claude_claude_md_in_project() {
        let dir = TestDir::new("dot-claude");
        let project = dir.path();
        fs::create_dir_all(project.join(".claude")).unwrap();
        let scoped = project.join(".claude").join("CLAUDE.md");
        fs::write(&scoped, "scoped body").unwrap();
        let cwd = project.to_string_lossy().to_string();
        let project_cwds = memory_test_cwds(&cwd, &["Claude"]);
        let sessions = scan_memory(&project_cwds).unwrap();
        let entry = sessions
            .iter()
            .find(|s| s.path == scoped.to_string_lossy())
            .expect(".claude/CLAUDE.md should be present");
        assert_eq!(entry.preview, "claude");
    }

    #[test]
    fn push_doc_files_recursive_skips_named_subdirs() {
        let dir = TestDir::new("skip-skills");
        let base = dir.path();
        fs::write(base.join("top.md"), "top body").unwrap();
        fs::create_dir_all(base.join("skills/skill-a")).unwrap();
        fs::write(base.join("skills/skill-a/SKILL.md"), "skill body").unwrap();
        fs::create_dir_all(base.join("rollout_summaries")).unwrap();
        fs::write(base.join("rollout_summaries/r1.md"), "summary").unwrap();

        let mut out = Vec::new();
        push_doc_files_recursive(
            base,
            base,
            "label",
            "codex",
            "Memory",
            &["skills"],
            &mut out,
        );
        let mut paths: Vec<String> = out.iter().map(|s| s.id.clone()).collect();
        paths.sort();
        assert_eq!(paths, vec!["rollout_summaries/r1.md", "top.md"]);
        for entry in &out {
            assert_eq!(entry.source, "Memory");
        }
    }

    #[test]
    fn push_doc_files_recursive_marks_source_skill() {
        let dir = TestDir::new("skill-source");
        let base = dir.path();
        fs::create_dir_all(base.join("git-workflow")).unwrap();
        fs::write(
            base.join("git-workflow/SKILL.md"),
            "---\nname: git-workflow\ndescription: a skill\n---\nbody",
        )
        .unwrap();

        let mut out = Vec::new();
        push_doc_files_recursive(
            base,
            base,
            "~/.claude/skills",
            "claude",
            "Skill",
            &[],
            &mut out,
        );
        assert_eq!(out.len(), 1);
        assert_eq!(out[0].source, "Skill");
        assert_eq!(out[0].preview, "claude");
        assert_eq!(out[0].project, "~/.claude/skills");
        assert_eq!(out[0].title, "git-workflow/SKILL.md");
    }

    #[test]
    fn scan_claude_skills_picks_up_project_skills_dir() {
        let dir = TestDir::new("claude-project-skills");
        let project = dir.path();
        let skills_dir = project.join(".claude").join("skills").join("my-skill");
        fs::create_dir_all(&skills_dir).unwrap();
        fs::write(skills_dir.join("SKILL.md"), "skill body").unwrap();

        let cwd = project.to_string_lossy().to_string();
        let project_cwds: HashSet<String> = [cwd.clone()].into_iter().collect();
        let sessions = scan_claude_skills(&project_cwds);

        let entry = sessions
            .iter()
            .find(|s| s.path.ends_with("my-skill/SKILL.md") && s.project == cwd)
            .expect("project-level Claude skill should be picked up");
        assert_eq!(entry.source, "Skill");
        // OpenCode also reads .claude/skills/, so the entry is tagged with both.
        assert_eq!(entry.preview, "claude,opencode");
    }

    #[test]
    fn scan_codex_skills_picks_up_project_skills_dir() {
        let dir = TestDir::new("codex-project-skills");
        let project = dir.path();
        let skills_dir = project.join(".codex").join("skills").join("dbg");
        fs::create_dir_all(&skills_dir).unwrap();
        fs::write(skills_dir.join("SKILL.md"), "skill body").unwrap();

        let cwd = project.to_string_lossy().to_string();
        let project_cwds: HashSet<String> = [cwd.clone()].into_iter().collect();
        let sessions = scan_codex_skills(&project_cwds);

        let entry = sessions
            .iter()
            .find(|s| s.path.ends_with("dbg/SKILL.md") && s.project == cwd)
            .expect("project-level Codex skill should be picked up");
        assert_eq!(entry.source, "Skill");
        assert_eq!(entry.preview, "codex");
    }

    #[test]
    fn scan_gemini_skills_picks_up_project_skills_dir() {
        let dir = TestDir::new("gemini-project-skills");
        let project = dir.path();
        let skills_dir = project.join(".gemini").join("skills").join("trace");
        fs::create_dir_all(&skills_dir).unwrap();
        fs::write(skills_dir.join("SKILL.md"), "skill body").unwrap();

        let cwd = project.to_string_lossy().to_string();
        let project_cwds: HashSet<String> = [cwd.clone()].into_iter().collect();
        let sessions = scan_gemini_skills(&project_cwds);

        let entry = sessions
            .iter()
            .find(|s| s.path.ends_with("trace/SKILL.md") && s.project == cwd)
            .expect("project-level Gemini skill should be picked up");
        assert_eq!(entry.source, "Skill");
        assert_eq!(entry.preview, "gemini");
    }

    #[test]
    fn scan_opencode_skills_picks_up_project_skills_dir() {
        let dir = TestDir::new("opencode-project-skills");
        let project = dir.path();
        let skills_dir = project.join(".opencode").join("skills").join("review");
        fs::create_dir_all(&skills_dir).unwrap();
        fs::write(skills_dir.join("SKILL.md"), "skill body").unwrap();

        let cwd = project.to_string_lossy().to_string();
        let project_cwds: HashSet<String> = [cwd.clone()].into_iter().collect();
        let sessions = scan_opencode_skills(&project_cwds);

        let entry = sessions
            .iter()
            .find(|s| s.path.ends_with("review/SKILL.md") && s.project == cwd)
            .expect("project-level OpenCode skill should be picked up");
        assert_eq!(entry.source, "Skill");
        assert_eq!(entry.preview, "opencode");
    }

    #[test]
    fn derive_memory_project_label_strips_dot_tool_wrapper_for_skill_paths() {
        let dir = TestDir::new("derive-project-skill");
        let project = dir.path();
        let path = project
            .join(".claude")
            .join("skills")
            .join("my-skill")
            .join("SKILL.md");
        fs::create_dir_all(path.parent().unwrap()).unwrap();
        fs::write(&path, "body").unwrap();
        let label = derive_memory_project_label(&path);
        assert_eq!(label, project.to_string_lossy());
    }

    #[test]
    fn scan_agents_skills_picks_up_project_dot_agents_skills_dir() {
        let dir = TestDir::new("agents-project-skills");
        let project = dir.path();
        let skills_dir = project.join(".agents").join("skills").join("debug-deploy");
        fs::create_dir_all(&skills_dir).unwrap();
        fs::write(skills_dir.join("SKILL.md"), "skill body").unwrap();

        let cwd = project.to_string_lossy().to_string();
        let project_cwds: HashSet<String> = [cwd.clone()].into_iter().collect();
        let sessions = scan_agents_skills(&project_cwds);

        let entry = sessions
            .iter()
            .find(|s| s.path.ends_with("debug-deploy/SKILL.md") && s.project == cwd)
            .expect("project-level .agents/skills/ entry should be picked up");
        assert_eq!(entry.source, "Skill");
        // Codex, Gemini CLI, and OpenCode all officially read this path.
        assert_eq!(entry.preview, "codex,gemini,opencode");
    }

    #[test]
    fn derive_memory_project_label_strips_dot_agents_wrapper_for_skill_paths() {
        let dir = TestDir::new("derive-project-agents-skill");
        let project = dir.path();
        let path = project
            .join(".agents")
            .join("skills")
            .join("my-skill")
            .join("SKILL.md");
        fs::create_dir_all(path.parent().unwrap()).unwrap();
        fs::write(&path, "body").unwrap();
        let label = derive_memory_project_label(&path);
        assert_eq!(label, project.to_string_lossy());
    }

    #[test]
    fn scan_claude_rules_picks_up_project_rules_dir_recursively() {
        let dir = TestDir::new("claude-project-rules");
        let project = dir.path();
        let rules_dir = project.join(".claude").join("rules");
        let nested = rules_dir.join("topics");
        fs::create_dir_all(&nested).unwrap();
        fs::write(rules_dir.join("style.md"), "be terse").unwrap();
        fs::write(nested.join("ts.md"), "no any").unwrap();

        let cwd = project.to_string_lossy().to_string();
        let project_cwds: HashSet<String> = [cwd.clone()].into_iter().collect();
        let sessions = scan_claude_rules(&project_cwds);

        let style = sessions
            .iter()
            .find(|s| s.path.ends_with("style.md") && s.project == cwd)
            .expect("project-level rule style.md should be picked up");
        assert_eq!(style.source, "Memory");
        assert_eq!(style.preview, "claude");

        let nested_entry = sessions
            .iter()
            .find(|s| s.path.ends_with("topics/ts.md") && s.project == cwd)
            .expect("nested rule ts.md should be picked up recursively");
        assert_eq!(nested_entry.source, "Memory");
        assert_eq!(nested_entry.preview, "claude");
    }

    #[test]
    fn derive_memory_project_label_handles_claude_rules_under_project() {
        let dir = TestDir::new("derive-project-rules");
        let project = dir.path();
        let path = project.join(".claude").join("rules").join("style.md");
        fs::create_dir_all(path.parent().unwrap()).unwrap();
        fs::write(&path, "body").unwrap();
        let label = derive_memory_project_label(&path);
        assert_eq!(label, project.to_string_lossy());
    }

    #[test]
    fn scan_memory_walks_up_ancestors_to_git_root() {
        let dir = TestDir::new("ancestor-walk");
        let root = dir.path();
        // Layout: <root>/.git, <root>/AGENTS.md, <root>/sub/, <root>/sub/CLAUDE.md
        fs::create_dir_all(root.join(".git")).unwrap();
        fs::write(root.join("AGENTS.md"), "root agents").unwrap();
        let sub = root.join("sub");
        fs::create_dir_all(&sub).unwrap();
        fs::write(sub.join("CLAUDE.md"), "sub claude").unwrap();

        // cwd is the deeper subdir; walk up should find the root AGENTS.md.
        let cwd = sub.to_string_lossy().to_string();
        let project_cwds: HashSet<String> = [cwd.clone()].into_iter().collect();
        let sessions = scan_memory(&project_cwds).unwrap();

        let root_agents_path = root.join("AGENTS.md").to_string_lossy().to_string();
        let root_agents = sessions
            .iter()
            .find(|s| s.path == root_agents_path)
            .expect("ancestor AGENTS.md should be picked up");
        assert_eq!(root_agents.preview, "codex,opencode");
        // The ancestor entry's project label should be the ancestor dir itself,
        // not the deeper cwd.
        assert_eq!(root_agents.project, root.to_string_lossy());

        let sub_claude_path = sub.join("CLAUDE.md").to_string_lossy().to_string();
        let sub_claude = sessions
            .iter()
            .find(|s| s.path == sub_claude_path)
            .expect("cwd CLAUDE.md should still be picked up");
        assert_eq!(sub_claude.preview, "claude,opencode");
        assert_eq!(sub_claude.project, cwd);
    }

    #[test]
    fn scan_memory_no_git_means_cwd_only() {
        let dir = TestDir::new("no-git");
        let root = dir.path();
        // Layout: <root>/AGENTS.md (in cwd) and <root>/sub/AGENTS.md.
        // No .git anywhere. Expect: only the deeper cwd's file is scanned,
        // not its parent's, since the source code in Codex/Gemini/OpenCode
        // refuses to ascend when no project-root marker is found.
        fs::write(root.join("AGENTS.md"), "parent agents").unwrap();
        let sub = root.join("sub");
        fs::create_dir_all(&sub).unwrap();
        fs::write(sub.join("AGENTS.md"), "sub agents").unwrap();

        let cwd = sub.to_string_lossy().to_string();
        let project_cwds: HashSet<String> = [cwd.clone()].into_iter().collect();
        let sessions = scan_memory(&project_cwds).unwrap();

        let sub_path = sub.join("AGENTS.md").to_string_lossy().to_string();
        assert!(
            sessions.iter().any(|s| s.path == sub_path),
            "cwd-level AGENTS.md should be picked up"
        );
        let parent_path = root.join("AGENTS.md").to_string_lossy().to_string();
        assert!(
            sessions.iter().all(|s| s.path != parent_path),
            "without .git the walk must not ascend to the parent dir"
        );
    }

    #[test]
    fn scan_memory_walk_stops_at_git_root() {
        let dir = TestDir::new("ancestor-walk-stop");
        let root = dir.path();
        // The git root is the cwd itself; we must NOT ascend above it.
        fs::create_dir_all(root.join(".git")).unwrap();
        fs::write(root.join("AGENTS.md"), "root agents").unwrap();
        // Create a file outside the git root that would falsely match if the
        // walk ascended too far. Place it in a parent dir of `root`.
        let above_marker = root.parent().unwrap().join("AGENTS.md");
        // Best effort — if the test runner's temp parent already has this file
        // we still validate the include/exclude logic via path checks below.
        let _ = fs::write(&above_marker, "outside");

        let cwd = root.to_string_lossy().to_string();
        let project_cwds: HashSet<String> = [cwd.clone()].into_iter().collect();
        let sessions = scan_memory(&project_cwds).unwrap();

        let inside_path = root.join("AGENTS.md").to_string_lossy().to_string();
        assert!(
            sessions.iter().any(|s| s.path == inside_path),
            "git root AGENTS.md should be present"
        );
        let outside_path = above_marker.to_string_lossy().to_string();
        assert!(
            sessions.iter().all(|s| s.path != outside_path),
            "ancestor walk must not ascend above the git root"
        );

        // Clean up the polluted parent file so we don't bleed into other tests.
        let _ = fs::remove_file(&above_marker);
    }

    #[test]
    fn parse_doc_file_emits_skill_kind_for_skill_source() {
        let dir = TestDir::new("parse-skill");
        let path = dir.path().join("SKILL.md");
        fs::write(
            &path,
            "---\nname: example\ndescription: a skill\n---\nbody text",
        )
        .unwrap();
        let detail = parse_doc_file(&path, "Skill").unwrap();
        assert_eq!(detail.session.source, "Skill");
        assert_eq!(detail.messages.len(), 1);
        assert_eq!(detail.messages[0].kind, "skill");
        assert_eq!(detail.messages[0].text, "body text");
    }
}
