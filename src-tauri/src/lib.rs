mod config;
mod providers;
mod sessions;
mod watcher;

/// Shared test infrastructure. The `HOME_LOCK` mutex serializes tests
/// that mutate the `HOME` env var across both `config` and
/// `providers` modules — without a single shared lock, parallel test
/// execution lets one module clobber another's HOME override.
#[cfg(test)]
pub(crate) mod testutils {
    use std::sync::Mutex;
    pub static HOME_LOCK: Mutex<()> = Mutex::new(());
}

use providers::{
    activate, deactivate, fetch_models, read_active_state, test_provider, ActiveState, CliApp,
    ModelListResult, Provider, TestResult,
};
use sessions::{get_session, scan_sessions, search_sessions, AppSession, SearchHit, SessionDetail};

#[tauri::command]
async fn scan_all_sessions() -> Result<Vec<AppSession>, String> {
    tauri::async_runtime::spawn_blocking(|| scan_sessions().map_err(|err| err.to_string()))
        .await
        .map_err(|err| err.to_string())?
}

#[tauri::command]
async fn load_session(source: String, path: String, id: String) -> Result<SessionDetail, String> {
    tauri::async_runtime::spawn_blocking(move || {
        get_session(&source, &path, &id).map_err(|err| err.to_string())
    })
    .await
    .map_err(|err| err.to_string())?
}

#[tauri::command]
async fn search_all_sessions(query: String) -> Result<Vec<SearchHit>, String> {
    tauri::async_runtime::spawn_blocking(move || {
        search_sessions(&query).map_err(|err| err.to_string())
    })
    .await
    .map_err(|err| err.to_string())?
}

/// Reverse-derive the active provider state for one CLI. The frontend
/// passes its current Provider list so we can match against it; nothing
/// is stored backend-side.
#[tauri::command]
async fn provider_active_state(
    app: String,
    providers: Vec<Provider>,
) -> Result<ActiveState, String> {
    tauri::async_runtime::spawn_blocking(move || {
        let cli = CliApp::parse(&app).ok_or_else(|| format!("unknown app: {app}"))?;
        read_active_state(cli, &providers).map_err(|e| e.to_string())
    })
    .await
    .map_err(|err| err.to_string())?
}

/// Reverse-derive active state for all four CLIs in one call.
#[tauri::command]
async fn provider_active_states(providers: Vec<Provider>) -> Result<Vec<ActiveState>, String> {
    tauri::async_runtime::spawn_blocking(move || {
        let mut out = Vec::with_capacity(4);
        for app in [
            CliApp::Claude,
            CliApp::Codex,
            CliApp::Gemini,
            CliApp::Opencode,
        ] {
            out.push(read_active_state(app, &providers).map_err(|e| e.to_string())?);
        }
        Ok(out)
    })
    .await
    .map_err(|err| err.to_string())?
}

/// Activate a provider — materializes it into the matching CLI's
/// live config files. For Official kind, clears Termory-injected
/// fields. `providers_for_app` is the user's full provider list for
/// this app, used by Opencode deactivate to know which custom blocks
/// to strip.
#[tauri::command]
async fn activate_provider(
    provider: Provider,
    providers_for_app: Vec<Provider>,
) -> Result<(), String> {
    tauri::async_runtime::spawn_blocking(move || {
        activate(&provider, &providers_for_app).map_err(|e| e.to_string())
    })
    .await
    .map_err(|err| err.to_string())?
}

/// Restore a CLI to its native auth flow by clearing all
/// Termory-injected fields.
#[tauri::command]
async fn deactivate_provider(app: String, providers_for_app: Vec<Provider>) -> Result<(), String> {
    tauri::async_runtime::spawn_blocking(move || {
        let cli = CliApp::parse(&app).ok_or_else(|| format!("unknown app: {app}"))?;
        deactivate(cli, &providers_for_app).map_err(|e| e.to_string())
    })
    .await
    .map_err(|err| err.to_string())?
}

/// Send a connectivity probe to the provider's base URL.
#[tauri::command]
async fn test_provider_api(provider: Provider) -> Result<TestResult, String> {
    Ok(test_provider(&provider).await)
}

/// Hit the provider's models endpoint and return the available model
/// ids. Used to populate the Model field autocomplete suggestions.
#[tauri::command]
async fn fetch_provider_models(provider: Provider) -> Result<ModelListResult, String> {
    Ok(fetch_models(&provider).await)
}

/// Read ~/.termory/config.json. Returns an empty `{}` if missing.
#[tauri::command]
async fn read_app_config() -> Result<serde_json::Value, String> {
    tauri::async_runtime::spawn_blocking(|| config::read_config().map_err(|e| e.to_string()))
        .await
        .map_err(|e| e.to_string())?
}

/// Atomically write ~/.termory/config.json with file mode 0600 (Unix).
#[tauri::command]
async fn write_app_config(value: serde_json::Value) -> Result<(), String> {
    tauri::async_runtime::spawn_blocking(move || {
        config::write_config(&value).map_err(|e| e.to_string())
    })
    .await
    .map_err(|e| e.to_string())?
}

/// Read ~/.termory/providers.json. Returns an empty `[]` if missing.
/// Separate file because it holds API keys — file is chmod 0600.
#[tauri::command]
async fn read_app_providers() -> Result<serde_json::Value, String> {
    tauri::async_runtime::spawn_blocking(|| config::read_providers().map_err(|e| e.to_string()))
        .await
        .map_err(|e| e.to_string())?
}

/// Atomically write ~/.termory/providers.json with file mode 0600 (Unix).
#[tauri::command]
async fn write_app_providers(value: serde_json::Value) -> Result<(), String> {
    tauri::async_runtime::spawn_blocking(move || {
        config::write_providers(&value).map_err(|e| e.to_string())
    })
    .await
    .map_err(|e| e.to_string())?
}

pub fn run() {
    tauri::Builder::default()
        .plugin(tauri_plugin_opener::init())
        .plugin(tauri_plugin_dialog::init())
        .invoke_handler(tauri::generate_handler![
            scan_all_sessions,
            load_session,
            search_all_sessions,
            provider_active_state,
            provider_active_states,
            activate_provider,
            deactivate_provider,
            test_provider_api,
            fetch_provider_models,
            read_app_config,
            write_app_config,
            read_app_providers,
            write_app_providers,
        ])
        .setup(|app| {
            // Background filesystem watcher: pushes a fresh
            // `Vec<AppSession>` to the frontend via
            // `termory:sources-changed` whenever a watched source
            // directory mutates. Failure is non-fatal — the app still
            // works with only the launch-time scan.
            let handle = app.handle().clone();
            if let Err(err) = watcher::start(handle) {
                eprintln!("termory watcher: init failed: {err}");
            }
            Ok(())
        })
        .run(tauri::generate_context!())
        .expect("error while running tauri application");
}
