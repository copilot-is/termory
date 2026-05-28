import type { CliApp, MemoryTool, Route } from "./types";

export const MEMORY_SOURCE = "Memory";
export const SKILL_SOURCE = "Skill";

export const MEMORY_TOOL_ORDER: MemoryTool[] = [
  "Claude",
  "Codex",
  "Gemini",
  "OpenCode",
  "Other"
];

export const CLI_APPS: CliApp[] = ["claude", "codex", "gemini", "opencode"];

export const CLI_APP_LABEL: Record<CliApp, string> = {
  claude: "Claude Code",
  codex: "Codex",
  gemini: "Gemini CLI",
  opencode: "OpenCode"
};

export const CLI_APP_SOURCE_BADGE: Record<CliApp, string> = {
  claude: "Claude",
  codex: "Codex",
  gemini: "Gemini",
  opencode: "OpenCode"
};

export const OPENCODE_PROVIDER_ID_OPTIONS: {
  value: string;
  label: string;
  hint: string;
}[] = [
  {
    value: "openai-compatible",
    label: "openai-compatible (default)",
    hint: "Generic OpenAI-shaped REST. Use for PackyCode, DMXAPI, Open Router, etc."
  },
  {
    value: "anthropic",
    label: "anthropic",
    hint: "Anthropic Claude API. Use for endpoints that mimic api.anthropic.com."
  },
  {
    value: "openai",
    label: "openai",
    hint: "Real OpenAI api.openai.com."
  },
  {
    value: "google",
    label: "google",
    hint: "Google Gemini API."
  },
  {
    value: "azure",
    label: "azure",
    hint: "Azure-hosted OpenAI."
  },
  {
    value: "amazon-bedrock",
    label: "amazon-bedrock",
    hint: "AWS Bedrock."
  },
  {
    value: "google-vertex",
    label: "google-vertex",
    hint: "Google Vertex AI."
  }
];

export const ACTIVE_STATE_REFRESH_EVENT = "termory:providers-refresh";

export const ROUTES: Route[] = ["records", "search", "stats", "config", "settings"];

// Order matches the rail's visual order (Providers / Records / Search / Stats / Settings)
// and ⌘1..5 bindings.
export const RAIL_ROUTE_ORDER: Route[] = ["config", "records", "search", "stats", "settings"];

// Time gap (ms) between two messages that triggers a TimeSeparator
// row in the message stream.
export const TIME_GAP_MS = 5 * 60 * 1000;
