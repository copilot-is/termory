//! Provider management for Claude Code / Codex / Gemini CLI / OpenCode.
//!
//! Design principle (per user instruction): **Termory does NOT store an
//! "active provider" pointer.** The active state is always re-derived
//! by reading each CLI's live configuration file and matching it
//! against the saved provider list. This keeps Termory consistent
//! when:
//!   - users edit the CLI config by hand
//!   - other tools (cc-switch, scripts) change the config
//!   - the CLI itself rewrites the config via OAuth flows
//!
//! What Termory DOES store (frontend prefs.json):
//!   - The Provider list (user-defined named snapshots)
//!   - UI prefs (which tab was last viewed, etc.)
//!
//! What this module owns (backend):
//!   - per-CLI activate / deactivate functions that write live configs
//!   - per-CLI read_active function that reverse-derives state
//!   - test_provider function that pings the API
//!
//! Provider data model (intentionally a flat user-facing shape, not
//! the per-CLI raw `settings_config` value cc-switch uses):
//!   { id, app, kind, name, base_url, api_key, model, ... }
//! The activate functions translate this into the right shape for
//! each CLI's config file format.

use serde::{Deserialize, Serialize};
use serde_json::{Map, Value as JsonValue};
use std::error::Error;
use std::fs;
use std::io::Write;
use std::path::{Path, PathBuf};
use std::time::Duration;

use toml_edit::{value as toml_value, DocumentMut, Item};

/// Stable provider id Termory uses inside Codex's `[model_providers.X]`
/// table and OpenCode's `provider.X` map. This avoids reserving any
/// of the CLI's built-in provider ids (codex: openai/amazon-bedrock/
/// ollama/lmstudio; opencode picks model id by `provider/model`) and
/// — for Codex — prevents session-history drift across switches
/// because Codex groups history by model_provider id.
pub const TERMORY_PROVIDER_ID: &str = "termory";

/// Codex's reserved built-in provider ids — writing to one of these
/// names doesn't actually take effect (built-ins win in
/// `merge_configured_model_providers` via `or_insert`).
const CODEX_RESERVED_IDS: &[&str] = &[
    "amazon-bedrock",
    "openai",
    "ollama",
    "lmstudio",
    "oss",
    "ollama-chat",
];

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "lowercase")]
pub enum CliApp {
    Claude,
    Codex,
    Gemini,
    Opencode,
}

impl CliApp {
    pub fn as_str(self) -> &'static str {
        match self {
            CliApp::Claude => "claude",
            CliApp::Codex => "codex",
            CliApp::Gemini => "gemini",
            CliApp::Opencode => "opencode",
        }
    }

    pub fn parse(raw: &str) -> Option<Self> {
        match raw {
            "claude" => Some(CliApp::Claude),
            "codex" => Some(CliApp::Codex),
            "gemini" => Some(CliApp::Gemini),
            "opencode" => Some(CliApp::Opencode),
            _ => None,
        }
    }
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "lowercase")]
pub enum ProviderKind {
    /// Use the CLI's native account/OAuth login. Activating means
    /// clearing the Termory-injected fields from the live config so
    /// the CLI falls back to its native auth flow.
    Official,
    /// Third-party API platform. Activating writes base_url + api_key
    /// + model into the live config in the per-CLI shape.
    Custom,
}

/// Which env-var name Claude Code looks up the auth string under.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Provider {
    pub id: String,
    pub app: CliApp,
    pub kind: ProviderKind,
    pub name: String,
    #[serde(default)]
    pub base_url: String,
    #[serde(default)]
    pub api_key: String,
    #[serde(default)]
    pub model: String,
    /// Claude-only: model id used when the user picks "Haiku" from
    /// Claude Code's `/model` menu (`modelOptions.ts:167` reads
    /// ANTHROPIC_DEFAULT_HAIKU_MODEL). Empty = inherit `model`.
    #[serde(default)]
    pub claude_haiku_model: String,
    /// Claude-only: model id used when the user picks "Sonnet".
    /// Backed by ANTHROPIC_DEFAULT_SONNET_MODEL.
    #[serde(default)]
    pub claude_sonnet_model: String,
    /// Claude-only: model id used when the user picks "Opus".
    /// Backed by ANTHROPIC_DEFAULT_OPUS_MODEL.
    #[serde(default)]
    pub claude_opus_model: String,
}

/// Reverse-derived active state for a single CLI.
#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ActiveState {
    pub app: CliApp,
    pub kind: ActiveKind,
    /// When kind=Custom, the id of the matched Provider from the
    /// user's list (or None when no Provider matches and the state
    /// is "Unmanaged").
    pub matched_provider_id: Option<String>,
    /// Reverse-derived snapshot of what's actually in live config.
    /// Always populated when kind != Official (used for the
    /// Unmanaged banner).
    pub live_snapshot: Option<LiveSnapshot>,
    /// Path of the file(s) consulted, for "open in finder" UX.
    pub live_path: String,
}

#[derive(Debug, Clone, Copy, Serialize, PartialEq, Eq)]
#[serde(rename_all = "lowercase")]
pub enum ActiveKind {
    Official,
    Custom,
    Unmanaged,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct LiveSnapshot {
    pub base_url: Option<String>,
    pub api_key_masked: Option<String>,
    pub model: Option<String>,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct TestResult {
    pub ok: bool,
    pub status: Option<u16>,
    pub latency_ms: u128,
    pub message: String,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ModelListResult {
    pub ok: bool,
    pub models: Vec<String>,
    pub status: Option<u16>,
    pub message: String,
}

// ===================================================================
// Activation entry point
// ===================================================================

pub fn activate(provider: &Provider, providers_for_app: &[Provider]) -> Result<(), Box<dyn Error>> {
    if provider.kind == ProviderKind::Official {
        return deactivate(provider.app, providers_for_app);
    }
    match provider.app {
        CliApp::Claude => activate_claude(provider),
        CliApp::Codex => activate_codex(provider),
        CliApp::Gemini => activate_gemini(provider),
        CliApp::Opencode => activate_opencode(provider, providers_for_app),
    }
}

/// Clear all Termory-injected fields from the live config so the CLI
/// falls back to its native auth flow.
pub fn deactivate(app: CliApp, providers_for_app: &[Provider]) -> Result<(), Box<dyn Error>> {
    match app {
        CliApp::Claude => deactivate_claude(),
        CliApp::Codex => deactivate_codex(),
        CliApp::Gemini => deactivate_gemini(),
        CliApp::Opencode => deactivate_opencode(providers_for_app),
    }
}

// ===================================================================
// Read active state (per-CLI reverse derivation)
// ===================================================================

pub fn read_active_state(
    app: CliApp,
    providers_for_app: &[Provider],
) -> Result<ActiveState, Box<dyn Error>> {
    match app {
        CliApp::Claude => read_active_claude(providers_for_app),
        CliApp::Codex => read_active_codex(providers_for_app),
        CliApp::Gemini => read_active_gemini(providers_for_app),
        CliApp::Opencode => read_active_opencode(providers_for_app),
    }
}

// ===================================================================
// Claude Code
// ===================================================================
//
// File: ~/.claude/settings.json
// Custom: writes env.ANTHROPIC_BASE_URL + env.ANTHROPIC_AUTH_TOKEN
//         + env.ANTHROPIC_MODEL (when set)
// Official: removes those env keys
// Reverse: read env block, compare to provider list
//
// OAuth credentials live in a separate file (~/.claude/.credentials.json
// or the macOS Keychain — see auth.ts:1323) which we never touch, so
// switching to a Custom provider and back leaves the OAuth login
// intact automatically.

fn claude_settings_path() -> Result<PathBuf, Box<dyn Error>> {
    Ok(home()?.join(".claude").join("settings.json"))
}

fn activate_claude(p: &Provider) -> Result<(), Box<dyn Error>> {
    let path = claude_settings_path()?;
    let mut root = load_json_object(&path)?;
    let env = ensure_json_object(&mut root, "env")?;
    if !p.base_url.is_empty() {
        env.insert(
            "ANTHROPIC_BASE_URL".into(),
            JsonValue::String(p.base_url.clone()),
        );
    } else {
        env.remove("ANTHROPIC_BASE_URL");
    }
    // Claude reads ANTHROPIC_AUTH_TOKEN first (treated as OAuth-style
    // bearer in `src/utils/auth.ts:164`), and falls back to
    // ANTHROPIC_API_KEY. We always write AUTH_TOKEN and clear API_KEY
    // — covers ~all known third-party gateways. Users who hit a
    // platform that requires API_KEY can edit settings.json directly.
    env.remove("ANTHROPIC_API_KEY");
    if !p.api_key.is_empty() {
        env.insert(
            "ANTHROPIC_AUTH_TOKEN".into(),
            JsonValue::String(p.api_key.clone()),
        );
    } else {
        env.remove("ANTHROPIC_AUTH_TOKEN");
    }
    // Main model goes into env.ANTHROPIC_MODEL — matches cc-switch
    // and Claude Code's priority chain (model.ts:69:
    // `process.env.ANTHROPIC_MODEL || settings.model`).
    if !p.model.is_empty() {
        env.insert("ANTHROPIC_MODEL".into(), JsonValue::String(p.model.clone()));
    } else {
        env.remove("ANTHROPIC_MODEL");
    }
    // Per-size routing: when the user picks Haiku / Sonnet / Opus
    // from Claude Code's `/model` menu in 3P mode, Claude reads
    // these env vars to decide the actual model id to send (see
    // `modelOptions.ts:77-89, 109, 167` — `is3P && customXxxModel`
    // branch). Empty string in the provider means "don't override
    // this size"; we strip the corresponding env var so Claude falls
    // back to its default Anthropic-side resolution.
    for (env_key, val) in [
        ("ANTHROPIC_DEFAULT_HAIKU_MODEL", &p.claude_haiku_model),
        ("ANTHROPIC_DEFAULT_SONNET_MODEL", &p.claude_sonnet_model),
        ("ANTHROPIC_DEFAULT_OPUS_MODEL", &p.claude_opus_model),
    ] {
        if val.is_empty() {
            env.remove(env_key);
        } else {
            env.insert(env_key.into(), JsonValue::String(val.clone()));
        }
    }
    // Strip any leftover top-level `model` field — earlier Termory
    // versions wrote here, so on upgrade we tidy up so the env-side
    // value is the single source of truth.
    root.remove("model");
    write_json_object(&path, &root)
}

fn deactivate_claude() -> Result<(), Box<dyn Error>> {
    let path = claude_settings_path()?;
    if !path.exists() {
        return Ok(());
    }
    let mut root = load_json_object(&path)?;
    if let Some(JsonValue::Object(env)) = root.get_mut("env") {
        env.remove("ANTHROPIC_BASE_URL");
        env.remove("ANTHROPIC_AUTH_TOKEN");
        env.remove("ANTHROPIC_API_KEY");
        env.remove("ANTHROPIC_MODEL");
        env.remove("ANTHROPIC_DEFAULT_HAIKU_MODEL");
        env.remove("ANTHROPIC_DEFAULT_SONNET_MODEL");
        env.remove("ANTHROPIC_DEFAULT_OPUS_MODEL");
        if env.is_empty() {
            root.remove("env");
        }
    }
    // Clear top-level `model` too — covers settings.json files written
    // by earlier Termory versions, before we switched to env.ANTHROPIC_MODEL.
    root.remove("model");
    write_json_object(&path, &root)
}

fn read_active_claude(providers: &[Provider]) -> Result<ActiveState, Box<dyn Error>> {
    let path = claude_settings_path()?;
    let live_path = path.display().to_string();
    if !path.exists() {
        return Ok(ActiveState {
            app: CliApp::Claude,
            kind: ActiveKind::Official,
            matched_provider_id: None,
            live_snapshot: None,
            live_path,
        });
    }
    let root = load_json_object(&path)?;
    let env = root.get("env").and_then(|v| v.as_object());
    let base_url = env
        .and_then(|e| e.get("ANTHROPIC_BASE_URL"))
        .and_then(|v| v.as_str())
        .map(str::to_string);
    let auth_token = env
        .and_then(|e| e.get("ANTHROPIC_AUTH_TOKEN"))
        .and_then(|v| v.as_str())
        .map(str::to_string);
    let api_key = env
        .and_then(|e| e.get("ANTHROPIC_API_KEY"))
        .and_then(|v| v.as_str())
        .map(str::to_string);
    let chosen_key = auth_token.clone().or(api_key.clone());
    // Read model from env.ANTHROPIC_MODEL first (where we now write
    // it). Fall back to the top-level `model` for settings.json files
    // produced by older Termory versions — keeps the reverse match
    // working during the transition.
    let model = env
        .and_then(|e| e.get("ANTHROPIC_MODEL"))
        .and_then(|v| v.as_str())
        .map(str::to_string)
        .or_else(|| {
            root.get("model")
                .and_then(|v| v.as_str())
                .map(str::to_string)
        });

    // No injection → Official
    if base_url.is_none() && chosen_key.is_none() {
        return Ok(ActiveState {
            app: CliApp::Claude,
            kind: ActiveKind::Official,
            matched_provider_id: None,
            live_snapshot: None,
            live_path,
        });
    }

    let snapshot = LiveSnapshot {
        base_url: base_url.clone(),
        api_key_masked: chosen_key.as_deref().map(mask_secret),
        model: model.clone(),
    };

    // Match against the user's provider list.
    let matched = providers.iter().find(|p| {
        p.app == CliApp::Claude
            && p.kind == ProviderKind::Custom
            && string_match(&p.base_url, base_url.as_deref())
            && string_match(&p.api_key, chosen_key.as_deref())
    });

    Ok(ActiveState {
        app: CliApp::Claude,
        kind: if matched.is_some() {
            ActiveKind::Custom
        } else {
            ActiveKind::Unmanaged
        },
        matched_provider_id: matched.map(|p| p.id.clone()),
        live_snapshot: Some(snapshot),
        live_path,
    })
}

// ===================================================================
// Codex
// ===================================================================
//
// Files: ~/.codex/auth.json + ~/.codex/config.toml
// Custom: writes auth.json's OPENAI_API_KEY + config.toml's
//         model_provider + [model_providers.termory] block + model
// Official: removes model_provider, removes [model_providers.termory],
//           removes OPENAI_API_KEY from auth.json
// Reverse: read config.toml's model_provider. If "termory" (or another
//          non-reserved id we wrote), read its base_url + model and
//          match. Otherwise Official.

fn codex_dir() -> Result<PathBuf, Box<dyn Error>> {
    Ok(home()?.join(".codex"))
}

fn codex_auth_path() -> Result<PathBuf, Box<dyn Error>> {
    Ok(codex_dir()?.join("auth.json"))
}

fn codex_config_path() -> Result<PathBuf, Box<dyn Error>> {
    Ok(codex_dir()?.join("config.toml"))
}

fn codex_provider_id_or_default(_p: &Provider) -> String {
    // Internal stable id, never user-configurable. Avoids Codex's
    // reserved built-in names (openai/amazon-bedrock/ollama/lmstudio)
    // so the merge in `merge_configured_model_providers` actually
    // takes our block (see model-provider-info/src/lib.rs:442-473).
    TERMORY_PROVIDER_ID.to_string()
}

fn activate_codex(p: &Provider) -> Result<(), Box<dyn Error>> {
    let provider_id = codex_provider_id_or_default(p);

    // Step 1: write auth.json.
    //
    // We MERGE — do NOT overwrite — so a previously run `codex login`
    // (OAuth) survives a round-trip through Custom Provider mode.
    // Concretely: we set `auth_mode = "apikey"` (Codex `resolved_mode()`
    // checks this first per login/src/auth/manager.rs:980-988, so it
    // takes precedence over any existing OAuth `tokens` field) and
    // write `OPENAI_API_KEY`, but leave `tokens / last_refresh /
    // agent_identity` untouched. When the user later deactivates back
    // to Official, deactivate_codex removes only auth_mode +
    // OPENAI_API_KEY; the preserved tokens then make resolved_mode()
    // fall back to ChatGPT mode → user stays logged in.
    //
    // This deliberately differs from the official `login_with_api_key`
    // (which nulls tokens) because Termory's "switch to API platform"
    // is a temporary swap, not a permanent OAuth abandonment.
    //
    // Saved with rollback so a failure on step 2 (config.toml) doesn't
    // strand auth.json in a half-written state.
    let auth_path = codex_auth_path()?;
    let prev_auth_bytes = if auth_path.exists() {
        Some(fs::read(&auth_path)?)
    } else {
        None
    };
    let mut auth_root = load_json_object(&auth_path)?;
    auth_root.insert("auth_mode".into(), JsonValue::String("apikey".into()));
    if !p.api_key.is_empty() {
        auth_root.insert(
            "OPENAI_API_KEY".into(),
            JsonValue::String(p.api_key.clone()),
        );
    } else {
        auth_root.remove("OPENAI_API_KEY");
    }
    write_json_object(&auth_path, &auth_root)?;

    // Step 2: write config.toml.
    //
    // Field choices verified against Codex source + cc-switch:
    //   - `requires_openai_auth = true` makes Codex load
    //     auth.json via AuthManager. Without this, TUI returns
    //     LoginStatus::NotAuthenticated (see Codex tui/src/lib.rs:1817).
    //   - `wire_api = "responses"` — "chat" is removed in current Codex
    //     (CHAT_WIRE_API_REMOVED_ERROR, model-provider-info/src/lib.rs:45).
    //   - We DO NOT set `env_key`. If `env_key` is set and the
    //     environment variable is missing, Codex errors out before
    //     falling back to auth.json (model-provider/src/auth.rs:92-103
    //     + model-provider-info/src/lib.rs:272-288).
    let config_result = (|| -> Result<(), Box<dyn Error>> {
        let config_path = codex_config_path()?;
        let mut doc = load_toml_document(&config_path)?;
        doc["model_provider"] = toml_value(provider_id.as_str());
        if !p.model.is_empty() {
            doc["model"] = toml_value(p.model.as_str());
        }
        if doc.get("model_providers").is_none() {
            doc["model_providers"] = toml_edit::table();
        }
        let providers_table = doc["model_providers"]
            .as_table_mut()
            .ok_or("model_providers must be a TOML table")?;
        if !providers_table.contains_key(&provider_id) {
            providers_table[&provider_id] = toml_edit::table();
        }
        let block = providers_table[&provider_id]
            .as_table_mut()
            .ok_or("model_providers.<id> must be a table")?;
        if !p.name.is_empty() {
            block["name"] = toml_value(p.name.as_str());
        }
        if !p.base_url.is_empty() {
            block["base_url"] = toml_value(p.base_url.as_str());
        }
        block["wire_api"] = toml_value("responses");
        block["requires_openai_auth"] = toml_value(true);
        // Defensive: scrub any pre-existing env_key on this block to
        // avoid Codex preferring an empty env var over auth.json.
        block.remove("env_key");
        write_text_file(&config_path, &doc.to_string())
    })();

    if let Err(err) = config_result {
        // Rollback auth.json to previous state.
        if let Some(bytes) = prev_auth_bytes {
            let _ = fs::write(&auth_path, bytes);
        } else {
            let _ = fs::remove_file(&auth_path);
        }
        return Err(err);
    }
    Ok(())
}

fn deactivate_codex() -> Result<(), Box<dyn Error>> {
    // Clear ApiKey-mode fields from auth.json, but preserve any
    // ChatGPT OAuth credentials. We only touch the API key path —
    // if the user previously ran `codex login` and has tokens in
    // auth.json, those keep working after we deactivate.
    let auth_path = codex_auth_path()?;
    if auth_path.exists() {
        let mut auth_root = load_json_object(&auth_path)?;
        let was_apikey_mode = auth_root
            .get("auth_mode")
            .and_then(|v| v.as_str())
            .map(|s| s.eq_ignore_ascii_case("apikey"))
            .unwrap_or(false);
        let has_tokens = matches!(auth_root.get("tokens"), Some(JsonValue::Object(_)));
        auth_root.remove("OPENAI_API_KEY");
        if was_apikey_mode {
            // Remove the explicit ApiKey marker. If a ChatGPT token
            // is also present (rare but possible), `resolved_mode()`
            // will fall back to ChatGPT mode via the presence of
            // `tokens`. Otherwise Codex falls through to
            // "NotAuthenticated" and the user runs `codex login`.
            auth_root.remove("auth_mode");
        }
        // If the file is now effectively empty, delete it so Codex
        // starts cleanly. "Effectively empty" = only null fields left.
        let effectively_empty =
            !has_tokens && auth_root.iter().all(|(_, v)| matches!(v, JsonValue::Null));
        if effectively_empty {
            let _ = fs::remove_file(&auth_path);
        } else {
            write_json_object(&auth_path, &auth_root)?;
        }
    }

    // Strip model_provider + matching provider block from config.toml.
    // Only remove provider blocks Termory could have written
    // (non-reserved id); never touch the user's openai/bedrock/ollama
    // blocks even if they happen to be the current selection.
    let config_path = codex_config_path()?;
    if !config_path.exists() {
        return Ok(());
    }
    let mut doc = load_toml_document(&config_path)?;
    let provider_id = doc
        .get("model_provider")
        .and_then(|item| item.as_str())
        .map(str::to_string);
    if let Some(id) = provider_id.as_deref() {
        let is_built_in = CODEX_RESERVED_IDS
            .iter()
            .any(|r| r.eq_ignore_ascii_case(id));
        if !is_built_in {
            doc.as_table_mut().remove("model_provider");
            doc.as_table_mut().remove("model");
            if let Some(providers) = doc
                .get_mut("model_providers")
                .and_then(|i| i.as_table_mut())
            {
                providers.remove(id);
                if providers.is_empty() {
                    doc.as_table_mut().remove("model_providers");
                }
            }
        }
    }
    write_text_file(&config_path, &doc.to_string())
}

fn read_active_codex(providers: &[Provider]) -> Result<ActiveState, Box<dyn Error>> {
    let config_path = codex_config_path()?;
    let live_path = config_path.display().to_string();
    if !config_path.exists() {
        return Ok(ActiveState {
            app: CliApp::Codex,
            kind: ActiveKind::Official,
            matched_provider_id: None,
            live_snapshot: None,
            live_path,
        });
    }
    let text = fs::read_to_string(&config_path)?;
    let doc = text.parse::<DocumentMut>()?;
    let active_id = doc
        .get("model_provider")
        .and_then(|item| item.as_str())
        .map(str::to_string);
    let model = doc
        .get("model")
        .and_then(|item| item.as_str())
        .map(str::to_string);

    // No model_provider, or it points to a built-in id → Official.
    let Some(active_id) = active_id else {
        return Ok(ActiveState {
            app: CliApp::Codex,
            kind: ActiveKind::Official,
            matched_provider_id: None,
            live_snapshot: None,
            live_path,
        });
    };
    if CODEX_RESERVED_IDS
        .iter()
        .any(|r| r.eq_ignore_ascii_case(&active_id))
    {
        return Ok(ActiveState {
            app: CliApp::Codex,
            kind: ActiveKind::Official,
            matched_provider_id: None,
            live_snapshot: None,
            live_path,
        });
    }

    let base_url = doc
        .get("model_providers")
        .and_then(|i| i.as_table())
        .and_then(|t| t.get(active_id.as_str()))
        .and_then(|i| i.as_table())
        .and_then(|t| t.get("base_url"))
        .and_then(Item::as_str)
        .map(str::to_string);
    let api_key = read_codex_auth_key()?;
    let snapshot = LiveSnapshot {
        base_url: base_url.clone(),
        api_key_masked: api_key.as_deref().map(mask_secret),
        model: model.clone(),
    };

    let matched = providers.iter().find(|p| {
        p.app == CliApp::Codex
            && p.kind == ProviderKind::Custom
            && string_match(&p.base_url, base_url.as_deref())
            && string_match(&p.api_key, api_key.as_deref())
    });

    Ok(ActiveState {
        app: CliApp::Codex,
        kind: if matched.is_some() {
            ActiveKind::Custom
        } else {
            ActiveKind::Unmanaged
        },
        matched_provider_id: matched.map(|p| p.id.clone()),
        live_snapshot: Some(snapshot),
        live_path,
    })
}

fn read_codex_auth_key() -> Result<Option<String>, Box<dyn Error>> {
    let path = codex_auth_path()?;
    if !path.exists() {
        return Ok(None);
    }
    let root = load_json_object(&path)?;
    Ok(root
        .get("OPENAI_API_KEY")
        .and_then(|v| v.as_str())
        .map(str::to_string))
}

// ===================================================================
// Gemini CLI
// ===================================================================
//
// File: ~/.gemini/.env  (dotenv; Gemini auto-loads on startup)
// Custom: writes GOOGLE_GEMINI_BASE_URL + GEMINI_API_KEY + GEMINI_MODEL
// Official: removes those three
// Reverse: parse .env; if any of them present → Custom-ish.
//
// OAuth credentials live in separate files (`~/.gemini/oauth_creds.json`
// and `~/.gemini/google_accounts.json`, see `storage.ts:22, 87`) which
// we never touch — switching to a Custom provider and back leaves
// `gemini auth` login intact automatically.

fn gemini_env_path() -> Result<PathBuf, Box<dyn Error>> {
    Ok(home()?.join(".gemini").join(".env"))
}

fn activate_gemini(p: &Provider) -> Result<(), Box<dyn Error>> {
    let path = gemini_env_path()?;
    let mut map = parse_dotenv(&path)?;
    // Each field: write when non-empty, strip when empty. Empty-string
    // strip prevents stale values from a prior Custom provider from
    // sticking around after the user clears the field.
    for (key, value) in [
        ("GOOGLE_GEMINI_BASE_URL", &p.base_url),
        ("GEMINI_API_KEY", &p.api_key),
        // Gemini CLI reads GEMINI_MODEL with priority just below
        // `--model` (see `cli/src/config/config.ts:836-837`:
        // `argv.model || process.env['GEMINI_MODEL'] || settings.model?.name`).
        // Matches cc-switch's preset shape (provider.rs:653-658).
        ("GEMINI_MODEL", &p.model),
    ] {
        if value.is_empty() {
            map.remove(key);
        } else {
            map.insert(key.into(), value.clone());
        }
    }
    write_dotenv(&path, &map)
}

fn deactivate_gemini() -> Result<(), Box<dyn Error>> {
    let path = gemini_env_path()?;
    if !path.exists() {
        return Ok(());
    }
    let mut map = parse_dotenv(&path)?;
    map.remove("GOOGLE_GEMINI_BASE_URL");
    map.remove("GEMINI_API_KEY");
    map.remove("GEMINI_MODEL");
    write_dotenv(&path, &map)
}

fn read_active_gemini(providers: &[Provider]) -> Result<ActiveState, Box<dyn Error>> {
    let path = gemini_env_path()?;
    let live_path = path.display().to_string();
    if !path.exists() {
        return Ok(ActiveState {
            app: CliApp::Gemini,
            kind: ActiveKind::Official,
            matched_provider_id: None,
            live_snapshot: None,
            live_path,
        });
    }
    let map = parse_dotenv(&path)?;
    let base_url = map.get("GOOGLE_GEMINI_BASE_URL").cloned();
    let api_key = map.get("GEMINI_API_KEY").cloned();
    let model = map.get("GEMINI_MODEL").cloned();
    if base_url.is_none() && api_key.is_none() && model.is_none() {
        return Ok(ActiveState {
            app: CliApp::Gemini,
            kind: ActiveKind::Official,
            matched_provider_id: None,
            live_snapshot: None,
            live_path,
        });
    }
    let snapshot = LiveSnapshot {
        base_url: base_url.clone(),
        api_key_masked: api_key.as_deref().map(mask_secret),
        model,
    };
    let matched = providers.iter().find(|p| {
        p.app == CliApp::Gemini
            && p.kind == ProviderKind::Custom
            && string_match(&p.base_url, base_url.as_deref())
            && string_match(&p.api_key, api_key.as_deref())
    });
    Ok(ActiveState {
        app: CliApp::Gemini,
        kind: if matched.is_some() {
            ActiveKind::Custom
        } else {
            ActiveKind::Unmanaged
        },
        matched_provider_id: matched.map(|p| p.id.clone()),
        live_snapshot: Some(snapshot),
        live_path,
    })
}

// ===================================================================
// OpenCode
// ===================================================================
//
// File: ~/.config/opencode/opencode.json (additive — multiple
//       provider.X blocks coexist)
// Custom: writes provider.<provider_id>.options.{baseURL,apiKey} +
//         top-level model = "<provider_id>/<model>"
// Official: removes provider.<termory> (or any other Custom we
//           added) and removes top-level model
// Reverse: parse top-level model. If it starts with a Custom-id we
//          recognize, read its options and match.

fn opencode_config_path() -> Result<PathBuf, Box<dyn Error>> {
    // xdg-basedir uses ~/.config on every platform (verified at
    // .audit-sources/opencode/packages/core/src/global.ts:12 +
    // xdg-basedir source). Don't use dirs::config_dir() — it returns
    // ~/Library/Application Support on macOS, which is wrong.
    Ok(home()?
        .join(".config")
        .join("opencode")
        .join("opencode.json"))
}

fn opencode_provider_id_or_default(_p: &Provider) -> String {
    // Internal stable id under `provider.<id>` in opencode.json.
    // Not user-configurable — keeps the JSON shape consistent so
    // deactivate can find and remove it.
    TERMORY_PROVIDER_ID.to_string()
}

fn activate_opencode(p: &Provider, _all: &[Provider]) -> Result<(), Box<dyn Error>> {
    // Model is required for OpenCode: `defaultModel()` falls through
    // to "first provider's first model" (`provider.ts:1801-1806`) and
    // throws "no models found" when our provider block has empty
    // `models: {}`. A half-written block would crash OpenCode at
    // startup, so refuse the activation up front.
    if p.model.trim().is_empty() {
        return Err(
            "OpenCode provider requires a Model. Fill the Model field (e.g. \
             gpt-5, claude-opus-4-7) before activating."
                .into(),
        );
    }

    let path = opencode_config_path()?;
    let mut root = load_json_object(&path)?;
    let provider_id = opencode_provider_id_or_default(p);

    let provider_map = ensure_json_object(&mut root, "provider")?;
    let block = ensure_object_at(provider_map, &provider_id);

    // `npm` selects the AI SDK adapter OpenCode loads at runtime
    // (`provider.ts:92-100`). Default to `@ai-sdk/openai-compatible`
    // — works for ~all third-party gateways that speak the OpenAI
    // wire format (PackyCode / DMXAPI / etc.). Users on a real
    // Anthropic-native endpoint can post-edit opencode.json.
    block.insert(
        "npm".into(),
        JsonValue::String("@ai-sdk/openai-compatible".into()),
    );
    // Display name shown in OpenCode's provider picker.
    if !p.name.is_empty() {
        block.insert("name".into(), JsonValue::String(p.name.clone()));
    }

    let options = ensure_object_at(block, "options");
    if !p.base_url.is_empty() {
        options.insert("baseURL".into(), JsonValue::String(p.base_url.clone()));
    } else {
        options.remove("baseURL");
    }
    if !p.api_key.is_empty() {
        options.insert("apiKey".into(), JsonValue::String(p.api_key.clone()));
    } else {
        options.remove("apiKey");
    }

    // `provider.X.models[Y]` MUST exist or OpenCode's `getModel()`
    // throws ModelNotFoundError at startup (`provider.ts:1668-1675`).
    let models = ensure_object_at(block, "models");
    let mut entry = serde_json::Map::new();
    entry.insert("name".into(), JsonValue::String(p.model.clone()));
    models.insert(p.model.clone(), JsonValue::Object(entry));

    // Top-level `model = "<provider>/<model>"` makes this Termory
    // provider the default — without it, OpenCode falls back to the
    // recent-models state file (which is empty on first launch).
    root.insert(
        "model".into(),
        JsonValue::String(format!("{}/{}", provider_id, p.model)),
    );

    write_json_object(&path, &root)
}

fn deactivate_opencode(providers: &[Provider]) -> Result<(), Box<dyn Error>> {
    let path = opencode_config_path()?;
    if !path.exists() {
        return Ok(());
    }
    let mut root = load_json_object(&path)?;
    // Remove every provider block that any of our Custom OpenCode
    // providers might have written to. Conservative: collect ids,
    // remove all of them, plus TERMORY_PROVIDER_ID.
    let mut ids_to_strip = vec![TERMORY_PROVIDER_ID.to_string()];
    for p in providers {
        if p.app == CliApp::Opencode && p.kind == ProviderKind::Custom {
            ids_to_strip.push(opencode_provider_id_or_default(p));
        }
    }
    if let Some(JsonValue::Object(map)) = root.get_mut("provider") {
        for id in &ids_to_strip {
            map.remove(id);
        }
        if map.is_empty() {
            root.remove("provider");
        }
    }
    root.remove("model");
    write_json_object(&path, &root)
}

fn read_active_opencode(providers: &[Provider]) -> Result<ActiveState, Box<dyn Error>> {
    let path = opencode_config_path()?;
    let live_path = path.display().to_string();
    if !path.exists() {
        return Ok(ActiveState {
            app: CliApp::Opencode,
            kind: ActiveKind::Official,
            matched_provider_id: None,
            live_snapshot: None,
            live_path,
        });
    }
    let root = load_json_object(&path)?;
    let model = root
        .get("model")
        .and_then(|v| v.as_str())
        .map(str::to_string);
    let Some(model_str) = model.clone() else {
        return Ok(ActiveState {
            app: CliApp::Opencode,
            kind: ActiveKind::Official,
            matched_provider_id: None,
            live_snapshot: None,
            live_path,
        });
    };
    let (active_id, bare_model) = match model_str.split_once('/') {
        Some((id, rest)) => (id.to_string(), rest.to_string()),
        None => {
            // No prefix → OpenCode picks default → Official.
            return Ok(ActiveState {
                app: CliApp::Opencode,
                kind: ActiveKind::Official,
                matched_provider_id: None,
                live_snapshot: None,
                live_path,
            });
        }
    };
    let block = root
        .get("provider")
        .and_then(|v| v.as_object())
        .and_then(|m| m.get(active_id.as_str()))
        .and_then(|v| v.as_object());
    let options = block
        .and_then(|b| b.get("options"))
        .and_then(|v| v.as_object());
    let base_url = options
        .and_then(|o| o.get("baseURL"))
        .and_then(|v| v.as_str())
        .map(str::to_string);
    let api_key = options
        .and_then(|o| o.get("apiKey"))
        .and_then(|v| v.as_str())
        .map(str::to_string);
    if base_url.is_none() && api_key.is_none() {
        // The model has a prefix but the provider block has no
        // Termory-injected baseURL/apiKey → it's a built-in models.dev
        // provider, treat as Official.
        return Ok(ActiveState {
            app: CliApp::Opencode,
            kind: ActiveKind::Official,
            matched_provider_id: None,
            live_snapshot: None,
            live_path,
        });
    }
    let snapshot = LiveSnapshot {
        base_url: base_url.clone(),
        api_key_masked: api_key.as_deref().map(mask_secret),
        model: Some(bare_model.clone()),
    };
    let matched = providers.iter().find(|p| {
        p.app == CliApp::Opencode
            && p.kind == ProviderKind::Custom
            && string_match(&p.base_url, base_url.as_deref())
            && string_match(&p.api_key, api_key.as_deref())
    });
    Ok(ActiveState {
        app: CliApp::Opencode,
        kind: if matched.is_some() {
            ActiveKind::Custom
        } else {
            ActiveKind::Unmanaged
        },
        matched_provider_id: matched.map(|p| p.id.clone()),
        live_snapshot: Some(snapshot),
        live_path,
    })
}

// ===================================================================
// Test API
// ===================================================================

/// Lightweight connectivity check — calls `GET {base_url}/models` (or
/// the Gemini variant) with the provider's API key. Counts any 2xx
/// as success.
pub async fn test_provider(p: &Provider) -> TestResult {
    let start = std::time::Instant::now();
    let client = match reqwest::Client::builder()
        .timeout(Duration::from_secs(10))
        .build()
    {
        Ok(c) => c,
        Err(err) => {
            return TestResult {
                ok: false,
                status: None,
                latency_ms: start.elapsed().as_millis(),
                message: format!("HTTP client init failed: {err}"),
            };
        }
    };

    let (url, gemini_query) = match p.app {
        CliApp::Gemini => {
            let base = p
                .base_url
                .trim_end_matches('/')
                .trim_end_matches("/v1beta")
                .to_string();
            (
                format!("{}/v1beta/models", base),
                Some(("key", p.api_key.clone())),
            )
        }
        _ => {
            let base = p.base_url.trim_end_matches('/');
            // Most providers expose /v1/models. Allow base URLs that
            // already include /v1 (e.g. https://api.openai.com/v1).
            let url = if base.ends_with("/v1") {
                format!("{base}/models")
            } else {
                format!("{base}/v1/models")
            };
            (url, None)
        }
    };

    let mut req = client.get(&url);
    if matches!(p.app, CliApp::Gemini) {
        if let Some((k, v)) = gemini_query {
            req = req.query(&[(k, v)]);
        }
    } else if !p.api_key.is_empty() {
        req = req.bearer_auth(&p.api_key);
    }

    let response = match req.send().await {
        Ok(r) => r,
        Err(err) => {
            return TestResult {
                ok: false,
                status: None,
                latency_ms: start.elapsed().as_millis(),
                message: format!("Request failed: {err}"),
            };
        }
    };
    let status = response.status();
    TestResult {
        ok: status.is_success(),
        status: Some(status.as_u16()),
        latency_ms: start.elapsed().as_millis(),
        message: if status.is_success() {
            "OK".into()
        } else {
            status.canonical_reason().unwrap_or("HTTP error").into()
        },
    }
}

/// Hit the provider's models endpoint and return the model id list.
/// Same routing as `test_provider`:
///   * Gemini  → `GET {base}/v1beta/models?key={apiKey}`,response
///               `{ models: [{ name: "models/gemini-2.5-pro", ... }] }`
///               → strip the `models/` prefix to get the bare id.
///   * others  → `GET {base}/v1/models` with Bearer auth, response
///               `{ data: [{ id: "gpt-5", ... }, ...] }`.
/// Gracefully returns `ok=false` + diagnostic message on any failure
/// (network, HTTP error, JSON shape mismatch) — the frontend treats
/// that as "fetch failed, user can still type manually".
pub async fn fetch_models(p: &Provider) -> ModelListResult {
    let client = match reqwest::Client::builder()
        .timeout(Duration::from_secs(10))
        .build()
    {
        Ok(c) => c,
        Err(err) => {
            return ModelListResult {
                ok: false,
                models: Vec::new(),
                status: None,
                message: format!("HTTP client init failed: {err}"),
            };
        }
    };

    let (url, gemini_query) = match p.app {
        CliApp::Gemini => {
            let base = p
                .base_url
                .trim_end_matches('/')
                .trim_end_matches("/v1beta")
                .to_string();
            (
                format!("{}/v1beta/models", base),
                Some(("key", p.api_key.clone())),
            )
        }
        _ => {
            let base = p.base_url.trim_end_matches('/');
            let url = if base.ends_with("/v1") {
                format!("{base}/models")
            } else {
                format!("{base}/v1/models")
            };
            (url, None)
        }
    };

    let mut req = client.get(&url);
    if matches!(p.app, CliApp::Gemini) {
        if let Some((k, v)) = gemini_query {
            req = req.query(&[(k, v)]);
        }
    } else if !p.api_key.is_empty() {
        req = req.bearer_auth(&p.api_key);
    }

    let response = match req.send().await {
        Ok(r) => r,
        Err(err) => {
            return ModelListResult {
                ok: false,
                models: Vec::new(),
                status: None,
                message: format!("Request failed: {err}"),
            };
        }
    };
    let status = response.status();
    let status_u16 = status.as_u16();
    if !status.is_success() {
        return ModelListResult {
            ok: false,
            models: Vec::new(),
            status: Some(status_u16),
            message: status
                .canonical_reason()
                .unwrap_or("HTTP error")
                .to_string(),
        };
    }
    let body: serde_json::Value = match response.json().await {
        Ok(v) => v,
        Err(err) => {
            return ModelListResult {
                ok: false,
                models: Vec::new(),
                status: Some(status_u16),
                message: format!("Response is not JSON: {err}"),
            };
        }
    };

    let mut models = match p.app {
        CliApp::Gemini => body
            .get("models")
            .and_then(|v| v.as_array())
            .map(|arr| {
                arr.iter()
                    .filter_map(|m| m.get("name").and_then(|n| n.as_str()))
                    .map(|name| name.trim_start_matches("models/").to_string())
                    .collect::<Vec<_>>()
            })
            .unwrap_or_default(),
        _ => body
            .get("data")
            .and_then(|v| v.as_array())
            .map(|arr| {
                arr.iter()
                    .filter_map(|m| m.get("id").and_then(|n| n.as_str()))
                    .map(|s| s.to_string())
                    .collect::<Vec<_>>()
            })
            .unwrap_or_default(),
    };
    models.sort();
    models.dedup();

    if models.is_empty() {
        return ModelListResult {
            ok: false,
            models,
            status: Some(status_u16),
            message: "Endpoint returned no models".into(),
        };
    }
    ModelListResult {
        ok: true,
        models,
        status: Some(status_u16),
        message: "OK".into(),
    }
}

// ===================================================================
// Helpers
// ===================================================================

fn home() -> Result<PathBuf, Box<dyn Error>> {
    dirs::home_dir().ok_or_else(|| "home directory not available".into())
}

fn ensure_parent_dir(path: &Path) -> std::io::Result<()> {
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent)?;
    }
    Ok(())
}

fn atomic_write(path: &Path, bytes: &[u8]) -> Result<(), Box<dyn Error>> {
    ensure_parent_dir(path)?;
    let mut tmp_name = path.file_name().ok_or("invalid path")?.to_owned();
    let nanos = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap_or_default()
        .as_nanos();
    tmp_name.push(format!(".tmp.{nanos}"));
    let tmp_path = path.with_file_name(tmp_name);
    {
        let mut f = fs::File::create(&tmp_path)?;
        f.write_all(bytes)?;
        f.sync_all()?;
    }
    fs::rename(&tmp_path, path)?;
    Ok(())
}

fn write_text_file(path: &Path, contents: &str) -> Result<(), Box<dyn Error>> {
    atomic_write(path, contents.as_bytes())
}

fn load_json_object(path: &Path) -> Result<Map<String, JsonValue>, Box<dyn Error>> {
    if !path.exists() {
        return Ok(Map::new());
    }
    let text = fs::read_to_string(path)?;
    if text.trim().is_empty() {
        return Ok(Map::new());
    }
    let parsed: JsonValue = serde_json::from_str(&text)?;
    match parsed {
        JsonValue::Object(map) => Ok(map),
        _ => Err(format!("{}: root must be a JSON object", path.display()).into()),
    }
}

fn write_json_object(path: &Path, root: &Map<String, JsonValue>) -> Result<(), Box<dyn Error>> {
    let serialized = serde_json::to_string_pretty(&JsonValue::Object(root.clone()))?;
    atomic_write(path, serialized.as_bytes())
}

fn ensure_json_object<'a>(
    root: &'a mut Map<String, JsonValue>,
    key: &str,
) -> Result<&'a mut Map<String, JsonValue>, Box<dyn Error>> {
    if !root.contains_key(key) {
        root.insert(key.into(), JsonValue::Object(Map::new()));
    }
    match root.get_mut(key) {
        Some(JsonValue::Object(map)) => Ok(map),
        _ => Err(format!("`{key}` is not a JSON object").into()),
    }
}

fn ensure_object_at<'a>(
    parent: &'a mut Map<String, JsonValue>,
    key: &str,
) -> &'a mut Map<String, JsonValue> {
    if !parent.contains_key(key) || !matches!(parent.get(key), Some(JsonValue::Object(_))) {
        parent.insert(key.into(), JsonValue::Object(Map::new()));
    }
    match parent.get_mut(key) {
        Some(JsonValue::Object(map)) => map,
        _ => unreachable!("just inserted an object"),
    }
}

fn load_toml_document(path: &Path) -> Result<DocumentMut, Box<dyn Error>> {
    if !path.exists() {
        return Ok(DocumentMut::new());
    }
    let text = fs::read_to_string(path)?;
    Ok(text.parse::<DocumentMut>()?)
}

fn parse_dotenv(path: &Path) -> Result<std::collections::BTreeMap<String, String>, Box<dyn Error>> {
    use std::collections::BTreeMap;
    let mut map = BTreeMap::new();
    if !path.exists() {
        return Ok(map);
    }
    let text = fs::read_to_string(path)?;
    for line in text.lines() {
        let trimmed = line.trim();
        if trimmed.is_empty() || trimmed.starts_with('#') {
            continue;
        }
        if let Some((k, v)) = trimmed.split_once('=') {
            let key = k.trim().to_string();
            let val = v.trim().to_string();
            if !key.is_empty() && key.chars().all(|c| c.is_alphanumeric() || c == '_') {
                map.insert(key, val);
            }
        }
    }
    Ok(map)
}

fn write_dotenv(
    path: &Path,
    map: &std::collections::BTreeMap<String, String>,
) -> Result<(), Box<dyn Error>> {
    let body = map
        .iter()
        .map(|(k, v)| format!("{k}={v}"))
        .collect::<Vec<_>>()
        .join("\n");
    let final_body = if body.is_empty() {
        String::new()
    } else {
        format!("{body}\n")
    };
    ensure_parent_dir(path)?;
    atomic_write(path, final_body.as_bytes())?;
    // Restrict permissions on Unix: API keys live in this file.
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        let mut perms = fs::metadata(path)?.permissions();
        perms.set_mode(0o600);
        fs::set_permissions(path, perms)?;
        if let Some(parent) = path.parent() {
            let mut dperm = fs::metadata(parent)?.permissions();
            dperm.set_mode(0o700);
            fs::set_permissions(parent, dperm)?;
        }
    }
    Ok(())
}

fn mask_secret(value: &str) -> String {
    if value.len() <= 8 {
        "•".repeat(value.len())
    } else {
        format!(
            "{}{}{}",
            &value[..4],
            "•".repeat(value.len() - 8),
            &value[value.len() - 4..]
        )
    }
}

fn string_match(provider_value: &str, live_value: Option<&str>) -> bool {
    let live = live_value.unwrap_or("");
    provider_value.trim() == live.trim()
}

// ===================================================================
// Tests
// ===================================================================

#[cfg(test)]
mod tests {
    use super::*;
    use crate::testutils::HOME_LOCK;

    struct HomeOverride {
        prev: Option<std::ffi::OsString>,
    }
    impl HomeOverride {
        fn new(p: &Path) -> Self {
            let prev = std::env::var_os("HOME");
            std::env::set_var("HOME", p);
            HomeOverride { prev }
        }
    }
    impl Drop for HomeOverride {
        fn drop(&mut self) {
            match &self.prev {
                Some(v) => std::env::set_var("HOME", v),
                None => std::env::remove_var("HOME"),
            }
        }
    }

    fn tempdir(tag: &str) -> PathBuf {
        let mut dir = std::env::temp_dir();
        let nanos = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_nanos();
        dir.push(format!("termory-providers-{tag}-{nanos}"));
        fs::create_dir_all(&dir).unwrap();
        dir
    }

    fn make_provider(app: CliApp, name: &str, base: &str, key: &str) -> Provider {
        Provider {
            id: format!("test-{name}"),
            app,
            kind: ProviderKind::Custom,
            name: name.into(),
            base_url: base.into(),
            api_key: key.into(),
            model: "test-model".into(),
            claude_haiku_model: String::new(),
            claude_sonnet_model: String::new(),
            claude_opus_model: String::new(),
        }
    }

    #[test]
    fn claude_activate_and_reverse_roundtrip() {
        let _g = HOME_LOCK.lock().unwrap();
        let tmp = tempdir("claude-rt");
        let _home = HomeOverride::new(&tmp);
        // Existing unrelated settings preserved.
        fs::create_dir_all(tmp.join(".claude")).unwrap();
        fs::write(
            tmp.join(".claude/settings.json"),
            r#"{"permissions": {"foo": true}, "env": {"OTHER": "x"}}"#,
        )
        .unwrap();
        let p = make_provider(
            CliApp::Claude,
            "Anthropic-thirdparty",
            "https://api.x.io",
            "sk-secret",
        );
        activate(&p, &[p.clone()]).unwrap();
        let state = read_active_state(CliApp::Claude, &[p.clone()]).unwrap();
        assert_eq!(state.kind, ActiveKind::Custom);
        assert_eq!(
            state.matched_provider_id.as_deref(),
            Some("test-Anthropic-thirdparty")
        );
        // Unrelated keys preserved
        let after: JsonValue =
            serde_json::from_str(&fs::read_to_string(tmp.join(".claude/settings.json")).unwrap())
                .unwrap();
        assert_eq!(
            after.pointer("/permissions/foo").and_then(|v| v.as_bool()),
            Some(true)
        );
        assert_eq!(
            after.pointer("/env/OTHER").and_then(|v| v.as_str()),
            Some("x")
        );
        assert_eq!(
            after
                .pointer("/env/ANTHROPIC_BASE_URL")
                .and_then(|v| v.as_str()),
            Some("https://api.x.io")
        );

        // Model written into env.ANTHROPIC_MODEL (matches cc-switch
        // preset shape and Claude's auth priority chain). Top-level
        // `model` must NOT be set — env wins anyway, and writing both
        // forks the source of truth.
        assert_eq!(
            after
                .pointer("/env/ANTHROPIC_MODEL")
                .and_then(|v| v.as_str()),
            Some("test-model")
        );
        assert!(
            after.get("model").is_none(),
            "top-level model must not be set"
        );

        // Deactivate restores Official + leaves unrelated env keys.
        deactivate(CliApp::Claude, &[p.clone()]).unwrap();
        let state2 = read_active_state(CliApp::Claude, &[p.clone()]).unwrap();
        assert_eq!(state2.kind, ActiveKind::Official);
        let after2: JsonValue =
            serde_json::from_str(&fs::read_to_string(tmp.join(".claude/settings.json")).unwrap())
                .unwrap();
        assert_eq!(
            after2.pointer("/permissions/foo").and_then(|v| v.as_bool()),
            Some(true)
        );
        assert_eq!(
            after2.pointer("/env/OTHER").and_then(|v| v.as_str()),
            Some("x")
        );
        assert!(after2.pointer("/env/ANTHROPIC_BASE_URL").is_none());
        assert!(after2.pointer("/env/ANTHROPIC_MODEL").is_none());
    }

    #[test]
    fn claude_oauth_login_then_activate_api_then_deactivate_keeps_credentials() {
        // Claude stores OAuth credentials in a separate file
        // (`~/.claude/.credentials.json`, see `src/utils/auth.ts:1323`)
        // that we never touch. Confirm activate → deactivate leaves
        // that file byte-identical, so the user stays logged in.
        let _g = HOME_LOCK.lock().unwrap();
        let tmp = tempdir("claude-oauth-keep");
        let _home = HomeOverride::new(&tmp);
        fs::create_dir_all(tmp.join(".claude")).unwrap();

        // Stage 1: user ran `claude login` — OAuth tokens persisted.
        let creds_contents = r#"{
          "claudeAiOauth": {
            "accessToken": "at-original",
            "refreshToken": "rt-original",
            "expiresAt": 9999999999000
          }
        }"#;
        fs::write(tmp.join(".claude/.credentials.json"), creds_contents).unwrap();

        // Stage 2: activate Custom provider via Termory.
        let p = make_provider(CliApp::Claude, "api-temp", "https://temp.api", "sk-temp");
        activate(&p, &[p.clone()]).unwrap();

        // settings.json now has env injection.
        let settings_after_activate: JsonValue =
            serde_json::from_str(&fs::read_to_string(tmp.join(".claude/settings.json")).unwrap())
                .unwrap();
        assert_eq!(
            settings_after_activate
                .pointer("/env/ANTHROPIC_AUTH_TOKEN")
                .and_then(|v| v.as_str()),
            Some("sk-temp")
        );

        // OAuth credentials file untouched.
        let creds_after_activate =
            fs::read_to_string(tmp.join(".claude/.credentials.json")).unwrap();
        assert_eq!(
            creds_after_activate, creds_contents,
            "OAuth credentials file must survive activate byte-for-byte"
        );

        // Stage 3: deactivate.
        deactivate(CliApp::Claude, &[p.clone()]).unwrap();
        let creds_after_deactivate =
            fs::read_to_string(tmp.join(".claude/.credentials.json")).unwrap();
        assert_eq!(
            creds_after_deactivate, creds_contents,
            "OAuth credentials file must survive deactivate byte-for-byte"
        );

        // settings.json env stripped.
        let settings_after_deactivate: JsonValue =
            serde_json::from_str(&fs::read_to_string(tmp.join(".claude/settings.json")).unwrap())
                .unwrap();
        assert!(settings_after_deactivate
            .pointer("/env/ANTHROPIC_AUTH_TOKEN")
            .is_none());
        assert!(settings_after_deactivate
            .pointer("/env/ANTHROPIC_BASE_URL")
            .is_none());
        assert!(settings_after_deactivate
            .pointer("/env/ANTHROPIC_MODEL")
            .is_none());
    }

    #[test]
    fn claude_reverse_falls_back_to_top_level_model_for_legacy_settings() {
        // settings.json written by older Termory versions put the
        // model at the top level. The active-state reader should
        // still match these correctly.
        let _g = HOME_LOCK.lock().unwrap();
        let tmp = tempdir("claude-legacy-model");
        let _home = HomeOverride::new(&tmp);
        fs::create_dir_all(tmp.join(".claude")).unwrap();
        fs::write(
            tmp.join(".claude/settings.json"),
            r#"{
              "env": {
                "ANTHROPIC_BASE_URL": "https://legacy.example",
                "ANTHROPIC_AUTH_TOKEN": "sk-legacy"
              },
              "model": "legacy-model-id"
            }"#,
        )
        .unwrap();
        let p = make_provider(
            CliApp::Claude,
            "legacy",
            "https://legacy.example",
            "sk-legacy",
        );
        let state = read_active_state(CliApp::Claude, &[p.clone()]).unwrap();
        assert_eq!(state.kind, ActiveKind::Custom);
        assert_eq!(
            state.live_snapshot.unwrap().model.as_deref(),
            Some("legacy-model-id")
        );
    }

    #[test]
    fn claude_activate_writes_per_size_model_envs_when_set() {
        // Routing GPT-5 to Sonnet, Claude-Opus to Opus, and DeepSeek
        // to Haiku — a non-trivial 3P setup. Confirm Termory writes
        // the three ANTHROPIC_DEFAULT_* env vars exactly as Claude
        // Code's `modelOptions.ts` expects.
        let _g = HOME_LOCK.lock().unwrap();
        let tmp = tempdir("claude-multi-model");
        let _home = HomeOverride::new(&tmp);

        let mut p = make_provider(
            CliApp::Claude,
            "multi-route",
            "https://api.x.io",
            "sk-multi",
        );
        p.model = "gpt-5".into();
        p.claude_sonnet_model = "gpt-5".into();
        p.claude_opus_model = "claude-opus-4-7".into();
        p.claude_haiku_model = "deepseek-chat".into();
        activate(&p, &[p.clone()]).unwrap();

        let after: JsonValue =
            serde_json::from_str(&fs::read_to_string(tmp.join(".claude/settings.json")).unwrap())
                .unwrap();
        assert_eq!(
            after
                .pointer("/env/ANTHROPIC_MODEL")
                .and_then(|v| v.as_str()),
            Some("gpt-5")
        );
        assert_eq!(
            after
                .pointer("/env/ANTHROPIC_DEFAULT_SONNET_MODEL")
                .and_then(|v| v.as_str()),
            Some("gpt-5")
        );
        assert_eq!(
            after
                .pointer("/env/ANTHROPIC_DEFAULT_OPUS_MODEL")
                .and_then(|v| v.as_str()),
            Some("claude-opus-4-7")
        );
        assert_eq!(
            after
                .pointer("/env/ANTHROPIC_DEFAULT_HAIKU_MODEL")
                .and_then(|v| v.as_str()),
            Some("deepseek-chat")
        );

        // Deactivate clears all four.
        deactivate(CliApp::Claude, &[p.clone()]).unwrap();
        let after2: JsonValue =
            serde_json::from_str(&fs::read_to_string(tmp.join(".claude/settings.json")).unwrap())
                .unwrap();
        for var in [
            "ANTHROPIC_MODEL",
            "ANTHROPIC_DEFAULT_HAIKU_MODEL",
            "ANTHROPIC_DEFAULT_SONNET_MODEL",
            "ANTHROPIC_DEFAULT_OPUS_MODEL",
        ] {
            assert!(
                after2.pointer(&format!("/env/{var}")).is_none(),
                "{var} must be cleared after deactivate"
            );
        }
    }

    #[test]
    fn claude_activate_skips_empty_sub_models_and_strips_stale_ones() {
        // User clears the Opus override on an existing provider and
        // saves. We must remove ANTHROPIC_DEFAULT_OPUS_MODEL from
        // settings.json — leaving a stale env var would silently
        // override Claude's default Opus next session.
        let _g = HOME_LOCK.lock().unwrap();
        let tmp = tempdir("claude-clear-sub-model");
        let _home = HomeOverride::new(&tmp);
        fs::create_dir_all(tmp.join(".claude")).unwrap();
        fs::write(
            tmp.join(".claude/settings.json"),
            r#"{
              "env": {
                "ANTHROPIC_BASE_URL": "https://api.x.io",
                "ANTHROPIC_AUTH_TOKEN": "sk-stale",
                "ANTHROPIC_DEFAULT_OPUS_MODEL": "stale-opus-route"
              }
            }"#,
        )
        .unwrap();

        let mut p = make_provider(CliApp::Claude, "clear-opus", "https://api.x.io", "sk-stale");
        p.model = "gpt-5".into();
        // claude_opus_model stays empty; we expect the stale value to be removed.
        activate(&p, &[p.clone()]).unwrap();

        let after: JsonValue =
            serde_json::from_str(&fs::read_to_string(tmp.join(".claude/settings.json")).unwrap())
                .unwrap();
        assert!(
            after.pointer("/env/ANTHROPIC_DEFAULT_OPUS_MODEL").is_none(),
            "empty claude_opus_model must strip the stale env var"
        );
        assert_eq!(
            after
                .pointer("/env/ANTHROPIC_MODEL")
                .and_then(|v| v.as_str()),
            Some("gpt-5")
        );
    }

    #[test]
    fn claude_unmanaged_when_external_edit_does_not_match_any_provider() {
        let _g = HOME_LOCK.lock().unwrap();
        let tmp = tempdir("claude-unmanaged");
        let _home = HomeOverride::new(&tmp);
        fs::create_dir_all(tmp.join(".claude")).unwrap();
        // Outside party set an unknown base URL.
        fs::write(
            tmp.join(".claude/settings.json"),
            r#"{"env": {"ANTHROPIC_BASE_URL": "https://unknown.example", "ANTHROPIC_AUTH_TOKEN": "sk-unknown"}}"#,
        )
        .unwrap();
        let known = make_provider(CliApp::Claude, "Other", "https://api.known", "sk-known");
        let state = read_active_state(CliApp::Claude, &[known.clone()]).unwrap();
        assert_eq!(state.kind, ActiveKind::Unmanaged);
        assert!(state.matched_provider_id.is_none());
        let snap = state.live_snapshot.unwrap();
        assert_eq!(snap.base_url.as_deref(), Some("https://unknown.example"));
    }

    #[test]
    fn codex_activate_and_reverse_roundtrip_preserves_unrelated_blocks() {
        let _g = HOME_LOCK.lock().unwrap();
        let tmp = tempdir("codex-rt");
        let _home = HomeOverride::new(&tmp);
        fs::create_dir_all(tmp.join(".codex")).unwrap();
        // Pre-existing config with unrelated mcp_servers block + an
        // unrelated provider block.
        fs::write(
            tmp.join(".codex/config.toml"),
            r#"approval_policy = "untrusted"

[model_providers.openai]
name = "OpenAI"
base_url = "https://api.openai.com/v1"

[mcp_servers.context7]
command = "npx"
"#,
        )
        .unwrap();
        let p = make_provider(
            CliApp::Codex,
            "Custom-codex",
            "https://codex.x.io/v1",
            "sk-codex",
        );
        activate(&p, &[p.clone()]).unwrap();
        let txt = fs::read_to_string(tmp.join(".codex/config.toml")).unwrap();
        // Unrelated stuff preserved.
        assert!(txt.contains("approval_policy"));
        assert!(txt.contains("mcp_servers.context7"));
        assert!(txt.contains("model_providers.openai"));
        // termory block written.
        assert!(txt.contains("[model_providers.termory]"));
        assert!(txt.contains("https://codex.x.io/v1"));

        let state = read_active_state(CliApp::Codex, &[p.clone()]).unwrap();
        assert_eq!(state.kind, ActiveKind::Custom);
        assert_eq!(
            state.matched_provider_id.as_deref(),
            Some("test-Custom-codex")
        );

        // config.toml: termory block has the verified shape
        // (wire_api=responses + requires_openai_auth=true, NO env_key).
        assert!(txt.contains(r#"wire_api = "responses""#));
        assert!(txt.contains("requires_openai_auth = true"));
        assert!(
            !txt.contains("env_key"),
            "env_key would force Codex to use env var only; we must not set it"
        );

        // Auth file: explicit auth_mode=apikey + OPENAI_API_KEY. We
        // do NOT null tokens/last_refresh (unlike official
        // login_with_api_key) — see merge rationale in activate_codex.
        let auth: JsonValue =
            serde_json::from_str(&fs::read_to_string(tmp.join(".codex/auth.json")).unwrap())
                .unwrap();
        assert_eq!(
            auth.get("auth_mode").and_then(|v| v.as_str()),
            Some("apikey")
        );
        assert_eq!(
            auth.get("OPENAI_API_KEY").and_then(|v| v.as_str()),
            Some("sk-codex")
        );

        // Deactivate: model_provider removed, termory block removed,
        // unrelated openai block preserved, mcp_servers preserved.
        // auth.json is effectively empty → file deleted.
        deactivate(CliApp::Codex, &[p.clone()]).unwrap();
        let txt2 = fs::read_to_string(tmp.join(".codex/config.toml")).unwrap();
        assert!(!txt2.contains("[model_providers.termory]"));
        assert!(txt2.contains("model_providers.openai"));
        assert!(txt2.contains("mcp_servers.context7"));
        assert!(!txt2.contains("model_provider ="));
        assert!(
            !tmp.join(".codex/auth.json").exists(),
            "auth.json should be removed when it contained only ApiKey-mode fields"
        );

        let state2 = read_active_state(CliApp::Codex, &[p.clone()]).unwrap();
        assert_eq!(state2.kind, ActiveKind::Official);
    }

    #[test]
    fn codex_oauth_login_then_activate_api_then_deactivate_keeps_oauth() {
        // Three-stage round-trip: user logged into ChatGPT, swaps to
        // a Custom API provider via Termory, then swaps back to
        // Official. The OAuth tokens must survive all three stages so
        // the user doesn't have to re-run `codex login`.
        let _g = HOME_LOCK.lock().unwrap();
        let tmp = tempdir("codex-three-stage");
        let _home = HomeOverride::new(&tmp);
        fs::create_dir_all(tmp.join(".codex")).unwrap();

        // Stage 1: user ran `codex login` — auth.json has OAuth tokens.
        fs::write(
            tmp.join(".codex/auth.json"),
            r#"{
              "auth_mode": "chatgpt",
              "OPENAI_API_KEY": null,
              "tokens": {
                "refresh_token": "rt-original",
                "access_token": "at-original",
                "id_token": "id-original",
                "account_id": "acc-1"
              },
              "last_refresh": "2025-01-01T00:00:00Z"
            }"#,
        )
        .unwrap();

        // Stage 2: activate Custom API provider via Termory.
        let p = make_provider(CliApp::Codex, "api-temp", "https://temp.api/v1", "sk-temp");
        activate(&p, &[p.clone()]).unwrap();

        let auth_after_activate: JsonValue =
            serde_json::from_str(&fs::read_to_string(tmp.join(".codex/auth.json")).unwrap())
                .unwrap();
        // Switched to apikey, BUT tokens still there.
        assert_eq!(
            auth_after_activate
                .get("auth_mode")
                .and_then(|v| v.as_str()),
            Some("apikey")
        );
        assert_eq!(
            auth_after_activate
                .get("OPENAI_API_KEY")
                .and_then(|v| v.as_str()),
            Some("sk-temp")
        );
        assert_eq!(
            auth_after_activate
                .pointer("/tokens/refresh_token")
                .and_then(|v| v.as_str()),
            Some("rt-original"),
            "OAuth refresh token must survive activate"
        );
        assert_eq!(
            auth_after_activate
                .pointer("/tokens/access_token")
                .and_then(|v| v.as_str()),
            Some("at-original")
        );

        // Stage 3: deactivate → back to ChatGPT mode.
        deactivate(CliApp::Codex, &[p.clone()]).unwrap();

        let auth_after_deactivate: JsonValue =
            serde_json::from_str(&fs::read_to_string(tmp.join(".codex/auth.json")).unwrap())
                .unwrap();
        // ApiKey-mode fields cleared.
        assert!(
            auth_after_deactivate.get("OPENAI_API_KEY").is_none()
                || matches!(
                    auth_after_deactivate.get("OPENAI_API_KEY"),
                    Some(JsonValue::Null)
                )
        );
        // auth_mode removed → Codex resolved_mode() falls back to ChatGPT
        // because tokens is present.
        assert!(auth_after_deactivate.get("auth_mode").is_none());
        // OAuth tokens still intact — user does NOT have to log in again.
        assert_eq!(
            auth_after_deactivate
                .pointer("/tokens/refresh_token")
                .and_then(|v| v.as_str()),
            Some("rt-original")
        );
        assert_eq!(
            auth_after_deactivate
                .pointer("/tokens/access_token")
                .and_then(|v| v.as_str()),
            Some("at-original")
        );
        assert_eq!(
            auth_after_deactivate
                .pointer("/tokens/account_id")
                .and_then(|v| v.as_str()),
            Some("acc-1")
        );
        assert_eq!(
            auth_after_deactivate
                .get("last_refresh")
                .and_then(|v| v.as_str()),
            Some("2025-01-01T00:00:00Z")
        );
    }

    #[test]
    fn codex_deactivate_preserves_existing_oauth_tokens() {
        let _g = HOME_LOCK.lock().unwrap();
        let tmp = tempdir("codex-preserve-oauth");
        let _home = HomeOverride::new(&tmp);
        fs::create_dir_all(tmp.join(".codex")).unwrap();
        // User previously ran `codex login` → OAuth tokens in auth.json.
        // Then activated a Termory Custom provider (auth_mode=apikey,
        // OPENAI_API_KEY set). Now deactivate → tokens MUST survive.
        fs::write(
            tmp.join(".codex/auth.json"),
            r#"{
              "auth_mode": "apikey",
              "OPENAI_API_KEY": "sk-temp",
              "tokens": { "refresh_token": "rt-keep", "access_token": "at-keep" },
              "last_refresh": "2025-01-01T00:00:00Z"
            }"#,
        )
        .unwrap();
        fs::write(
            tmp.join(".codex/config.toml"),
            r#"model_provider = "termory"

[model_providers.termory]
base_url = "https://x.io/v1"
"#,
        )
        .unwrap();

        deactivate(CliApp::Codex, &[]).unwrap();

        let auth: JsonValue =
            serde_json::from_str(&fs::read_to_string(tmp.join(".codex/auth.json")).unwrap())
                .unwrap();
        // ApiKey fields gone.
        assert!(auth.get("OPENAI_API_KEY").is_none());
        assert!(auth.get("auth_mode").is_none());
        // OAuth tokens preserved.
        assert_eq!(
            auth.pointer("/tokens/refresh_token")
                .and_then(|v| v.as_str()),
            Some("rt-keep")
        );
        assert_eq!(
            auth.pointer("/tokens/access_token")
                .and_then(|v| v.as_str()),
            Some("at-keep")
        );
    }

    #[test]
    fn codex_reverse_returns_official_when_model_provider_points_to_builtin() {
        let _g = HOME_LOCK.lock().unwrap();
        let tmp = tempdir("codex-builtin");
        let _home = HomeOverride::new(&tmp);
        fs::create_dir_all(tmp.join(".codex")).unwrap();
        fs::write(
            tmp.join(".codex/config.toml"),
            r#"model_provider = "openai"
"#,
        )
        .unwrap();
        let state = read_active_state(CliApp::Codex, &[]).unwrap();
        assert_eq!(state.kind, ActiveKind::Official);
    }

    #[test]
    fn gemini_activate_writes_dotenv_and_reverses_with_0600() {
        let _g = HOME_LOCK.lock().unwrap();
        let tmp = tempdir("gemini-rt");
        let _home = HomeOverride::new(&tmp);
        let p = make_provider(CliApp::Gemini, "g-third", "https://g.example", "g-sk");
        activate(&p, &[p.clone()]).unwrap();
        let env_text = fs::read_to_string(tmp.join(".gemini/.env")).unwrap();
        assert!(env_text.contains("GOOGLE_GEMINI_BASE_URL=https://g.example"));
        assert!(env_text.contains("GEMINI_API_KEY=g-sk"));
        // make_provider sets model="test-model" → GEMINI_MODEL must
        // also be written, matching cc-switch's preset shape.
        assert!(env_text.contains("GEMINI_MODEL=test-model"));
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            let mode = fs::metadata(tmp.join(".gemini/.env"))
                .unwrap()
                .permissions()
                .mode();
            assert_eq!(mode & 0o777, 0o600, ".env must be 0600");
        }
        let state = read_active_state(CliApp::Gemini, &[p.clone()]).unwrap();
        assert_eq!(state.kind, ActiveKind::Custom);
        assert_eq!(state.matched_provider_id.as_deref(), Some("test-g-third"));
        assert_eq!(
            state
                .live_snapshot
                .as_ref()
                .and_then(|s| s.model.as_deref()),
            Some("test-model")
        );

        deactivate(CliApp::Gemini, &[p.clone()]).unwrap();
        let env_text2 = fs::read_to_string(tmp.join(".gemini/.env")).unwrap();
        // All three Termory-managed env vars cleared.
        for var in ["GOOGLE_GEMINI_BASE_URL", "GEMINI_API_KEY", "GEMINI_MODEL"] {
            assert!(
                !env_text2.contains(&format!("{var}=")),
                "{var} must be cleared after deactivate"
            );
        }
        let state2 = read_active_state(CliApp::Gemini, &[p.clone()]).unwrap();
        assert_eq!(state2.kind, ActiveKind::Official);
    }

    #[test]
    fn gemini_activate_preserves_unrelated_env_vars() {
        // User's `~/.gemini/.env` may already contain other variables
        // (DEBUG_MODE, custom tooling, etc.). We must merge — never
        // overwrite — and only touch the three Termory-managed keys.
        let _g = HOME_LOCK.lock().unwrap();
        let tmp = tempdir("gemini-preserve");
        let _home = HomeOverride::new(&tmp);
        fs::create_dir_all(tmp.join(".gemini")).unwrap();
        fs::write(
            tmp.join(".gemini/.env"),
            "DEBUG=true\nMY_TOOL_PATH=/opt/x\nGEMINI_MODEL=stale-model\n",
        )
        .unwrap();

        let p = make_provider(CliApp::Gemini, "g", "https://g.example", "g-sk");
        activate(&p, &[p.clone()]).unwrap();

        let env_text = fs::read_to_string(tmp.join(".gemini/.env")).unwrap();
        assert!(
            env_text.contains("DEBUG=true"),
            "unrelated DEBUG var must survive"
        );
        assert!(
            env_text.contains("MY_TOOL_PATH=/opt/x"),
            "unrelated MY_TOOL_PATH must survive"
        );
        assert!(env_text.contains("GEMINI_MODEL=test-model"));
        assert!(!env_text.contains("stale-model"));
    }

    #[test]
    fn gemini_oauth_credentials_survive_activate_deactivate_cycle() {
        // `gemini auth` persists OAuth tokens to oauth_creds.json (see
        // `core/src/config/storage.ts:22`). Termory only writes `.env`,
        // so activate → deactivate must leave the credentials file
        // byte-identical and the user stays logged in.
        let _g = HOME_LOCK.lock().unwrap();
        let tmp = tempdir("gemini-oauth-keep");
        let _home = HomeOverride::new(&tmp);
        fs::create_dir_all(tmp.join(".gemini")).unwrap();
        let creds_contents = r#"{
          "access_token": "at-original",
          "refresh_token": "rt-original",
          "expiry_date": 9999999999000
        }"#;
        fs::write(tmp.join(".gemini/oauth_creds.json"), creds_contents).unwrap();
        let accounts_contents = r#"{ "active": "user@example.com" }"#;
        fs::write(tmp.join(".gemini/google_accounts.json"), accounts_contents).unwrap();

        let p = make_provider(CliApp::Gemini, "g-temp", "https://temp.g", "g-temp");
        activate(&p, &[p.clone()]).unwrap();
        assert_eq!(
            fs::read_to_string(tmp.join(".gemini/oauth_creds.json")).unwrap(),
            creds_contents,
            "oauth_creds.json must survive activate byte-for-byte"
        );
        assert_eq!(
            fs::read_to_string(tmp.join(".gemini/google_accounts.json")).unwrap(),
            accounts_contents,
        );

        deactivate(CliApp::Gemini, &[p.clone()]).unwrap();
        assert_eq!(
            fs::read_to_string(tmp.join(".gemini/oauth_creds.json")).unwrap(),
            creds_contents,
            "oauth_creds.json must survive deactivate byte-for-byte"
        );
        assert_eq!(
            fs::read_to_string(tmp.join(".gemini/google_accounts.json")).unwrap(),
            accounts_contents,
        );
    }

    #[test]
    fn opencode_activate_writes_full_provider_block_for_runtime_resolution() {
        // OpenCode's `getModel()` (`provider.ts:1668-1675`) throws
        // ModelNotFoundError if `provider.X.models[Y]` isn't registered,
        // and the SDK adapter selector falls through to
        // `@ai-sdk/openai-compatible` only when neither model.provider.npm
        // nor provider.npm is set (`provider.ts:1273-1278`). We write
        // both to keep OpenCode from crashing or silently falling back.
        let _g = HOME_LOCK.lock().unwrap();
        let tmp = tempdir("opencode-rt");
        let _home = HomeOverride::new(&tmp);
        let p = make_provider(
            CliApp::Opencode,
            "oc-custom",
            "https://oc.example/v1",
            "oc-sk",
        );
        activate(&p, &[p.clone()]).unwrap();
        let after: JsonValue = serde_json::from_str(
            &fs::read_to_string(tmp.join(".config/opencode/opencode.json")).unwrap(),
        )
        .unwrap();
        assert_eq!(
            after.get("model").and_then(|v| v.as_str()),
            Some("termory/test-model")
        );
        assert_eq!(
            after
                .pointer("/provider/termory/options/baseURL")
                .and_then(|v| v.as_str()),
            Some("https://oc.example/v1")
        );
        assert_eq!(
            after
                .pointer("/provider/termory/options/apiKey")
                .and_then(|v| v.as_str()),
            Some("oc-sk")
        );
        // SDK adapter — must be set so OpenCode knows how to call it.
        assert_eq!(
            after
                .pointer("/provider/termory/npm")
                .and_then(|v| v.as_str()),
            Some("@ai-sdk/openai-compatible")
        );
        // Display name carried from Provider.name.
        assert_eq!(
            after
                .pointer("/provider/termory/name")
                .and_then(|v| v.as_str()),
            Some("oc-custom")
        );
        // Model registry entry — required to clear getModel's existence check.
        assert!(
            after
                .pointer("/provider/termory/models/test-model")
                .is_some(),
            "models[test-model] must be registered or OpenCode throws ModelNotFoundError"
        );

        let state = read_active_state(CliApp::Opencode, &[p.clone()]).unwrap();
        assert_eq!(state.kind, ActiveKind::Custom);
        assert_eq!(state.matched_provider_id.as_deref(), Some("test-oc-custom"));

        deactivate(CliApp::Opencode, &[p.clone()]).unwrap();
        let state2 = read_active_state(CliApp::Opencode, &[p.clone()]).unwrap();
        assert_eq!(state2.kind, ActiveKind::Official);
    }

    #[test]
    fn opencode_activate_does_not_touch_auth_json() {
        // Users log in to OpenCode providers via `opencode auth login`,
        // which stores credentials in `~/.local/share/opencode/auth.json`
        // (`auth/index.ts:9, 78`). Termory writes to a different file
        // (`~/.config/opencode/opencode.json`), so existing auth.json
        // logins must survive activate/deactivate untouched.
        let _g = HOME_LOCK.lock().unwrap();
        let tmp = tempdir("opencode-auth-isolated");
        let _home = HomeOverride::new(&tmp);
        // Simulate a prior `opencode auth login anthropic`.
        fs::create_dir_all(tmp.join(".local/share/opencode")).unwrap();
        let auth_contents = r#"{
          "anthropic": {
            "type": "oauth",
            "refresh": "rt-anthropic",
            "access": "at-anthropic",
            "expires": 9999999999000
          }
        }"#;
        fs::write(tmp.join(".local/share/opencode/auth.json"), auth_contents).unwrap();

        let p = make_provider(
            CliApp::Opencode,
            "third-party",
            "https://oc.example/v1",
            "oc-sk",
        );
        activate(&p, &[p.clone()]).unwrap();
        assert_eq!(
            fs::read_to_string(tmp.join(".local/share/opencode/auth.json")).unwrap(),
            auth_contents,
            "auth.json must survive activate byte-for-byte"
        );

        deactivate(CliApp::Opencode, &[p.clone()]).unwrap();
        assert_eq!(
            fs::read_to_string(tmp.join(".local/share/opencode/auth.json")).unwrap(),
            auth_contents,
            "auth.json must survive deactivate byte-for-byte"
        );
    }

    #[test]
    fn opencode_activate_preserves_user_provider_blocks_for_other_ids() {
        // OpenCode is additive: users may have multiple `provider.X`
        // blocks (e.g. one for github-copilot via `auth login`).
        // Termory only writes/clears `provider.termory` — unrelated
        // blocks must survive the whole cycle.
        let _g = HOME_LOCK.lock().unwrap();
        let tmp = tempdir("opencode-additive");
        let _home = HomeOverride::new(&tmp);
        fs::create_dir_all(tmp.join(".config/opencode")).unwrap();
        fs::write(
            tmp.join(".config/opencode/opencode.json"),
            r#"{
              "$schema": "https://opencode.ai/config.json",
              "provider": {
                "github-copilot": {
                  "options": { "enterpriseUrl": "https://ghe.example/" }
                }
              }
            }"#,
        )
        .unwrap();

        let p = make_provider(
            CliApp::Opencode,
            "oc-custom",
            "https://oc.example/v1",
            "oc-sk",
        );
        activate(&p, &[p.clone()]).unwrap();
        let after: JsonValue = serde_json::from_str(
            &fs::read_to_string(tmp.join(".config/opencode/opencode.json")).unwrap(),
        )
        .unwrap();
        // Our new block is there.
        assert!(after.pointer("/provider/termory/npm").is_some());
        // User's github-copilot block survived.
        assert_eq!(
            after
                .pointer("/provider/github-copilot/options/enterpriseUrl")
                .and_then(|v| v.as_str()),
            Some("https://ghe.example/")
        );
        // $schema preserved.
        assert!(after.get("$schema").is_some());

        deactivate(CliApp::Opencode, &[p.clone()]).unwrap();
        let after2: JsonValue = serde_json::from_str(
            &fs::read_to_string(tmp.join(".config/opencode/opencode.json")).unwrap(),
        )
        .unwrap();
        // termory gone, github-copilot survived.
        assert!(after2.pointer("/provider/termory").is_none());
        assert_eq!(
            after2
                .pointer("/provider/github-copilot/options/enterpriseUrl")
                .and_then(|v| v.as_str()),
            Some("https://ghe.example/")
        );
        assert!(after2.get("$schema").is_some());
    }

    #[test]
    fn opencode_activate_rejects_empty_model() {
        // Saving an OpenCode provider with no model would write
        // `provider.X.models = {}` plus no top-level `model` field —
        // OpenCode's defaultModel() then throws "no models found" at
        // startup. Refuse the activation explicitly so the user
        // doesn't end up with a config that crashes OpenCode.
        let _g = HOME_LOCK.lock().unwrap();
        let tmp = tempdir("opencode-no-model");
        let _home = HomeOverride::new(&tmp);

        let mut p = make_provider(
            CliApp::Opencode,
            "no-model",
            "https://oc.example/v1",
            "oc-sk",
        );
        p.model = String::new();
        let result = activate(&p, &[p.clone()]);
        assert!(result.is_err(), "empty model must be rejected");
        let msg = result.unwrap_err().to_string();
        assert!(
            msg.to_lowercase().contains("model"),
            "error message must mention model: {msg}"
        );
        // opencode.json must NOT have been touched (we error out before
        // writing).
        assert!(
            !tmp.join(".config/opencode/opencode.json").exists(),
            "no partial config should be written on rejection"
        );
    }

    #[test]
    fn mask_secret_format() {
        assert_eq!(mask_secret("short"), "•••••");
        // "sk-1234567890abcd" is 17 chars; mask = head(4) + dots(17-8=9) + tail(4)
        assert_eq!(mask_secret("sk-1234567890abcd"), "sk-1•••••••••abcd");
    }
}
