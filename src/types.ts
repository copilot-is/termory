// Cross-file type definitions for Termory.

export type AppSession = {
  id: string;
  source: string;
  title: string;
  project: string;
  path: string;
  started_at?: string | null;
  updated_at?: string | null;
  message_count: number;
  preview: string;
  snippet?: string;
  message_previews: SessionMessage[];
};

export type SessionMessage = {
  role: string;
  text: string;
  timestamp?: string | null;
  kind: string;
};

export type SessionDetail = {
  session: AppSession;
  messages: SessionMessage[];
};

export type SearchHit = {
  session: AppSession;
  snippet: string;
  role: string;
  match_count: number;
};

export type MemoryTool = "Claude" | "Codex" | "Gemini" | "OpenCode" | "Other";

export type CliApp = "claude" | "codex" | "gemini" | "opencode";

export type ProviderKind = "official" | "custom";

export type Provider = {
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
  // Claude-only options nested so the JSON is grouped: when set,
  // Claude Code's `/model` menu (Sonnet/Opus/Haiku) maps to these model
  // ids instead of the Anthropic-native ones — matters when the
  // provider doesn't speak Anthropic model id (e.g. routes Claude
  // requests to gpt-5).
  claude?: {
    haikuModel?: string;
    sonnetModel?: string;
    opusModel?: string;
  };
  // OpenCode-only nested options. `providerId` is the catalog id
  // whose npm package OpenCode should load (anthropic /
  // openai-compatible / …). `models` are extra model ids surfaced in
  // OpenCode's picker alongside the primary `model` (top-level).
  opencode?: {
    providerId?: string;
    models?: string[];
  };
};

export type ActiveKind = "official" | "custom" | "unmanaged";

export type LiveSnapshot = {
  baseUrl?: string | null;
  apiKeyMasked?: string | null;
  model?: string | null;
};

export type ActiveState = {
  app: CliApp;
  kind: ActiveKind;
  matchedProviderId?: string | null;
  liveSnapshot?: LiveSnapshot | null;
  livePath: string;
  // OpenCode-only: ids of Termory providers whose slots exist in
  // opencode.json (i.e. activated). Activated vs default are distinct
  // for OpenCode — multiple slots coexist, only one can be default.
  configuredProviderIds?: string[];
};

export type TestResult = {
  ok: boolean;
  status?: number | null;
  latencyMs: number;
  message: string;
};

export type Route = "records" | "search" | "stats" | "config" | "settings";

export type Pane = "sessions" | "memory" | "skills";
