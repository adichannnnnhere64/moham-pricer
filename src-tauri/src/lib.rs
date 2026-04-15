pub mod server;

#[cfg(feature = "desktop")]
use serde::Serialize;
#[cfg(feature = "desktop")]
use server::{start_server, ServerConfig, ServerHandle};
#[cfg(feature = "desktop")]
use std::{
    fs,
    path::PathBuf,
    sync::{Mutex, MutexGuard},
};
#[cfg(feature = "desktop")]
use tauri::{AppHandle, Manager, State};

#[cfg(feature = "desktop")]
#[derive(Default)]
struct ManagedState {
    server: Mutex<Option<ServerHandle>>,
    last_error: Mutex<Option<String>>,
}

#[cfg(feature = "desktop")]
#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
struct ServerStatus {
    running: bool,
    bind_address: Option<String>,
    last_error: Option<String>,
}

#[cfg(feature = "desktop")]
#[tauri::command]
fn load_settings(app: AppHandle) -> Result<ServerConfig, String> {
    let path = settings_path(&app)?;
    if !path.exists() {
        return Ok(ServerConfig::default());
    }

    let contents = fs::read_to_string(&path)
        .map_err(|error| format!("Unable to read settings file {}: {error}", path.display()))?;
    serde_json::from_str(&contents)
        .map_err(|error| format!("Unable to parse settings file {}: {error}", path.display()))
}

#[cfg(feature = "desktop")]
#[tauri::command]
fn save_settings(app: AppHandle, config: ServerConfig) -> Result<(), String> {
    let path = settings_path(&app)?;
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent)
            .map_err(|error| format!("Unable to create settings directory: {error}"))?;
    }

    let contents = serde_json::to_string_pretty(&config)
        .map_err(|error| format!("Unable to serialize settings: {error}"))?;
    fs::write(&path, contents)
        .map_err(|error| format!("Unable to write settings file {}: {error}", path.display()))
}

#[cfg(feature = "desktop")]
#[tauri::command]
async fn start_api_server(
    app: AppHandle,
    state: State<'_, ManagedState>,
    config: ServerConfig,
) -> Result<ServerStatus, String> {
    save_settings(app, config.clone())?;

    {
        let mut current = lock(&state.server)?;
        if let Some(server) = current.as_mut() {
            server.stop();
        }
        *current = None;
    }

    match start_server(config).await {
        Ok(handle) => {
            {
                let mut last_error = lock(&state.last_error)?;
                *last_error = None;
            }

            let bind_address = handle.bind_address.clone();
            let mut current = lock(&state.server)?;
            *current = Some(handle);

            Ok(ServerStatus {
                running: true,
                bind_address: Some(bind_address),
                last_error: None,
            })
        }
        Err(error) => {
            let mut last_error = lock(&state.last_error)?;
            *last_error = Some(error.clone());
            Err(error)
        }
    }
}

#[cfg(feature = "desktop")]
#[tauri::command]
fn stop_api_server(state: State<'_, ManagedState>) -> Result<ServerStatus, String> {
    let mut current = lock(&state.server)?;
    if let Some(server) = current.as_mut() {
        server.stop();
    }
    *current = None;

    Ok(ServerStatus {
        running: false,
        bind_address: None,
        last_error: lock(&state.last_error)?.clone(),
    })
}

#[cfg(feature = "desktop")]
#[tauri::command]
fn server_status(state: State<'_, ManagedState>) -> Result<ServerStatus, String> {
    let current = lock(&state.server)?;
    Ok(ServerStatus {
        running: current.is_some(),
        bind_address: current.as_ref().map(|server| server.bind_address.clone()),
        last_error: lock(&state.last_error)?.clone(),
    })
}

#[cfg(feature = "desktop")]
fn settings_path(app: &AppHandle) -> Result<PathBuf, String> {
    app.path()
        .app_config_dir()
        .map(|path| path.join("settings.json"))
        .map_err(|error| format!("Unable to locate app settings directory: {error}"))
}

#[cfg(feature = "desktop")]
fn lock<T>(mutex: &Mutex<T>) -> Result<MutexGuard<'_, T>, String> {
    mutex
        .lock()
        .map_err(|_| "Internal state lock was poisoned.".to_string())
}

#[cfg(feature = "desktop")]
#[cfg_attr(mobile, tauri::mobile_entry_point)]
pub fn run() {
    tauri::Builder::default()
        .plugin(tauri_plugin_opener::init())
        .manage(ManagedState::default())
        .invoke_handler(tauri::generate_handler![
            load_settings,
            save_settings,
            start_api_server,
            stop_api_server,
            server_status
        ])
        .run(tauri::generate_context!())
        .expect("error while running tauri application");
}

#[cfg(not(feature = "desktop"))]
pub fn run() {
    panic!("The desktop Tauri app requires the `desktop` feature.");
}
