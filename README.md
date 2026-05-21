# Termory

> Management for your AI Terminal and Memory.

Local-first desktop app for browsing coding-agent CLI session history and inspecting the memory each tool writes to disk — Codex, Claude Code, Gemini CLI, and OpenCode in one place.

## Sources

Sessions:

- Codex CLI: `~/.codex/state_5.sqlite`, `~/.codex/sessions/**/*.jsonl`
- Claude Code: `~/.claude/projects/**/*.jsonl`
- Gemini CLI: `~/.gemini/tmp/**/chats/*.json`
- OpenCode: `~/.local/share/opencode/opencode.db` (with JSON storage fallback)

Memory (paths verified against each tool's open-source code, not just docs):

- Claude Code: `~/.claude/projects/<sanitized-canonical-git-root>/memory/**/*.md`, `~/.claude/rules/**/*.md` (global), `<cwd>/.claude/rules/**/*.md` (project)
- Codex: `~/.codex/memories/**/*.md` (excluding `skills/`)
- Gemini CLI:
  - Global: `~/.gemini/GEMINI.md` (legacy) and `~/.gemini/MEMORY.md` (modern alias)
  - Per-project: `~/.gemini/tmp/<id>/memory/{MEMORY.md preferred, GEMINI.md legacy}` (also recursive — source: `packages/core/src/config/storage.ts getProjectMemoryDir`)
- Per-project instruction files: scanned at cwd AND every ancestor **only when a `.git` directory exists at or above cwd**. Without `.git` only cwd is scanned (matches Codex/Gemini/OpenCode source). Walk stops at the git root (inclusive) and never enters `$HOME`. Files at each level:
  - `CLAUDE.md` → tag `claude,opencode`
  - `CLAUDE.local.md` → tag `claude`
  - `AGENTS.md` → tag `codex,opencode`
  - `AGENTS.override.md` → tag `codex`
  - `GEMINI.md` → tag `gemini`
  - `MEMORY.md` → tag `gemini`
  - `<cwd>/.claude/CLAUDE.md` → tag `claude` (project root only, not ancestors)
- Global instruction files: `~/.claude/CLAUDE.md` (`claude,opencode`), `~/.codex/AGENTS.md` + `~/.codex/AGENTS.override.md` (`codex`), `~/.config/opencode/AGENTS.md` (`opencode`)

Paths intentionally NOT scanned (not in any tool's source):

- `AGENTS.local.md` — not used by Codex/OpenCode (Codex uses `AGENTS.override.md`)
- `~/.codex/instructions.md` — legacy, no current Codex source reference
- `~/.claude/CLAUDE.local.md` (user-level) — only project-level is documented
- `CONTEXT.md` — OpenCode deprecated
- `project_doc_fallback_filenames` from `~/.codex/config.toml` — Termory does not read user config

Sources (source code, not docs):

- Codex: [`codex-rs/core/src/agents_md.rs`](https://github.com/openai/codex/blob/main/codex-rs/core/src/agents_md.rs), [`codex-rs/ext/memories/src/local.rs`](https://github.com/openai/codex/blob/main/codex-rs/ext/memories/src/local.rs), [`codex-rs/config/src/project_root_markers.rs`](https://github.com/openai/codex/blob/main/codex-rs/config/src/project_root_markers.rs)
- Gemini CLI: [`packages/core/src/utils/memoryDiscovery.ts`](https://github.com/google-gemini/gemini-cli/blob/main/packages/core/src/utils/memoryDiscovery.ts), [`packages/core/src/config/storage.ts`](https://github.com/google-gemini/gemini-cli/blob/main/packages/core/src/config/storage.ts), [`packages/core/src/tools/memoryTool.ts`](https://github.com/google-gemini/gemini-cli/blob/main/packages/core/src/tools/memoryTool.ts)
- OpenCode: [`packages/opencode/src/session/instruction.ts`](https://github.com/sst/opencode/blob/dev/packages/opencode/src/session/instruction.ts), [`packages/opencode/src/project/project.ts`](https://github.com/sst/opencode/blob/dev/packages/opencode/src/project/project.ts)
- Claude Code: `src/memdir/paths.ts`, `src/utils/attachments.ts` (videcoding/cli reference)

Skills (paths verified from each tool's open-source code):

- Claude Code: `~/.claude/skills/**/*.md` (global), `<cwd>/.claude/skills/**/*.md` (project) — tagged `claude,opencode` (OpenCode also officially reads `.claude/skills/`)
- Codex: `~/.codex/skills/**/*.md` (global, confirmed at `codex-rs/core/src/session/tests.rs:3817` — `codex_home.join("skills")`), `<cwd>/.codex/skills/**/*.md` (project)
- Gemini CLI: `~/.gemini/skills/**/*.md` (global, confirmed at `Storage.getUserSkillsDir`), `~/.gemini/tmp/<id>/memory/skills/**/*.md` (per-project, confirmed at `Storage.getProjectSkillsMemoryDir`), `<cwd>/.gemini/skills/**/*.md` (project workspace)
- OpenCode: `~/.config/opencode/skills/**/*.md` (global), `<cwd>/.opencode/skills/**/*.md` (project)
- Tool-neutral (Codex + Gemini CLI + OpenCode all officially read these): `~/.agents/skills/**/*.md` (global), `<cwd>/.agents/skills/**/*.md` (project) — tagged `codex,gemini,opencode`

The app reads source history in place and does not modify original session files.

## Stack

- Tauri v2
- React + TypeScript + Vite
- Rust backend
- SQLite reader for OpenCode

## Development

Prerequisites:

- Node.js 20+
- Rust toolchain with Cargo
- Platform-specific Tauri prerequisites

Install dependencies:

```bash
npm install
```

Run the web UI:

```bash
npm run dev
```

Run the desktop app:

```bash
npm run tauri:dev
```

Build:

```bash
npm run tauri:build
```

Run backend tests:

```bash
cargo test --manifest-path src-tauri/Cargo.toml --lib
```
