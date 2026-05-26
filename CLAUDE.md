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
- Frontend local-store wrapper: `src/config.ts` (routes `getConfig`/`setConfig` to the two backing files via IPC)
- Tauri IPC commands: `src-tauri/src/lib.rs`
- Session/Memory/Skill scanning, parsing, and formatting: `src-tauri/src/sessions.rs`
- Provider switching (activate / deactivate / reverse-derive / test / fetch-models): `src-tauri/src/providers.rs`
- Local KV store (config.json + providers.json under `~/.termory/`, chmod 0600): `src-tauri/src/config.rs`
- Tauri config: `src-tauri/tauri.conf.json`
- Rust parser/formatter tests: inline tests at the bottom of `src-tauri/src/sessions.rs`
- Rust provider/store tests: inline tests at the bottom of `src-tauri/src/providers.rs` and `src-tauri/src/config.rs`

Current Tauri IPC commands, called from the frontend with `invoke(...)`:

- `scan_all_sessions` — returns Sessions + Memory + Skill entries as `AppSession[]`
- `load_session` — loads one entry by `{ source, path, id }`
- `search_all_sessions` — substring search across all loaded session/memory/skill bodies
- `provider_active_state(app, providers)` / `provider_active_states(providers)` — reverse-derive which Provider is currently active by reading the CLI's live config files; nothing about "active" is stored backend-side
- `activate_provider(provider, providersForApp)` / `deactivate_provider(app, providersForApp)` — materialize a Provider into the CLI's live config (or clear Termory-injected fields)
- `test_provider_api(provider)` — connectivity probe to the provider's base URL (returns `{ ok, status, latencyMs, message }`)
- `fetch_provider_models(provider)` — hits `/v1/models` (or `/v1beta/models?key=` for Gemini) and returns the available model ids for the editor's autocomplete
- `read_app_config` / `write_app_config` — `~/.termory/config.json` (UI prefs)
- `read_app_providers` / `write_app_providers` — `~/.termory/providers.json` (provider library, contains API keys, chmod 0600)

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

The repo also contains `.audit-sources/` (gitignored) with shallow clones of `openai/codex`, `google-gemini/gemini-cli`, `sst/opencode`, `videcoding/cli` (legacy Claude Code reference), and `farion1231/cc-switch` (reference for provider-switcher behavior). This is the source-of-truth for path/behavior verification when official docs disagree with implementation — grep here instead of WebFetching docs.

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

### Unified tool-message format — LOCKED RULE

Every tool message — regardless of source platform — uses the same markdown shape. This is a hard rule: any new tool / structured-result formatter MUST follow it without exception.

**Shape:**

`````
{status} **{Verb}**({args})

⎿ {summary}              ← present when the tool has a structured summary
                           (line counts, status codes, settings, etc.)

```{lang}                ← optional fence for diff / source / structured output
{body}
```

or

````                     ← 4-backtick fence for unstructured text output
{body}                     (avoids collision with content containing ```)
````
`````

**Rules:**

1. `{status}` glyph: `⏺` success, `✗` failure (Claude `constants/figures.ts:4` + Codex `exec_cell/render.rs:236`). Cross-platform — applied to every tool card.
2. `{Verb}` text is platform-native (Claude `userFacingName`, OpenCode `session-v2.tsx`, Codex `exec_cell/render.rs`, Gemini `displayName`); the wrapper shape `**Verb**(args)` is identical across platforms.
3. `{args}` always passes through `wrap_inline_code` (sessions.rs:48) so embedded backticks / `*` / `()` don't break markdown.
4. **`⎿ ` prefix is REQUIRED on every summary line**, with one trailing space before the content. Tools without a structured summary skip the line entirely (Bash, generic MCP, etc.). NEVER put `⎿` inside a code fence — browser monospace fonts render U+23BF inconsistently, breaking column alignment.
5. Summary content matches the per-tool Claude TUI component verbatim (count bolding, label pluralization). Examples:
   - `⎿ Read **N** lines` — `FileReadTool/UI.tsx:138-139`
   - `⎿ Wrote **N** lines to **{file}**` — `FileWriteTool/UI.tsx:58`
   - `⎿ Found **N** {label}` — `GrepTool/UI.tsx:32-33`
   - `⎿ Added **N** lines, removed **M** lines` — `FileEditToolUpdatedMessage.tsx:42-50`
   - `⎿ Received **{size}** ({code} {codeText})` — `WebFetchTool/UI.tsx:31-58`
   - `⎿ Error: Exit code N` (Codex) / `⎿ Error: {message}` (Claude — no exit code)
6. Reasoning across all platforms: `format_reasoning_body(content)` → `> *line*` italic blockquote.
7. **No content is dropped to match official TUI behavior.** Suppressed tools (Claude `TodoWrite`, `AskUserQuestion`, `EnterPlanMode`, etc.) and synthetic / contextual wrappers (Codex `<environment_context>`, OpenCode `synthetic` parts, Claude `<tick>` / `<local-command-caveat>` / NULL_RENDERING attachments) all surface in Termory — usually as italic `*[wrapper-name]*` notices when there's no structured representation. Termory is a history browser; hiding things misleads the user.

**Failure detection per platform** (`SessionMessage.exit_code: Option<i64>` carries the parsed value through `merge_tool_outputs`):

| Platform | Signal source | Notes |
|---|---|---|
| Codex | `Process exited with code N` / `Exit code: N` in the `function_call_output.output` wrapper (`ExecCommandToolOutput.response_text()` — context.rs:409) parsed by `codex_parse_exec_output` | Limited mode default; populates `exit_code` |
| Claude | `tool_result.is_error: true` content block | No exit code field — `Error:` prefix has no `Exit code N` part |
| OpenCode | `state.status === "error"` on a tool part (and `state.error.message` for the text); `assistant.error` for whole-message failures (`SessionErrorUnknown` shape per types.gen.ts:2905) | Body gets `✗ ` marker + 4-backtick `Error: {message}` fence; kind = `tool_error` so future UI can colour the card |
| Gemini | `status` field on each `toolCalls[]` entry; per `sessionUtils.ts:654-657` anything other than `'success'` (e.g. `'error'`, `'cancelled'`) maps to `CoreToolCallStatus.Error` | No exit code; body gets an `Error:` prefix |

### Per-platform verb mapping

Every Termory branch cites the exact source file that produces the verb in each TUI. Survey under `.audit-sources/{codex,gemini-cli,opencode,claude-code}/`.

**Codex** (`codex_function_call_message` for ResponseItem::FunctionCall, `codex_custom_tool_call_message` for ResponseItem::CustomToolCall) — `.audit-sources/codex/codex-rs/tui/src/exec_cell/render.rs:381-385`:

- `exec_command` / `shell` / `shell_command` / `local_shell` (all 4 names per `rollout-trace/src/tool_dispatch.rs:263`) → `**Bash**({wrap_inline_code(cmd)})` — verb unified with Claude per design call (was `**Ran** \`cmd\`` before unification).
- `apply_patch` → `**{Verb}**({wrap_inline_code(path)})\n\n```diff\n{patch}\n```` ` — `codex_parse_patch_actions` scans `*** Add File:` / `*** Delete File:` / `*** Update File:` markers, picks `Added` / `Deleted` / `Edited` per `diff_render.rs:421-436`. Multi-file patches collapse to `**Edited**({N} files)`. Modern Codex stores apply_patch as `payload.type = "custom_tool_call"` with `input` field (raw patch text); legacy form is `function_call` with `arguments`. Both shapes route to the same patch-header builder.
- `update_plan` → `**Updated Plan**` + optional `*explanation*` + GFM task list `- [x]/[~]/[ ]` (matches PlanUpdateCell at `history_cell/plans.rs:138-194` — TUI uses ✔/□ symbols with crossed-out / bold / dim styling; Termory stays on GFM markers so checkboxes render natively in react-markdown)
- `view_image` → `**Viewed Image**({wrap_inline_code(path)})` (patches.rs:63-72 — capital `I` per TUI)
- other → `**{name}**({compact args})` fallback

**Codex EventMsg dispatch** (`codex_event_msg_to_message`) — `RolloutItem::EventMsg` records are the canonical replay source for Codex; the wrapper `codex_message_from_value` routes `event_msg` records here. Handled variants:

- `user_message` / `agent_message` / `agent_reasoning` / `agent_reasoning_raw_content`
- `web_search_end` → `**Searched**({wrap_inline_code(detail)})` where `detail` follows Codex's `web_search_action_detail` (search.rs:13-38): `query` for `Search` (or first of `queries` with ` ...` suffix when multiple), `url` for `OpenPage`, `'pattern' in url` / `'pattern'` / `url` for `FindInPage`
- `mcp_tool_call_end` → `**MCP**({server}.{tool})` (dot separator per Codex `format_mcp_invocation` mcp.rs:761-780); when `arguments` is a non-empty / non-`null` JSON value, appends `, {compact_json}` inside the parens
- `image_generation_end` → `**Generated Image**({wrap_inline_code(prompt)})` + saved path (capital `I` per TUI patches.rs:74-93)
- `view_image_tool_call` → same shape as the function_call variant
- `plan_update` → same as the function_call `update_plan` (payload IS the UpdatePlanArgs)
- `patch_apply_end` (Extended mode) → per-file `**Verb**({path})` lines; on failure appends stderr fence + `**Error**`
- `context_compacted` → `*Context compacted*` system notice
- `error` → `**Error**: {message}` system notice
- `turn_aborted` → `*Turn interrupted by user*` / `*Turn stopped — budget limit reached*`
- `thread_rolled_back` → `*Rolled back N turn(s)*`
- `entered_review_mode` / `exited_review_mode` → italic notices

**Codex `custom_tool_call` / `custom_tool_call_output`** (`codex_custom_tool_call_message` / `codex_custom_tool_call_output_message`) — modern shape for apply_patch and similar tools, differs from `function_call`:
* input arrives in an `input` field (raw text) instead of `arguments` (JSON-encoded args)
* output is wrapped in a JSON envelope `{"output":"..."}` — the message handler unwraps `output` / `text` / `result` keys, falling back to raw on parse failure

Without these handlers, modern apply_patch was silently dropped and no ```diff fence was emitted.

`exec_command_end` (Extended-mode shell) is intentionally NOT dispatched yet — it would duplicate the ResponseItem-derived card. Need call_id-based dedup before enabling.

`Limited` vs `Extended` mode (per `codex-rs/rollout/src/policy.rs:135-153`): the CLI default is Limited (`tui/src/app_server_session.rs: persist_extended_history: false`), so most rollouts only carry `ResponseItem::FunctionCall` + `FunctionCallOutput` for shell tools — NOT `EventMsg::ExecCommandEnd`. Termory's `codex_function_call_output_message` is the authoritative path for shell output in that mode; `codex_parse_exec_output` strips the wrapper to recover `aggregated_output`.

**Claude Code** (`claude_tool_use_text`) — `.audit-sources/claude-code/src/components/messages/AssistantToolUseMessage.tsx:152` wraps `<bold>{userFacingName}</bold>({renderToolUseMessage})`. Each Tool's `UI.tsx` provides both pieces. All argument values pass through `wrap_inline_code` so markdown-special chars in user payloads can't leak.

`claude_tool_use_text` returns `Option<String>` so tools that Claude TUI explicitly suppresses (`userFacingName: ''` AND `renderToolUseMessage: () => null`) can return `None` and the entire tool card is skipped — matching the TUI which renders nothing for them:

| Raw name | userFacingName source | Termory output |
|---|---|---|
| `Bash` | BashTool/UI.tsx | `**Bash**({command})` (empty cmd → just `**Bash**`) |
| `Read` / `View` | FileReadTool/UI.tsx:179 → "Read" / "Read agent output" (path matches `/tasks/{taskId}.output` per `getAgentOutputTaskId`); `getPlansDirectory` "Reading Plan" variant is intentionally skipped (depends on session config) | `**Read**({path} · lines X-Y / · pages N / · limit N)` / `**Read agent output**({taskId})` |
| `Write` | FileWriteTool/UI.tsx → "Write" | `**Write**({path})` |
| `Edit` / `MultiEdit` / `str_replace*` | FileEditTool/UI.tsx:28-87 → "Update" by default, "Create" when `old_string === ''` (or first edit's `old_string === ''` for MultiEdit) | `**Update**({path})` / `**Create**({path})` |
| `Grep` | GrepTool.ts:170 → "Search" | `**Search**(pattern: ..., path: ...)` |
| `Glob` | GlobTool/UI.tsx:13 → "Search" | `**Search**(pattern: ..., path: ...)` |
| `WebFetch` | WebFetchTool.ts:81 → "Fetch" | `**Fetch**({url})` |
| `WebSearch` | WebSearchTool.ts:160 → "Web Search" (space) | `**Web Search**({query})` |
| `NotebookEdit` | NotebookEditTool.ts → "Edit Notebook" | `**Edit Notebook**({notebook_path})` |
| `Task` / `Agent` | AgentTool/UI.tsx `userFacingName` — "Agent" for `worker`/`general-purpose`/missing subagent_type, else the subagent_type verbatim | `**{verb}**({description})` (`verb` = "Agent" or `subagent_type`) |
| `Skill` | SkillTool/UI.tsx | `**Skill**({name})` |
| `ReadMcpResource` | ReadMcpResourceTool/UI.tsx → literal **`readMcpResource`** (camelCase, NOT title-cased) | `**readMcpResource**({uri})` |
| `ListMcpResources` | literal **`listMcpResources`** | `**listMcpResources**({server})` |
| `McpAuth` | McpAuthTool.ts → literal `'{server} - authenticate (MCP)'` (the whole label IS the verb) | `**{server} - authenticate (MCP)**` |
| `mcp__{server}__{tool}` (generic MCP) | — | `**MCP**({server}/{tool})` (matches Codex MCP) |
| **SUPPRESSED in Claude TUI** — `TodoWrite` / `AskUserQuestion` / `EnterPlanMode` / `ExitPlanMode` / `ExitPlanModeV2` / `TaskCreate` / `TaskUpdate` / `TaskGet` / `TaskList` / `TaskStop` / `TaskOutput` / `ToolSearch` | userFacingName `''` AND renderToolUseMessage returns null | `claude_tool_use_text` returns `None` → no tool card emitted at all |

**Claude content blocks** beyond `text` / `tool_use`:

- `thinking` and `redacted_thinking` → reasoning message via `claude_thinking_blocks` + `format_reasoning_body`. Claude TUI renders `∴ Thinking…` (AssistantThinkingMessage.tsx); Termory emits the unified `> *content*` blockquote instead.
- `image` (`{source: {type: "base64"|"url", media_type, ...}}`) → italic `*Image ({mime})*` or `*Image: {url}*` notice via `claude_image_part_label`.
- `tool_result.content` may be `Value::String` or `Value::Array` of `text` blocks. For `Edit` / `MultiEdit` / `Write` tools, Termory prefers the structured diff over the brief tool_result text — `claude_format_structured_patch` reads the JSONL line's sibling `toolUseResult.structuredPatch` field (the same data Claude TUI's `StructuredDiff.tsx` consumes) and emits a `**Added N lines, removed M lines**` summary header on its own line, then a ```diff fence with the actual hunks. NO `@@ -X,N +Y,M @@` text in the fence — Claude's `formatDiff` (StructuredDiff/Fallback.tsx:373-440) conveys hunk boundaries via gutter line-number jumps, not the unified-diff header. Multi-hunk patches get a blank line between hunks instead.

`claude_display_text` strips / rewrites the following Claude-internal text wrappers and constants (per `UserTextMessage.tsx:40-197` dispatch chain and `constants/messages.ts`):

| Wrapper / signal | Claude TUI | Termory output |
|---|---|---|
| `(no content)` (NO_CONTENT_MESSAGE) | null (UserTextMessage.tsx:48) | drop |
| `[Request interrupted by user]` / `[Request interrupted by user for tool use]` | `<InterruptedByUser>` italic (l.83-92) | `*[Interrupted by user]*` |
| `<tick>...</tick>` | null (l.57-59) | drop |
| `<local-command-caveat>...` | null (l.61-64) | drop |
| `<bash-stdout>...` / `<bash-stderr>...` | `<UserBashOutputMessage>` → stdout + stderr (l.66-71) | unwrapped + concatenated (stdout then `\n\n` + stderr); inner `<persisted-output>` also stripped |
| `<local-command-stdout>` / `<local-command-stderr>` | `<UserLocalCommandOutputMessage>` indented w/ Markdown (l.74-79) | inner text passed through |
| `<bash-input>...` | `<UserBashInputMessage>` `! {input}` (l.110-113) | `! {input}` |
| `<command-message>...` | `<UserCommandMessage>` `❯ /cmd args` (l.115-118) | `/cmd args` |
| `<user-memory-input>...` | `<UserMemoryInputMessage>` `# {content}` chip (l.120-122) | `\# {content}` (H1-escape so markdown doesn't render as heading) |
| `<task-notification>...<summary>...</summary>...` | `<UserAgentNotificationMessage>` `⏺ {summary}` (l.139-141) | `⏺ {summary}` |
| `<tool_use_error>...` (inside tool_result.content only) | stripped to inner text | inner error text only |
| `({tool} completed with no output)` (toolResultStorage.ts:293 placeholder) | `(No output)` summary via `BashToolResultMessage.tsx:107-121` | `(No output)` |

Feature-gated wrappers not handled: `<github-webhook-activity>` (KAIROS_GITHUB_WEBHOOKS), `<teammate-message>` (swarms), `<fork-boilerplate>` (FORK_SUBAGENT), `<cross-session-message>`, `<channel source=...>`, `<mcp-resource-update>` / `<mcp-polling-update>`. All are dropped silently via the generic `strip_display_tags` fallback.

**Claude top-level record types** (per Message.tsx:103-281 dispatch):

- `user` / `assistant` — message containers (see content-block handling above)
- `attachment` — dispatched per `attachment.type` by `claude_attachment_messages` (sessions.rs). Subtypes that emit a notice line: `directory`, `file` / `already_read_file` (with `numLines` / `cells` / `unchanged` / `bytes` detail), `compact_file_reference`, `pdf_reference`, `selected_lines_in_ide`, `nested_memory`, `skill_listing` (non-initial only), `queued_command` (prompt text run through `claude_display_text` so embedded `<task-notification>` etc. dispatch correctly), `plan_file_reference`, `invoked_skills`, `mcp_resource`. NULL_RENDERING subtypes (`task_reminder`, `deferred_tools_delta`, `command_permissions`, `date_change`, `hook_success`, `async_hook_response`, `agent_setting`, `relevant_memories`, `dynamic_skill`, `agent_listing_delta`) drop silently — matches `nullRenderingAttachments.ts:14-49`.
- `system` — dispatched by `subtype` via `claude_system_message`:
  - `local_command` → strips `<command-message>`/`<command-args>` to `/cmd args` (kind=LOCAL_COMMAND)
  - `turn_duration` → `*※ Worked for {duration}*` italic dim (matches SystemTextMessage.tsx:342-401). Duration formatted via `format_duration_short` (e.g. `45269ms` → `45.3s`).
  - `away_summary` → `*※ {content}*` italic dim (l.70-84)
  - `agents_killed` → `**Error** All background agents stopped` (l.87-101)
  - `compact_boundary` → `---\n\n*{content}*\n\n---` GFM divider notice (Message.tsx:195-203 `CompactBoundaryMessage`)
  - `microcompact_boundary` / `api_error` / other → silent drop (matches verbose-only or null fallthrough)

**OpenCode** (`opencode_v2_tool_part_text`) — each tool header uses the unified `**Verb**(args)` shape but the verb text + body content stay platform-native (matching `session-v2.tsx` lines cited below). Body decorations (`\# description` BlockTool title, bash fence with `$ cmd` prefix, ```diff diff fence, `↳ Loaded` instruction-file list, `{✓/~/✕/☐}` todo icons) are preserved verbatim — only the header line was reshaped:

- `Bash` / `Shell` (l.707): header `**Shell**({wrap_inline_code(cmd)})`. With output → followed by `\# {description ?? "Shell"}\n\n```bash\n$ {cmd}\n{output}\n```` (original BlockTool body). Without output → header alone (original InlineTool was `$ {cmd}`). Output resolution mirrors TUI l.710: `metadata.output ?? state.content` (Bash-specific override — other tools just use `state.content`), then `strip_ansi` to drop terminal colour codes.
- `Glob` (l.748): `**Glob**(pattern: {wrap_inline_code(pattern)}, path: {wrap_inline_code(path)} — {N} match[es])` (singular/plural matched).
- `Read` (l.764): `**Read**({wrap_inline_code(filePath)} [other=...])` + per-entry `↳ Loaded {path}` lines using CommonMark hard breaks (`\` line terminator). `metadata.loaded` is the `instruction.resolve` array from `read.ts:264` — the auto-loaded instruction files (AGENTS.md / CLAUDE.md / etc.) the Read tool fetched alongside the requested file; surfaced because it's data, not decoration.
- `Grep` (l.794): `**Grep**(pattern: {pattern}, path: {path} — {N} match[es])`.
- `WebFetch` (l.810): `**WebFetch**({wrap_inline_code(url)})`.
- `WebSearch` (l.818): `**{provider label}**({wrap_inline_code(query)} — {N} results)`. Verb is provider-derived per `webSearchProviderLabel` (`tool/websearch.ts:39-43`): `"parallel"` → `Parallel Web Search`, `"exa"` → `Exa Web Search`, otherwise → `Web Search` (default, with space — matches Claude's verb).
- `Write` (l.828): `**Write**({wrap_inline_code(filePath)})` + ```{lang from ext}\n{content}\n``` body when completed.
- `Edit` (l.857): `**Edit**({wrap_inline_code(filePath)})` + ```diff\n{diff}\n``` body when diff present.
- `ApplyPatch` (l.891): per-file header → `**Deleted**({path})` / `**Created**({path})` / `**Moved**({old → new})` / `**Patched**({path})` + ```diff fence (matches FileChange tags in fileTitle()). When a file has no `patch` text, body falls back to `-N line` / `-N lines` (pluralized per TUI l.923).
- `TodoWrite` (l.964): `**Todos**\n\n{✓/~/✕/☐} {content}` per todo (verb is "Todos" — matches the original BlockTool title `\# Todos`; icons from todoIcon helper).
- `Question` (l.991): `**Questions**\n\n{Q}\n{A}` per Q/A pair (verb "Questions" matches `\# Questions` title).
- `Skill` (l.1022): `**Skill**({wrap_inline_code(name)})`.
- `Task` (l.1030): `**{Titlecase(subagent_type ?? "General")} Task**({wrap_inline_code(description)})` — verb includes the agent name prefix, matching the original `{Agent} Task — description` heading.
- generic (l.522): `**{name}**({input})` header + 4-backtick output fence when present.
- `reasoning` part → `format_reasoning_body` (unified italic blockquote — replaces the old `_Thinking:_` inline prefix).

All tool cards emit a `⏺` / `✗` leading marker (`status_marker` per session-v2.tsx:572 + l.669 — error state flips the InlineTool/BlockTool color). Failed parts append a 4-backtick `Error: {message}` body from `state.error.message`, mirroring Codex / Claude / Gemini failure formatting.

Top-level `SessionMessage` types beyond the tool parts (session-v2.tsx Match arms l.92-122):

- `user` (l.159 UserMessage) → `text` body + attachment row built by `opencode_v2_user_attachments`. Files surface as `` `{mime}` `` `` `{name ?? uri}` `` code-span pairs (l.176-185); agents as `` `agent` `` `` `{name}` `` (l.186-193). `references` (PromptReferenceAttachment) are persisted but TUI skips them, so Termory does too.
- `assistant` (l.296 AssistantMessage) → text from parts; if `message.error.message` is set (l.339-353), append `*✕ {message}*` italic notice on its own line.
- `synthetic` (l.105-107) → TUI renders `<></>`; Termory returns `None` so they don't appear in the transcript.
- `shell` (l.200 ShellMessage) → `$ {command}` + `strip_ansi(output)` on a second line.
- `compaction` (l.231) → bold header `**Auto Compaction**` (when `reason === "auto"`) or `**Compaction**`, followed by the `summary` body.
- `agent-switched` (l.261) → `▣ Switched agent to {Titlecase(agent)}` (prefix matches the TUI agent-color glyph at l.267).
- `model-switched` (l.275) → `◇ Switched model to {provider}/{id}[/{variant}]` (prefix matches l.284 secondary-color glyph).

Audit reference is OpenCode `1.15.5` (commit `9324ef0`). Compared against `v1.15.7`: only cosmetic reasoning collapse-icon change in session-v2.tsx (`▼/▶` → `-/+`), no structural / schema diffs. No re-audit needed.

**Gemini CLI** (`gemini_tool_messages_from_value` + `gemini_thought_messages_from_value` + `gemini_part_to_string`) — `.audit-sources/gemini-cli/packages/cli/src/ui/components/messages/`:

- `toolCalls[]` entries (ToolShared.tsx:202 `ToolInfo`) → `{status_marker} **{displayName}**({description})` with status-aware body. `status === 'success'` → `⏺` marker, body fenced verbatim. Otherwise `✗` marker + `Error: ` prefix inside the fence (per sessionUtils.ts:654-657 only `'success'` is success). `resultDisplay` body shapes per `ToolResultDisplay.tsx` are dispatched in `gemini_result_display_to_text`:
  - `string` → as-is (markdown / plain text)
  - `Array<AnsiLine>` (each line is `Array<AnsiToken {text, ...}>`, detected via `isAnsiOutput`) → join token `text` fields, trim per-line trailing whitespace (xterm-headless pads to terminal width)
  - `{todos: ...}` → drop body (TUI hides it; TodoTray renders todos separately, ToolResultDisplay.tsx:84-87)
  - `{isSubagentProgress: true, ...}` → drop body (live-progress widget, no useful static representation)
  - `{fileDiff, fileName?}` → `gemini_format_file_diff` (DiffRenderer.tsx:204-214 `isNewFile`): when every non-header line is an addition, emit ```{lang}\n{added lines}\n``` (lang inferred from the filename extension); otherwise ```diff\n{full diff}\n```
  - `{summary, ...}` (StructuredToolResult / GrepResult / ListDirectoryResult / ReadManyFilesResult) → emit the `summary` string
  - other object → `serde_json::to_string_pretty` fallback (matches TUI's `JSON.stringify(obj, null, 2)`)
- `thoughts[{subject, description}]` array (ThinkingMessage.tsx:22 `normalizeThoughtLines`) → one reasoning message per entry. Subject is wrapped in `**...**` so `format_reasoning_body` keeps it as a bold blockquote header line (mirrors the TUI's bold-italic subject + italic body at l.84-93); description lines render italic. `gemini_normalize_escaped_newlines` applies the same `\\n` / `\\r\\n` → real-newline pass as `textUtils.ts:168` so persisted escaped literals split into multiple lines. Noise filtering matches the source (skip whitespace-only or `...` runs)
- System-notice records (`type: 'info' | 'error' | 'warning'`) → `format_gemini_system_notice` wraps the body in an italic span with the TUI icon prefix (`ℹ` per InfoMessage.tsx:30 / `✕` per ErrorMessage.tsx:16 / `⚠` per WarningMessage.tsx:17). Multi-line bodies use CommonMark hard breaks (`  \n`) so the italic span survives across visual lines without a paragraph break terminating it
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
- `strip_ansi(text)` — drop ANSI escapes (CSI colour / cursor codes, OSC title-set sequences, and lone `ESC + letter` escapes). Used for OpenCode Bash output (session-v2.tsx:710) and `type: "shell"` message captures (session-v2.tsx:203). No regex crate — small inline state machine, leaves non-ESC content untouched.

### Tool message metadata + UI

- `SessionMessage` carries two `#[serde(skip)]` fields used only during parsing/merging:
  - `tool_use_id: Option<String>` — links `tool_use` ↔ `tool_result` by provider id (Claude `tool_use.id` / Codex `function_call.call_id`).
  - `exit_code: Option<i64>` — Codex shell exit code parsed from `function_call_output` metadata; surfaced in the `Error: Exit code N` fence line.
- Provider-native combined formats (OpenCode parts, Gemini toolCalls, Codex EventMsg-derived cards) skip `merge_tool_outputs` — they already arrive complete with their own fence and add the `⏺` / `✗` prefix at emission time.

### Markdown rendering (frontend)

- The detail-pane body uses `react-markdown` + `remark-gfm` (tables / task lists / strikethrough). No syntax-highlight pass: code blocks render as plain monospace until a per-language renderer is added intentionally.
- No DOMPurify / rehype-sanitize: react-markdown emits React elements (not HTML strings), so raw `<tag>` in session content is auto-escaped by React's text node rendering and displays as literal text — same characters the CLI shows.
- No raw / rendered toggle and no `viewMode` state — every message renders through the same react-markdown pipeline. The "open original file" affordance in the detail header still lets the user inspect the underlying JSONL / db row outside Termory.
- **TUI-style scrollback layout**: messages render as continuous vertical flow without card borders. Each message has a colored left bar (`.roleBar`) + lowercase `.roleLabel` (OpenCode TUI inspired — `session-v2.tsx:267, 284, 357`). The bar color encodes role: user teal, assistant blue, tool tan, event gray. No per-message timestamp chip; `<TimeSeparator>` between turn boundaries handles big time jumps.
- `.messageBody` has `padding-left: 11px` so body text aligns under the role label (label sits 3px bar + 8px gap = 11px from the message left edge). Overridden to `padding: 0` for `.docBody` (memory/skill single-doc view).
- Inline `<code>` carries `word-break: break-all` so a long no-space path inside `**Read**(\`/very/long/path\`)` wraps with the surrounding paragraph instead of overflowing the message card.
- `.messageBody pre` has `margin: 0; padding: 0 0 0 1em` — code fences sit flush with the verb header above, with a 1em left indent so the fence content visually aligns with the verb under the `⏺` marker.
- `.message.tool .messageBody p + p { padding-left: 1em }` — second-and-later paragraphs inside tool cards (e.g. the `⎿ Added N lines, removed M lines` summary above an Edit diff) indent 1em to align with the verb past the `⏺` glyph. Scoped to `.message.tool` so plain-text assistant messages with multi-paragraph layouts stay flush-left.
- `.messageBody p + pre { margin-top: -0.4em }` — pulls a fence visually onto the preceding paragraph when they form a logical pair (summary + diff). The CommonMark-required blank line between them stays in the markdown source so the fence is still recognized as a block.
- Unordered lists render with the `- ` text marker via `list-style-type: "- "` (matching Codex TUI's `start_item` output at `codex-rs/tui/src/markdown_render.rs:754-760`).
- Tool detail-pane loading state shows only the spinner icon (no `Loading transcript` label) so the brief delay between session select and detail load is unobtrusive.

## History and Preview Behavior

- Session lists should come from the same stored records the official tool uses for its history/resume list.
- Session list fields should use official values when available: title, project/cwd, timestamps, source id, and original path.
- Loading a session should parse the underlying transcript/messages for that exact selected record.
- Message previews in the detail pane should show the same user-visible content style as the official tool, including command/tool output formatting.
- **Show everything that was recorded.** Termory deliberately surfaces content that the official TUIs hide (Claude's suppressed tools, `<tick>` / `<local-command-caveat>` / NULL_RENDERING attachments / `isMeta` user messages; Codex's `<environment_context>` / `<user_instructions>` / `<skill_instructions>` / etc.; OpenCode's `synthetic` parts). The transcript is the source of truth — hiding things makes the history misleading. List-time filters (`isSidechain`, `kind === "subagent"`) that decide *which sessions* appear in the list are separate and still apply.
- Compatibility readers are allowed only for real older/alternate storage layouts and should not override the current official path.
- App-only UI features such as source filters, project grouping, search, stats, cross-source sorting, and the Memory/Skills pane organization must not be used as evidence for official data behavior.

## Implementation Rules

- Keep data acquisition and message preview formatting aligned with the official tool behavior, but **never hide content** — Termory is a history browser, so anything recorded must be surfaced (usually as an italic `*[wrapper-name]*` notice when there's no nicer representation). See the "Unified tool-message format — LOCKED RULE" rule 7.
- Do not add custom title/message fallbacks unless the official tool does the same.
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

## Providers (switch CLI to a third-party API platform)

User flow: a Provider is a named snapshot of `{baseUrl, apiKey, model, ...}`. Each CLI has its own list. Activate = materialize into the CLI's live config so the next launch picks it up. Switching back to Official = clear the Termory-injected fields; the CLI's native OAuth/credentials file is **never touched**, so logins survive a round-trip.

**Local storage** — `~/.termory/` (same path on macOS / Linux / Windows), permissions `0700` dir / `0600` files on Unix. Atomic write (tmp + rename):

- `~/.termory/config.json` — UI prefs (`default_pane`, `providers_app`, `recent_searches`). No secrets.
- `~/.termory/providers.json` — provider library (`Provider[]`). Contains API keys, `0600`.

Termory does **not** store an "active provider" pointer anywhere. `provider_active_state` reverse-derives the active state on every read by parsing the CLI's live config files and matching against the in-memory provider list — this keeps Termory consistent when other tools (`cc-switch`, manual `vim`, the CLI's own OAuth flow) change the same files.

`Provider` schema (in `providers.json`, fields default to omitted when empty):

```
{ id, app: "claude"|"codex"|"gemini"|"opencode", kind: "custom"|"official",
  name, baseUrl?, apiKey?, model?,
  claude?: { haikuModel?, sonnetModel?, opusModel? },
  opencode?: { providerId? } }
```

App-specific options live under their app key so the JSON is grouped and `Provider` doesn't grow a wide column of `xxx_yyy_zzz` flat fields. The editor submit pass collapses an empty nested block to `undefined` so providers.json never carries `claude: {}` / `opencode: {}`; `config.ts` strips `""`/`null`/`undefined` top-level fields the same way. `opencode.providerId` is the AI SDK catalog id (`anthropic` / `openai` / `openai-compatible` / …) Termory writes the key under in `~/.local/share/opencode/auth.json`.

### Per-CLI materialization (source-of-truth: cite official source AND cc-switch when both verified)

All four were cross-verified against the upstream CLI source (`.audit-sources/{codex,claude-code,gemini-cli,opencode}/`) AND cc-switch's implementation (`.audit-sources/cc-switch/src-tauri/`). Cite `file:line` next to each branch — don't infer from docs.

| CLI | Files written | Key fields | OAuth credential file (untouched) |
|---|---|---|---|
| **Claude Code** | `~/.claude/settings.json` (merge) | `env.ANTHROPIC_BASE_URL`, `env.ANTHROPIC_AUTH_TOKEN`, `env.ANTHROPIC_MODEL`, plus optional sub-routing `env.ANTHROPIC_DEFAULT_{HAIKU,SONNET,OPUS}_MODEL` for Claude Code's `/model` size picker (see `model.ts:69, 105-138`) | `~/.claude/.credentials.json` (or macOS Keychain — `auth.ts:1323`). Independent file, isolated by design. |
| **Codex** | `~/.codex/auth.json` (merge) + `~/.codex/config.toml` (merge) | auth.json: `auth_mode = "apikey"`, `OPENAI_API_KEY` (matches `login_with_api_key` shape `manager.rs:529-542`; we **merge** instead of nulling `tokens` so ChatGPT OAuth survives a swap). config.toml: top-level `model_provider`, `model`; `[model_providers.termory]` block with `name`, `base_url`, `wire_api = "responses"`, `requires_openai_auth = true` (gates `AuthManager`'s auth.json load — `tui/src/lib.rs:1817`). **Never set `env_key`** — it forces Codex onto a hard env-var path with no fallback (`model-provider/src/auth.rs:92-103`). | The `tokens` / `last_refresh` / `agent_identity` fields **inside** auth.json. Termory never overwrites them; deactivate only removes `auth_mode` + `OPENAI_API_KEY`. |
| **Gemini CLI** | `~/.gemini/.env` (dotenv merge, `chmod 0600`) | `GOOGLE_GEMINI_BASE_URL` (triggers GATEWAY mode `contentGenerator.ts:85-87`), `GEMINI_API_KEY`, `GEMINI_MODEL` (`config.ts:836-837`). Other env vars in the file are preserved. | `~/.gemini/oauth_creds.json` + `~/.gemini/google_accounts.json` (`storage.ts:22, 87`). Independent files, isolated. |
| **OpenCode** | `~/.local/share/opencode/auth.json` (merge, additive) + optional `~/.config/opencode/opencode.json` overlay | auth.json: `{<provider_id>: {type: "api", key: "..."}}` under the user-chosen AI SDK catalog id (`anthropic` / `openai` / `openai-compatible` / `google` / `azure` / `amazon-bedrock` / `google-vertex`). Same shape `/connect` writes (`auth/index.ts:22-26, 72-80`) so OpenCode's built-in npm + model directory is reused — Termory does NOT re-declare `npm`/`name`/`models`/top-level `model`. Optional overlay: `provider.<id>.options.baseURL` in opencode.json — the official "custom base URL" pattern from `providers.mdx:36-48`. All other `provider.*` blocks (and other auth.json entries, including OAuth-type entries from `providers login`) are preserved on activate AND deactivate. | Other auth.json entries with `type ≠ "api"` — or any entry under a different catalog id — are not touched. OpenCode's `providers login` writes `{type: "oauth", ...}` entries under its own id; Termory only adds/removes its own `{type: "api"}` entry under the user-chosen id. |

**Codex "stable provider id" rationale:** Codex stores session history keyed by `model_provider`. If we used a different id per provider, switching would visually "drop" history. We pin all Termory-written Codex provider blocks to id `"termory"` (`TERMORY_PROVIDER_ID` constant) and refuse to overwrite Codex's built-in reserved ids (`openai`/`amazon-bedrock`/`ollama`/`lmstudio` — `CODEX_RESERVED_IDS`).

**Claude sub-model routing (Advanced section):** Claude Code's `/model` menu in 3P mode reads `process.env.ANTHROPIC_DEFAULT_{HAIKU,SONNET,OPUS}_MODEL` via `getDefaultXxxModel()` (`model.ts:105-138`). Users who route different sizes to different upstream models (e.g. Sonnet → `gpt-5`, Opus → `claude-opus-4-7`) fill the Advanced fields; empty fields strip the corresponding env var.

**Test coverage:** Each CLI has an activate/reverse roundtrip test, an unrelated-fields-preserved test, and an OAuth-credentials-isolated three-stage test (Stage 1 simulates a prior CLI login, Stage 2 activates a Custom provider via Termory, Stage 3 deactivates — credentials file must be byte-identical at the end). See `providers::tests::*` in `src-tauri/src/providers.rs`.

## Pending feature work

The current UI shell is settled: activity rail (Records / Search / Stats / Providers / Settings), routed via URL hash, with a passive bottom freshness footer fed by the Rust filesystem watcher. Records and Providers are fully implemented; the other three rail destinations render placeholder cards.

Roadmap below is grouped by priority. Pick top-down within a tier.

### P0 — core capabilities for v1 ✅

All P0 items have shipped:

- ~~Search page + Cmd-K palette~~ — done. Backend `search_all_sessions` IPC + frontend `SearchPage` (grouped by source with snippet highlight) + global Cmd-K palette reusing the same store + recent-search history persisted via `config.ts`.
- ~~Empty states per route~~ — done. Distinguishes first-launch (no data) from filtered-empty (with "Clear filters" action) across Sessions / Memories / Skills panes.
- Message content rendering polish — deferred. `renderMessages` / `TimeSeparator` helpers stay staged. Revisit only with a concrete pain point.

### P1 — new pages & persistence

- ~~`tauri-plugin-store`~~ — replaced with custom `config.rs` module (`~/.termory/{config,providers}.json` with `chmod 0600`). The plugin couldn't control file location or Unix permissions; rolling our own KV gives both.
- ~~Providers page~~ — done. See the "Providers" section above. Cross-verified against `.audit-sources/cc-switch/` for the per-CLI write shapes; 4 CLIs supported with per-CLI tests. OpenCode adapter was re-done in a second pass: instead of self-declaring `provider.termory.{npm,name,models}` (which fights OpenCode's catalog at runtime), Termory now writes the API key under one of OpenCode's built-in catalog ids (`anthropic`/`openai`/`openai-compatible`/…) into `~/.local/share/opencode/auth.json` — same shape `/connect` produces — and optionally overlays a `baseURL` in `opencode.json`. The AI SDK dropdown in the editor is the catalog id picker.
- **Stats page** — dashboards (sessions/day, tokens/tool, top projects, model distribution). Requires extending the four parsers in `sessions.rs` to extract token counts (currently dropped); each platform exposes the field differently.
- **App Settings page** — theme, scan-path overrides, keyboard shortcuts, watcher toggle.

### P2 — quality of life

- **Right-click context menus** — on list items ("Re-read this file", "Reveal in Finder", "Copy ID") and on sidebar source rows ("Re-scan this source").
- **Keyboard navigation** — arrow keys in lists, Enter to open, Esc to close, Cmd-1..5 to switch rail, Cmd-F to summon search.
- **Watcher completion** — current Rust watcher in `watcher.rs` skips cwd / per-project files (CLAUDE.md, AGENTS.md, etc.) to avoid `node_modules` noise. Either add a targeted cwd watcher or trigger a force-rescan on route change.
- **Frontend test baseline** — zero tests today. Start with Vitest + React Testing Library on `CopyMenu`, `FreshnessFooter`, the route reducer.

### P3 — nice to have

- **New-item badges** — watcher detects new session → rail Records icon + sidebar source row get an unread badge; cleared on view. Needs P1 store for cross-restart state.
- **Export session** — single session → markdown / PDF, surfaced via the detail header's existing action row.
- **Starred sessions / tags** — virtual "Starred" source in sidebar; custom labels per record. Needs P1 store.
