# CLAUDE.md

## Scope

This file is for Claude Code working in this repository. Keep it limited to current codebase facts, implementation rules, and verification steps.

## Current App

Tauri v2 desktop app for browsing local history records from supported AI tool clients, plus their on-disk memory and skill files.

Supported sources in the current code:

- Codex
- Claude Code
- Gemini CLI
- OpenCode

Top-level UI panes:

- **Sessions** — chat/transcript history per tool
- **Memories** — on-disk memory files (CLAUDE.md, AGENTS.md, GEMINI.md, `~/.codex/memories/`, `~/.claude/projects/<slug>/memory/`, etc.)
- **Skills** — `SKILL.md` files under each tool's skills directory plus the cross-tool `.agents/skills/` location

Current alignment target: data acquisition and message preview formatting should follow the official tools. UI layout, source filters, project grouping, search, stats, cross-source sorting, and the Memory/Skills views are app behavior and do not need official UI parity.

## Tech Stack

- Desktop framework: Tauri v2
- Frontend: React 18, TypeScript, Vite
- Backend: Rust 2021
- Database access: `rusqlite` with bundled SQLite
- JSON parsing: `serde`, `serde_json`
- Filesystem scanning: `walkdir`, `dirs`
- Time handling: `chrono`
- Icons: `lucide-react` plus inline SVG brand icons

## Code Map

- Frontend: `src/main.tsx`
- Styles: `src/styles.css`
- Tauri IPC commands: `src-tauri/src/lib.rs`
- Session/Memory/Skill scanning, parsing, and formatting: `src-tauri/src/sessions.rs`
- Tauri config: `src-tauri/tauri.conf.json`
- Rust parser/formatter tests: inline tests at the bottom of `src-tauri/src/sessions.rs`

Current Tauri IPC commands, called from the frontend with `invoke(...)`:

- `scan_all_sessions` — returns Sessions + Memory + Skill entries as `AppSession[]`
- `load_session` — loads one entry by `{ source, path, id }`
- `search_all_sessions` — substring search across all loaded session/memory/skill bodies

## Project Commands

- Package manager: npm (`package-lock.json` is present).
- Web dev server: `npm run dev`
- Tauri dev app: `npm run tauri:dev`
- Frontend production build: `npm run build`
- Tauri bundle build: `npm run tauri:build`
- Rust tests: `cargo test --manifest-path src-tauri/Cargo.toml --lib`
- Rust format: `cd src-tauri && cargo fmt`

The Tauri binary is renamed via `[[bin]] name = "Termory"` in `src-tauri/Cargo.toml` plus `mainBinaryName: "Termory"` in `tauri.conf.json`, so the macOS menu bar shows "Termory" rather than the lowercase Cargo package name.

macOS bundle identifier: `is.chats.termory` (reverse DNS of the `chats.is` domain the project ships under). Do NOT change this after a public release — macOS treats a different identifier as a different app, so existing user data and the Tauri updater would break.

The repo also contains `.audit-sources/` (gitignored) with shallow clones of `openai/codex`, `google-gemini/gemini-cli`, `sst/opencode`, and `videcoding/cli` (legacy Claude Code reference). This is the source-of-truth for path/behavior verification when official docs disagree with implementation — grep here instead of WebFetching docs.

## Upstream References

Use upstream implementations as the reference for history data and message preview behavior:

- Codex official source: https://github.com/openai/codex
- Claude Code referenced CLI implementation: https://github.com/videcoding/cli
- Gemini CLI official source: https://github.com/google-gemini/gemini-cli
- OpenCode official source: https://github.com/anomalyco/opencode

For Memory paths:

- Claude Code memory: https://code.claude.com/docs/en/memory
- Codex AGENTS.md guide: https://developers.openai.com/codex/guides/agents-md
- Codex memories: https://developers.openai.com/codex/memories
- Gemini GEMINI.md: https://github.com/google-gemini/gemini-cli/blob/main/docs/cli/gemini-md.md
- Gemini memory tool: https://github.com/google-gemini/gemini-cli/blob/main/docs/tools/memory.md
- OpenCode rules: https://opencode.ai/docs/rules/

For Skills paths:

- Codex skills docs: https://developers.openai.com/codex/skills
- Gemini CLI skills docs: https://github.com/google-gemini/gemini-cli/blob/main/docs/cli/skills.md
- OpenCode skills docs: https://opencode.ai/docs/skills/

For TUI tool-message rendering (every Termory branch cites a line here):

- Codex exec/shell render: `.audit-sources/codex/codex-rs/tui/src/exec_cell/render.rs`, bash highlight alias at `codex-rs/tui/src/render/highlight.rs:533`
- Claude tool-use wrapper: `.audit-sources/claude-code/src/components/messages/AssistantToolUseMessage.tsx:152` (assembles `<bold>{userFacingName}</bold>({renderToolUseMessage})`); per-tool: `src/tools/<Tool>/UI.tsx` (`userFacingName` + `renderToolUseMessage`)
- Gemini ToolInfo render: `.audit-sources/gemini-cli/packages/cli/src/ui/components/messages/ToolShared.tsx:202`; type at `packages/cli/src/ui/types.ts:119` `IndividualToolCallDisplay`
- OpenCode tool components: `.audit-sources/opencode/packages/opencode/src/cli/cmd/tui/feature-plugins/system/session-v2.tsx` (Bash l.707, Glob l.748, Read l.764, Grep l.794, WebFetch l.810, WebSearch l.818, Write l.828, Edit l.857, ApplyPatch l.891, TodoWrite l.964, Question l.991, Skill l.1022, Task l.1030, generic l.522, BlockTool helper l.659, InlineTool helper l.559)

When behavior differs by version, match the locally installed or explicitly requested target version and cover it with a focused test. Tool-message rendering should reference the TUI source files above, not the doc sites — docs lag behind the actual UI for many of these tools.

## Current Data Sources

### Sessions

- Codex
  - List: `~/.codex/state_5.sqlite`, table `threads`, rows where `archived = 0` AND `preview <> ''` AND `source IN ('cli', 'vscode', 'atlas', 'chatgpt')`. The four sources match `INTERACTIVE_SESSION_SOURCES` in `codex-rs/rollout/src/lib.rs`; the `preview <> ''` clause matches `push_thread_filters` in `codex-rs/state/src/runtime/threads.rs`. Same filter is applied when loading a single thread by id.
  - Messages: each selected thread's `threads.rollout_path` JSONL file.
- Claude Code
  - List: `CLAUDE_CONFIG_DIR/projects/**/*.jsonl` when `CLAUDE_CONFIG_DIR` is set, otherwise `~/.claude/projects/**/*.jsonl`. Filename must be a UUID (`is_uuid_like`), first line must NOT contain `"isSidechain":true`, and the session must have at least one of customTitle/aiTitle/lastPrompt/summary/firstPrompt — same filter as videcoding/cli `parseSessionInfoFromLite`.
  - Messages: the selected project JSONL file.
- Gemini CLI
  - List: `~/.gemini/tmp/*/chats/session-*.jsonl` and `~/.gemini/tmp/*/chats/session-*.json`. Sessions must have a non-empty `sessionId`, `hasUserOrAssistantMessage`, and `kind !== 'subagent'`. When `startTime`/`lastUpdated` are missing on the record, Termory falls back to the file's mtime (then to `Utc::now()`) — mirrors `getAllSessionFiles` in `packages/cli/src/utils/sessionUtils.ts`.
  - Project path: sibling/related `.project_root` file under the Gemini temp project directory.
  - Messages: the selected session JSONL/JSON chat file.
- OpenCode
  - List: `~/.local/share/opencode/opencode.db`, table `session`, rows where `parent_id IS NULL` and `time_archived IS NULL`, ordered by `time_updated DESC, id DESC`. Mirrors `listByProject` in `packages/opencode/src/session/session.ts`.
  - Messages: `~/.local/share/opencode/opencode.db`, tables `message` and `part`; `session_message` is only a fallback when `message`/`part` are unavailable (a real compat path for older databases — `session_message` is otherwise the projections table per `projectors-next.ts`).
  - Compatibility storage: JSON files under `~/.local/share/opencode/**/storage`; use only for older/alternate local layouts and never before the current SQLite path.

Read source history in place. Do not modify original history files or databases.

### Memory

Verified against each tool's open-source implementation (not just docs). When docs and source disagree, source is authoritative. See Upstream References for source URLs.

- Claude Code: `~/.claude/projects/<sanitized-canonical-git-root>/memory/**/*.md` (auto-memory per project — `src/memdir/paths.ts` uses `findCanonicalGitRoot` so worktrees of the same repo share one dir), `~/.claude/rules/**/*.md` (global rules), `<cwd>/.claude/rules/**/*.md` (project rules — both recursive, all `.md`)
- Codex: `~/.codex/memories/**/*.md` — `scan_codex_memory` skips the `skills/` subdir for backward compatibility (current Codex source stores skills at `~/.codex/skills/`)
- Gemini CLI:
  - Global: `~/.gemini/GEMINI.md` (legacy) AND `~/.gemini/MEMORY.md` (modern alias — `getAllGeminiMdFilenames()` returns both)
  - Per-project: `~/.gemini/tmp/<id>/memory/{MEMORY.md preferred, GEMINI.md legacy}` — confirmed at `packages/core/src/config/storage.ts getProjectMemoryDir()` → `getProjectMemoryTempDir() = path.join(globalTempDir, projectIdentifier, 'memory')`. Termory recursively reads .md inside, skipping the `skills/` subdir which is surfaced under Skills.
- Per-project instruction files — scanned at the cwd AND, **only when a `.git` directory exists at or above cwd**, every ancestor up to and including the git root (stopping before `$HOME`):
  - `CLAUDE.md` → tag `claude,opencode` (OpenCode officially falls back to it)
  - `CLAUDE.local.md` → tag `claude`
  - `AGENTS.md` → tag `codex,opencode`
  - `AGENTS.override.md` → tag `codex` (Codex's official override file)
  - `GEMINI.md` → tag `gemini`
  - `MEMORY.md` → tag `gemini`
  - `<cwd>/.claude/CLAUDE.md` → tag `claude` (only at cwd, not at ancestors — `.claude/CLAUDE.md` is a project-root convention)
- Global instruction files:
  - `~/.claude/CLAUDE.md` → tag `claude,opencode`
  - `~/.codex/AGENTS.md`, `~/.codex/AGENTS.override.md` → tag `codex`
  - `~/.config/opencode/AGENTS.md` → tag `opencode`

Paths intentionally NOT scanned (no current source reads them):

- `AGENTS.local.md` (any location) — not in any tool's source; Codex uses `AGENTS.override.md`
- `~/.codex/instructions.md` — legacy
- `~/.claude/CLAUDE.local.md` — not documented at user level
- `CONTEXT.md` — OpenCode deprecated, intentionally skipped
- `project_doc_fallback_filenames` from `~/.codex/config.toml` — Termory does not read user config

### Why ancestor walk gates on `.git`

All three open-source tools refuse to ascend without a project-root marker:

- **Codex** (`codex-rs/core/src/agents_md.rs`): `DEFAULT_PROJECT_ROOT_MARKERS = &[".git"]`. The doc-comment on the loader: *"If no marker is found, only the current working directory is considered."*
- **Gemini** (`packages/core/src/utils/memoryDiscovery.ts findProjectRoot`): defaults to `['.git']`. When no marker is found, returns null → caller sets `ceiling = startDir` → `findUpwardGeminiFiles` breaks immediately on the start dir.
- **OpenCode** (`packages/opencode/src/project/project.ts`): `worktree` is resolved via `git rev-parse --git-common-dir`; without git the fallback sets `worktree: sandbox` (= cwd), so `Filesystem.findUp(start=cwd, stop=cwd)` collects only cwd.

Claude Code (the only one NOT gating on `.git` — its `attachments.ts` walks to fs root) is the outlier; for simplicity we apply the stricter (more common) rule. This is a known minor mismatch documented in [`codex-ancestor-walk-rule`](memory).

The implementation lives in `scan_memory`:

1. `push_project_root_instruction_files(cwd, ...)` always runs (cwd-level files including `.claude/CLAUDE.md`).
2. `find_git_root(cwd, home)` walks up looking for `.git`. Returns `Some(dir)` or `None`.
3. If `Some(git_root)` and `git_root != cwd`, walk from `cwd.parent()` up to and including `git_root`, calling `push_ancestor_instruction_files` at each level (omits `.claude/CLAUDE.md`).
4. The final dedup-by-path keeps each file's first label; ancestor files get their own ancestor dir as the project label.

### Skills

Source-verified locations:

| Tool | Global | Project | Tag |
|---|---|---|---|
| Claude Code | `~/.claude/skills/` | `<cwd>/.claude/skills/` | `claude,opencode` (OpenCode officially also reads `.claude/skills/`) |
| Codex | `~/.codex/skills/` (NOT `~/.codex/memories/skills/`) | `<cwd>/.codex/skills/` | `codex` |
| Gemini CLI | `~/.gemini/skills/` (`Storage.getUserSkillsDir`) | `~/.gemini/tmp/<id>/memory/skills/` (`Storage.getProjectSkillsMemoryDir`) + `<cwd>/.gemini/skills/` | `gemini` |
| OpenCode | `~/.config/opencode/skills/` | `<cwd>/.opencode/skills/` | `opencode` |
| Tool-neutral | `~/.agents/skills/` (`Storage.getUserAgentSkillsDir`) | `<cwd>/.agents/skills/` | `codex,gemini,opencode` (officially supported by all three) |

Implementation notes:

- All skill scanners route through `push_doc_files_recursive(dir, base, project, tag, source="Skill", skip_dirs=&[], out)`.
- `doc_session_from_file(path, project, source)` is shared between Memory and Skill scanners; the `source` field on `AppSession` is `"Memory"` or `"Skill"` accordingly.
- `parse_doc_file(path, source)` handles loading either kind in `get_session`.
- `derive_memory_project_label` recognizes both memory and skill on-disk paths (including `.agents/skills/`) so loading a single file by absolute path produces a sensible project label.

## Current Implementation Notes

- `scan_all_sessions` calls Rust scanning on a blocking worker and returns sessions, memories, and skills in one list (distinguished by `source`).
- `load_session` loads one selected record by `source`, `path`, and `id`.
- `AppSession.preview` carries comma-separated tool tags (e.g. `"codex,opencode"` for AGENTS.md). The list-card `MemoryCard` renders one brand badge per tag via `memoryToolsOf()`; the detail-header badge renders a single type label (`Session` / `Memory` / `Skill`) via `typeLabelOf()`.
- For session-type entries the detail header shows the GUID (`selected.id`) on its own line below the project path, styled monospace via `.detailGuid`. Memory/Skill entries omit the GUID line.
- Project-level `AGENTS.md` and `AGENTS.override.md` are always tagged with both `codex` and `opencode` regardless of which tool actually has sessions in the cwd. Rationale: the AGENTS.md spec is tool-neutral — Termory reports which tools CAN read the file, not which tool happened to run there. Verified by `scan_memory_always_tags_project_agents_md_with_both_codex_and_opencode` test.
- Sidebar source filter (Codex/Claude/Gemini/OpenCode/All) applies to **all three** panes (Sessions, Memory, Skills). Memory and Skill filtering goes through `memoryToolsOf(item).includes(source as MemoryTool)`, so multi-tagged files (AGENTS.md with `codex,opencode`) appear under both Codex and OpenCode filters.
- Session list cards currently show source, date, title, project, and message count.
- `message_count` is an app-derived visible parsed message count when the official list does not provide the same count directly.
- Empty or missing official titles should stay empty unless the official tool has the same fallback.
- Badge colors live in `src/styles.css` and split into two families:
  - **Brand badges** (list cards, one per tool tag) match each tool's logo:
    - `.badge.codex` `#0E0E10` (OpenAI black)
    - `.badge.claude` `#CC785C` (Anthropic Clay)
    - `.badge.gemini` `linear-gradient(135deg, #4285F4 → #A142F4 → #34A853)` (Google blue/purple/green)
    - `.badge.opencode` `#374151` (slate, distinct from Codex black)
  - **Type badges** (detail header, one per entry by type — semantic, not brand-derived). All three pills sit at the Tailwind `600` saturation level so they feel visually balanced:
    - `.badge.session` `#0284C7` (sky 600 — conversation/information)
    - `.badge.memory` `#9333EA` (purple 600 — knowledge/recall)
    - `.badge.skill` `#059669` (emerald 600 — capability/action)

### Unified tool-message format

Every tool message — regardless of source platform — funnels into the same markdown shape so the detail pane reads consistently across Codex / Claude / Gemini / OpenCode.

**Shape:**

```
{status} **{Verb}**({args})

````
{body — may include `Error: ...` prefix on failures}
````
```

- `{status}` is the leading glyph: `⏺` on success, `✗` on failure. Inserted by `merge_tool_outputs` (for paired tool_use/tool_result) or by the tool emitter directly (for self-contained EventMsg-derived cards like MCP / WebSearch). Matches Claude TUI's BLACK_CIRCLE prefix in `constants/figures.ts:4` (`⏺` on darwin, `●` elsewhere) for success; `✗` mirrors Codex's failure badge in `exec_cell/render.rs:236`.
- `{Verb}` and `{args}` are platform-native (Codex `**Ran**`, Claude `**Bash**`, etc.) — "各家用各家" per design call. The badge on each session list card already disambiguates the platform.
- `{args}` content (commands, paths, URLs, patterns) is wrapped with `wrap_inline_code` (sessions.rs:48 — CommonMark §6.1 delimiter sizing) so embedded backticks / asterisks / parens don't escape into markdown formatting.
- `Error:` prefix appears INSIDE the fence body on failures, replacing the old footer-style `**Error**` line. Format is `Error: Exit code N` for Codex (exit code parsed from the `Process exited with code N` wrapper line) or `Error: {message}` for Claude (`is_error: true` with no exit code).
- Reasoning across all four platforms goes through `format_reasoning_body(content)` → `> *line*\n> *line*\n...` (italic blockquote) so the visual style is consistent.

**Failure detection per platform** (`SessionMessage.exit_code: Option<i64>` carries the parsed value through `merge_tool_outputs`):

| Platform | Signal source | Notes |
|---|---|---|
| Codex | `Process exited with code N` / `Exit code: N` in the `function_call_output.output` wrapper (`ExecCommandToolOutput.response_text()` — context.rs:409) parsed by `codex_parse_exec_output` | Limited mode default; populates `exit_code` |
| Claude | `tool_result.is_error: true` content block | No exit code field — `Error:` prefix has no `Exit code N` part |
| OpenCode | `tool.error` / failed step state | Wired through `merge_tool_outputs` is_error flag |
| Gemini | `result.error` on `toolCalls[]` entries | Optional; absent on most calls |

### Per-platform verb mapping

Every Termory branch cites the exact source file that produces the verb in each TUI. Survey under `.audit-sources/{codex,gemini-cli,opencode,claude-code}/`.

**Codex** (`codex_function_call_message`) — `.audit-sources/codex/codex-rs/tui/src/exec_cell/render.rs:381-385`:

- `exec_command` / `shell` / `shell_command` / `local_shell` (all 4 names per `rollout-trace/src/tool_dispatch.rs:263`) → `**Ran** {wrap_inline_code(cmd)}`
- `apply_patch` → `**{Verb}** {path}\n\n```diff\n{patch}\n```` ` — `codex_parse_patch_actions` scans `*** Add File:` / `*** Delete File:` / `*** Update File:` markers, picks `Added` / `Deleted` / `Edited` per `diff_render.rs:421-436`. Multi-file patches collapse to `**Edited** N files`.
- `update_plan` → `**Updated plan**` + optional `*explanation*` + GFM task list `- [x]/[~]/[ ]` (matches PlanUpdateCell at `history_cell/plans.rs:138-194`)
- `view_image` → `**Viewed image** {wrap_inline_code(path)}` (patches.rs:63-72)
- other → `{name}({compact args})` fallback

**Codex EventMsg dispatch** (`codex_event_msg_to_message`) — `RolloutItem::EventMsg` records are the canonical replay source for Codex; the wrapper `codex_message_from_value` routes `event_msg` records here. Handled variants:

- `user_message` / `agent_message` / `agent_reasoning` / `agent_reasoning_raw_content`
- `web_search_end` → `**Searched** {wrap_inline_code(query)}`
- `mcp_tool_call_end` → `**MCP** {server}/{tool}` + 4-backtick result fence
- `image_generation_end` → `**Generated image** {prompt}` + saved path
- `view_image_tool_call` → same shape as the function_call variant
- `plan_update` → same as the function_call `update_plan` (payload IS the UpdatePlanArgs)
- `patch_apply_end` (Extended mode) → per-file `**Verb** {path}` lines; on failure appends stderr fence + `**Error**`
- `context_compacted` → `*Context compacted*` system notice
- `error` → `**Error**: {message}` system notice
- `turn_aborted` → `*Turn interrupted by user*` / `*Turn stopped — budget limit reached*`
- `thread_rolled_back` → `*Rolled back N turn(s)*`
- `entered_review_mode` / `exited_review_mode` → italic notices

`exec_command_end` (Extended-mode shell) is intentionally NOT dispatched yet — it would duplicate the ResponseItem-derived card. Need call_id-based dedup before enabling.

`Limited` vs `Extended` mode (per `codex-rs/rollout/src/policy.rs:135-153`): the CLI default is Limited (`tui/src/app_server_session.rs: persist_extended_history: false`), so most rollouts only carry `ResponseItem::FunctionCall` + `FunctionCallOutput` for shell tools — NOT `EventMsg::ExecCommandEnd`. Termory's `codex_function_call_output_message` is the authoritative path for shell output in that mode; `codex_parse_exec_output` strips the wrapper to recover `aggregated_output`.

**Claude Code** (`claude_tool_use_text`) — `.audit-sources/claude-code/src/components/messages/AssistantToolUseMessage.tsx:152` wraps `<bold>{userFacingName}</bold>({renderToolUseMessage})`. Each Tool's `UI.tsx` provides both pieces. All argument values pass through `wrap_inline_code` so markdown-special chars in user payloads can't leak:

| Raw name | `userFacingName` | Termory output |
|---|---|---|
| `Bash` | BashTool/UI.tsx | `**Bash**({command})` (empty cmd → just `**Bash**`) |
| `Read` / `View` | FileReadTool/UI.tsx:8 → "Read" | `**Read**({path} · lines X-Y / · pages N / · limit N)` |
| `Write` | FileWriteTool/UI.tsx → "Write" | `**Write**({path})` |
| `Edit` / `MultiEdit` / `str_replace*` | FileEditTool/UI.tsx → "Update" | `**Update**({path})` |
| `Grep` | GrepTool.ts:170 → "Search" | `**Search**(pattern: ..., path: ...)` |
| `Glob` | GlobTool/UI.tsx:13 → "Search" | `**Search**(pattern: ..., path: ...)` |
| `WebFetch` | WebFetchTool.ts:81 → "Fetch" | `**Fetch**({url})` |
| `WebSearch` | WebSearchTool.ts:160 → "Web Search" (space) | `**Web Search**({query})` |
| `TodoWrite` | (Termory uses GFM task-list) | `**TodoWrite**\n\n- [x]/[~]/[ ] ...` |
| `Task` | TaskCreateTool/etc. | `**Task**({description}, agent: ...)` |
| `NotebookEdit` | NotebookEditTool.ts → "Edit Notebook" | `**Edit Notebook**({notebook_path})` |
| `ExitPlanMode` / `EnterPlanMode` | empty userFacingName (suppressed in TUI) | `**Exit/Enter Plan Mode**` minimal marker |
| `AskUserQuestion` | empty userFacingName | `**Ask** {question}` (one) / `**Ask** (N questions)` |
| `ReadMcpResource` | ReadMcpResourceTool/UI.tsx → "readMcpResource" | `**Read MCP Resource**({uri})` |
| `ListMcpResources` | "listMcpResources" | `**List MCP Resources**({server})` |
| `McpAuth` | "{server} - authenticate (MCP)" | `**MCP Auth** {server}` |
| `mcp__{server}__{tool}` (generic MCP) | — | `**MCP** {server}/{tool}` (same shape as Codex MCP) |

**Claude content blocks** beyond `text` / `tool_use`:

- `thinking` and `redacted_thinking` → reasoning message via `claude_thinking_blocks` + `format_reasoning_body`. Claude TUI renders `∴ Thinking…` (AssistantThinkingMessage.tsx); Termory emits the unified `> *content*` blockquote instead.
- `image` (`{source: {type: "base64"|"url", media_type, ...}}`) → italic `*Image ({mime})*` or `*Image: {url}*` notice via `claude_image_part_label`.
- `tool_result.content` may be `Value::String` or `Value::Array` of `text` blocks. The `<tool_use_error>...</tool_use_error>` wrapper Anthropic adds for internal validation failures is stripped by `claude_display_text` so only the human-readable error text remains.

**OpenCode** (`opencode_v2_tool_part_text`) — every branch cites a line in `.audit-sources/opencode/packages/opencode/src/cli/cmd/tui/feature-plugins/system/session-v2.tsx`:

- `Bash` (l.707): with output → `\# {description ?? "Shell"}\n\n```bash\n$ {command}\n{output}\n```` (BlockTool); without output → `$ {command}` (InlineTool). The `\#` is an escaped `#` so markdown does not interpret the title as an H1 — the CLI's literal title string starts with `# `.
- `Glob` (l.748): `Glob "{pattern}" in {path} ({N} match[es])` (singular/plural matched)
- `Read` (l.764): `Read {filePath} [other=...]\n↳ Loaded {path}` per loaded entry
- `Grep` (l.794): `Grep "{pattern}" in {path} ({N} match[es])`
- `WebFetch` (l.810): `WebFetch {url}`
- `WebSearch` (l.818): `WebSearch "{query}" ({N} results)`
- `Write` (l.828): `\# Wrote {filePath}\n\n```{lang}\n{content}\n```` block when completed, else `Write {filePath}` inline
- `Edit` (l.857): `← Edit {filePath}\n\n```diff\n{diff}\n```` when diff present (title starts with `←`, no `#`)
- `ApplyPatch` (l.891): per-file title — `\# Deleted {path}` / `\# Created {path}` / `\# Moved {old} → {new}` / `← Patched {path}` + ```diff fence
- `TodoWrite` (l.964): `\# Todos\n\n{✓/~/✕/☐} {content}` per todo (icons match `todoIcon` helper)
- `Question` (l.991): `\# Questions\n\n{Q}\n{A}` per Q/A pair
- `Skill` (l.1022): `Skill "{name}"`
- `Task` (l.1030): `{Titlecase(subagent_type ?? "General")} Task — {description}`
- generic (l.522): `{name} {input}` inline, or `**{name} {input}**\n\n```\n{output}\n```` block
- `reasoning` part → `format_reasoning_body` (unified italic blockquote — replaces the old `_Thinking:_` inline prefix)

Audit reference is OpenCode `1.15.5` (commit `9324ef0`). Compared against `v1.15.7`: only cosmetic reasoning collapse-icon change in session-v2.tsx (`▼/▶` → `-/+`), no structural / schema diffs. No re-audit needed.

**Gemini CLI** (`gemini_tool_messages_from_value` + `gemini_thought_messages_from_value` + `gemini_part_to_string`) — `.audit-sources/gemini-cli/packages/cli/src/ui/components/messages/`:

- `toolCalls[]` entries (ToolShared.tsx:202 `ToolInfo`) → `**{displayName}** {description}` + 4-backtick `resultDisplay` fence
- `thoughts[{subject, description}]` array (ThinkingMessage.tsx:22 `normalizeThoughtLines`) → one reasoning message per entry via `format_reasoning_body`. Noise filtering matches the source (skip whitespace-only or `...` runs)
- Parts with `executableCode: {code, language}` → ```{lang}\n{code}\n``` fence
- Parts with `codeExecutionResult: {outcome, output}` → 4-backtick output fence + italic `*Outcome: OUTCOME_FAILED*` footer when non-OK
- Parts with `inlineData: {mimeType, ...}` → `*Inline data ({mime})*` italic notice
- Parts with `fileData: {fileUri}` → `*File: {uri}*` italic notice
- Parts with `functionCall: {name}` → `*Tool call: {name}*` (inline marker; the structured card comes from `toolCalls[]`)
- Parts with `functionResponse: {name}` → `*Tool response: {name}*`

### Helpers used across all four platforms

- `wrap_inline_code(content)` (sessions.rs:48) — CommonMark §6.1: pick a backtick delimiter longer than the longest run inside the content; pad with spaces when content starts or ends with a backtick. Used everywhere an unsafe user payload (path, command, URL, query, pattern) becomes inline `\`code\`` in markdown.
- `format_reasoning_body(content)` (sessions.rs:71) — line-by-line `> *...*` italic blockquote, escapes stray `*` / `_` so italic spans can't break mid-line.
- `merge_tool_outputs(messages)` (sessions.rs runs in `parse_claude_session` and `parse_codex_session`): folds matching `tool_result` / `tool_error` into the leading `tool_use` card. On a matched failure it prefixes the leading line with `✗ ` (instead of `⏺ `) and prepends the fence body with `Error:` (plus `Exit code N` when `SessionMessage.exit_code` is set). Orphan results (no matching tool_use) keep their text but also get a `⏺` / `✗` status prefix.
- `codex_parse_exec_output(text)` returns `CodexExecOutput { raw, exit_code }` — strips Codex's `Chunk ID: ... Output:` wrapper line-by-line so the visible body is just `aggregated_output`, AND extracts the exit code for the `Error: Exit code N` line.
- `codex_parse_patch_actions(patch_text)` scans `*** Add/Delete/Update File:` markers and returns `Vec<CodexPatchAction>` for the apply_patch header builder.

### Tool message metadata + UI

- `SessionMessage` carries two `#[serde(skip)]` fields used only during parsing/merging:
  - `tool_use_id: Option<String>` — links `tool_use` ↔ `tool_result` by provider id (Claude `tool_use.id` / Codex `function_call.call_id`).
  - `exit_code: Option<i64>` — Codex shell exit code parsed from `function_call_output` metadata; surfaced in the `Error: Exit code N` fence line.
- Provider-native combined formats (OpenCode parts, Gemini toolCalls, Codex EventMsg-derived cards) skip `merge_tool_outputs` — they already arrive complete with their own fence and add the `⏺` / `✗` prefix at emission time.

### Markdown rendering (frontend)

- The detail-pane body uses `react-markdown` + `remark-gfm` (tables / task lists / strikethrough). No syntax-highlight pass: code blocks render as plain monospace until a per-language renderer is added intentionally.
- No DOMPurify / rehype-sanitize: react-markdown emits React elements (not HTML strings), so raw `<tag>` in session content is auto-escaped by React's text node rendering and displays as literal text — same characters the CLI shows.
- No raw / rendered toggle and no `viewMode` state — every message renders through the same react-markdown pipeline. The "open original file" affordance in the detail header still lets the user inspect the underlying JSONL / db row outside Termory.
- Inline `<code>` carries `word-break: break-all` so a long no-space path inside `**Read**(\`/very/long/path\`)` wraps with the surrounding paragraph instead of overflowing the message card.
- Unordered lists render with the `- ` text marker via `list-style-type: "- "` (matching Codex TUI's `start_item` output at `codex-rs/tui/src/markdown_render.rs:754-760`).

## History and Preview Behavior

- Session lists should come from the same stored records the official tool uses for its history/resume list.
- Session list fields should use official values when available: title, project/cwd, timestamps, source id, and original path.
- Loading a session should parse the underlying transcript/messages for that exact selected record.
- Message previews in the detail pane should show the same user-visible content style as the official tool, including command/tool output formatting.
- Internal context, metadata, hidden tool payloads, and storage-only records should stay hidden when the official tool hides them.
- Compatibility readers are allowed only for real older/alternate storage layouts and should not override the current official path.
- App-only UI features such as source filters, project grouping, search, stats, cross-source sorting, and the Memory/Skills pane organization must not be used as evidence for official data behavior.

## Implementation Rules

- Keep data acquisition and message preview formatting aligned with the official tool behavior.
- Do not add custom title/message fallbacks unless the official tool does the same.
- Hide internal metadata when the official tool hides it.
- Format command and tool output the way the official tool **actually renders it in its TUI** — not what its docs say, and not what feels right. Always grep `.audit-sources/<repo>/` for the real render function and put a `// path/to/file.tsx:LINE` citation next to the matching Termory branch. Earlier rounds of this codebase had ~600 lines of tool-formatting guesswork that diverged from every TUI; those have been replaced and the rule exists to prevent regressing.
- Treat UI behavior separately from official data behavior. Source filters, project grouping, search, stats, cross-source sorting, and the Memory/Skills views are app UI behavior.
- Keep changes scoped. Avoid unrelated refactors.
- Add or update tests when changing a parser or formatter. Skill/memory scanners have parallel tests at the bottom of `sessions.rs` — extend that block when adding scan paths. Tool-rendering tests should assert verbatim strings (e.g. `"**Search**(pattern: \"TODO\", path: \"src\")"`), not regex matches, so renames are caught.
- When adding a new scan location for an existing tool, verify against the tool's official source first (then docs as a secondary reference); do not infer from naming conventions alone.

## Verification

Run when practical:

```sh
cd src-tauri
cargo fmt
```

```sh
cargo test --manifest-path src-tauri/Cargo.toml --lib
```

```sh
npm run build
```

Parser/formatter tests should cover the relevant official storage shape, title extraction, visible messages, hidden metadata, and command/tool preview formatting. Skill/memory tests should cover the actual scan paths and the per-tool tag string (e.g. `claude,opencode` for `.claude/skills/`).
