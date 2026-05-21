mod sessions;

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

pub fn run() {
    tauri::Builder::default()
        .plugin(tauri_plugin_opener::init())
        .plugin(tauri_plugin_dialog::init())
        .invoke_handler(tauri::generate_handler![
            scan_all_sessions,
            load_session,
            search_all_sessions
        ])
        .run(tauri::generate_context!())
        .expect("error while running tauri application");
}
