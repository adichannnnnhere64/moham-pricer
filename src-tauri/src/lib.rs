pub mod server;

#[cfg(feature = "desktop")]
use serde::Serialize;
#[cfg(feature = "desktop")]
use server::{start_server, ApiRequestLog, RequestHistory, ServerConfig, ServerHandle};
#[cfg(feature = "desktop")]
use std::{
    collections::VecDeque,
    fs,
    net::{IpAddr, Ipv4Addr, SocketAddr, UdpSocket},
    path::PathBuf,
    sync::{Arc, Mutex, MutexGuard},
};
#[cfg(feature = "desktop")]
use tauri::{AppHandle, Manager, State};
#[cfg(all(feature = "desktop", target_os = "windows"))]
use window_vibrancy::apply_acrylic;

#[cfg(feature = "desktop")]
#[derive(Default)]
struct ManagedState {
    server: Mutex<Option<ServerHandle>>,
    last_error: Mutex<Option<String>>,
    request_history: RequestHistory,
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
#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
struct RequestMetrics {
    total: usize,
    ok: usize,
    errors: usize,
    avg_duration_ms: u64,
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
async fn start_api_server_internal(
    state: &ManagedState,
    config: ServerConfig,
    save: bool,
    app: Option<AppHandle>,
) -> Result<ServerStatus, String> {
    if save {
        if let Some(handle) = app {
            save_settings(handle, config.clone())?;
        }
    }

    {
        let mut current = lock(&state.server)?;
        if let Some(server) = current.as_mut() {
            server.stop();
        }
        *current = None;
    }

    match start_server(config, state.request_history.clone()).await {
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
async fn start_api_server(
    app: AppHandle,
    state: State<'_, ManagedState>,
    config: ServerConfig,
) -> Result<ServerStatus, String> {
    start_api_server_internal(&*state, config, true, Some(app)).await
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
#[tauri::command]
fn request_history(state: State<'_, ManagedState>) -> Result<Vec<ApiRequestLog>, String> {
    Ok(lock(&state.request_history)?
        .iter()
        .rev()
        .take(100)
        .cloned()
        .collect())
}

#[cfg(feature = "desktop")]
#[tauri::command]
fn clear_request_history(state: State<'_, ManagedState>) -> Result<(), String> {
    lock(&state.request_history)?.clear();
    Ok(())
}

#[cfg(feature = "desktop")]
#[tauri::command]
fn request_metrics(state: State<'_, ManagedState>) -> Result<RequestMetrics, String> {
    let history = lock(&state.request_history)?;
    let total = history.len();
    let ok = history
        .iter()
        .filter(|entry| (200..400).contains(&entry.status))
        .count();
    let errors = total.saturating_sub(ok);
    let duration_sum: u64 = history.iter().map(|entry| entry.duration_ms).sum();
    let avg_duration_ms = if total == 0 {
        0
    } else {
        duration_sum / total as u64
    };

    Ok(RequestMetrics {
        total,
        ok,
        errors,
        avg_duration_ms,
    })
}

#[cfg(feature = "desktop")]
#[tauri::command]
fn machine_ip() -> Option<String> {
    let socket = UdpSocket::bind(SocketAddr::from((Ipv4Addr::UNSPECIFIED, 0))).ok()?;
    socket.connect(SocketAddr::from(([8, 8, 8, 8], 80))).ok()?;

    match socket.local_addr().ok()?.ip() {
        IpAddr::V4(address) if !address.is_unspecified() && !address.is_loopback() => {
            Some(address.to_string())
        }
        _ => None,
    }
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
        .manage(ManagedState {
            server: Mutex::new(None),
            last_error: Mutex::new(None),
            request_history: Arc::new(Mutex::new(VecDeque::new())),
        })
        .invoke_handler(tauri::generate_handler![
            load_settings,
            save_settings,
            start_api_server,
            stop_api_server,
            server_status,
            request_history,
            clear_request_history,
            request_metrics,
            machine_ip
        ])
        .setup(|app| {
            #[cfg(target_os = "windows")]
            {
                let window = app.get_webview_window("main").unwrap();
                if let Err(error) = apply_acrylic(&window, Some((18, 18, 18, 125))) {
                    eprintln!("Unable to apply acrylic window effect: {error}");
                }
            }

            // Auto-start server if settings exist and are valid
            #[cfg(feature = "desktop")]
            {
                let app_handle = app.handle().clone();

                tauri::async_runtime::spawn(async move {
                    // Small delay to let UI initialize first
                    tokio::time::sleep(tokio::time::Duration::from_millis(500)).await;

                    if let Ok(config) = load_settings(app_handle.clone()) {
                        // Only auto-start if config has database configured (not empty)
                        if !config.mysql_host.trim().is_empty()
                            && !config.mysql_database.trim().is_empty()
                            && !config.mysql_username.trim().is_empty()
                            && !config.api_token.trim().is_empty()
                            && !config.table_name.trim().is_empty()
                        {
                            let state = app_handle.state::<ManagedState>();
                            let _ = start_api_server_internal(&*state, config, false, None).await;
                        }
                    }
                });
            }

            Ok(())
        })
        .run(tauri::generate_context!())
        .expect("error while running tauri application");
}

#[cfg(not(feature = "desktop"))]
pub fn run() {
    panic!("The desktop Tauri app requires the `desktop` feature.");
}
