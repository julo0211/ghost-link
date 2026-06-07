// ghost link — application native (Tauri 2 + iroh).
// Session : se connecter à un pair, puis envoyer/recevoir des fichiers librement,
// avec débit, annulation et déconnexion propagée.
#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]

mod net;

use net::Net;
use std::sync::atomic::Ordering;
use tauri::{Emitter, Manager, State};
use tauri_plugin_updater::{Update, UpdaterExt};

/// Mise à jour téléchargée en attente d'installation.
struct PendingUpdate(std::sync::Mutex<Option<Update>>);

#[tauri::command]
async fn my_addr(state: State<'_, Net>) -> Result<String, String> {
    let ep = state.endpoint.clone();
    net::my_addr(&ep).await.map_err(|e| e.to_string())
}

#[tauri::command]
async fn my_id(state: State<'_, Net>) -> Result<String, String> {
    let ep = state.endpoint.clone();
    net::my_id(&ep).await.map_err(|e| e.to_string())
}

#[tauri::command]
async fn probe(state: State<'_, Net>, id: String) -> Result<bool, String> {
    let ep = state.endpoint.clone();
    Ok(net::probe(&ep, &id).await)
}

#[tauri::command]
async fn connect(app: tauri::AppHandle, state: State<'_, Net>, addr: String) -> Result<String, String> {
    let ep = state.endpoint.clone();
    let slot = state.slot.clone();
    let rc = state.recv_cancel.clone();
    let settings = state.settings.clone();
    net::connect(&ep, &app, &slot, &rc, &settings, &addr).await.map_err(|e| e.to_string())
}

#[tauri::command]
async fn send_file(app: tauri::AppHandle, state: State<'_, Net>, path: String) -> Result<String, String> {
    let slot = state.slot.clone();
    let sc = state.send_cancel.clone();
    net::send_file(&app, &slot, &sc, &path).await.map_err(|e| e.to_string())
}

#[tauri::command]
async fn send_chat(state: State<'_, Net>, text: String, name: String) -> Result<(), String> {
    let slot = state.slot.clone();
    net::send_chat(&slot, &name, &text).await.map_err(|e| e.to_string())
}

#[tauri::command]
async fn send_freq(state: State<'_, Net>, name: String) -> Result<(), String> {
    let slot = state.slot.clone();
    net::send_freq(&slot, &name).await.map_err(|e| e.to_string())
}

#[tauri::command]
async fn send_faccept(state: State<'_, Net>, name: String) -> Result<(), String> {
    let slot = state.slot.clone();
    net::send_faccept(&slot, &name).await.map_err(|e| e.to_string())
}

#[tauri::command]
fn set_download_dir(state: State<'_, Net>, path: String) {
    net::set_download_dir(&state.settings, &path);
}

#[tauri::command]
fn get_download_dir(state: State<'_, Net>) -> String {
    net::get_download_dir(&state.settings)
}

#[tauri::command]
fn set_only_friends(state: State<'_, Net>, on: bool) {
    net::set_only_friends(&state.settings, on);
}

#[tauri::command]
fn set_friends(state: State<'_, Net>, codes: Vec<String>) {
    net::set_friends(&state.settings, codes);
}

#[tauri::command]
fn fingerprint(code: String) -> String {
    net::fingerprint(&code)
}

#[tauri::command]
fn app_version(app: tauri::AppHandle) -> String {
    app.package_info().version.to_string()
}

/// Cherche une mise à jour. Renvoie la version disponible (ou null), et la garde en attente.
#[tauri::command]
async fn check_update(
    app: tauri::AppHandle,
    pending: State<'_, PendingUpdate>,
) -> Result<Option<String>, String> {
    let update = app
        .updater()
        .map_err(|e| e.to_string())?
        .check()
        .await
        .map_err(|e| e.to_string())?;
    let version = update.as_ref().map(|u| u.version.clone());
    *pending.0.lock().unwrap() = update;
    Ok(version)
}

/// Télécharge et installe la mise à jour en attente, puis redémarre l'app.
#[tauri::command]
async fn install_update(
    app: tauri::AppHandle,
    pending: State<'_, PendingUpdate>,
) -> Result<(), String> {
    let update = pending.0.lock().unwrap().take();
    let update = update.ok_or_else(|| "aucune mise à jour en attente".to_string())?;
    let app2 = app.clone();
    update
        .download_and_install(
            move |chunk, total| {
                let _ = app2.emit(
                    "update-progress",
                    serde_json::json!({ "chunk": chunk, "total": total }),
                );
            },
            || {},
        )
        .await
        .map_err(|e| e.to_string())?;
    app.restart();
    #[allow(unreachable_code)]
    Ok(())
}

#[tauri::command]
async fn disconnect(app: tauri::AppHandle, state: State<'_, Net>) -> Result<(), String> {
    let slot = state.slot.clone();
    net::disconnect(&app, &slot).await;
    Ok(())
}

#[tauri::command]
fn cancel_send(state: State<'_, Net>) {
    state.send_cancel.store(true, Ordering::SeqCst);
}

#[tauri::command]
fn cancel_recv(state: State<'_, Net>) {
    state.recv_cancel.store(true, Ordering::SeqCst);
}

fn main() {
    tauri::Builder::default()
        .plugin(tauri_plugin_updater::Builder::new().build())
        .setup(|app| {
            let handle = app.handle().clone();
            let net = tauri::async_runtime::block_on(net::start(handle))
                .expect("démarrage du réseau iroh impossible");
            app.manage(net);
            app.manage(PendingUpdate(std::sync::Mutex::new(None)));
            Ok(())
        })
        .invoke_handler(tauri::generate_handler![
            my_addr, my_id, probe, connect, send_file, send_chat, send_freq, send_faccept,
            fingerprint, app_version, check_update, install_update, set_download_dir,
            get_download_dir, set_only_friends, set_friends, disconnect, cancel_send,
            cancel_recv
        ])
        .run(tauri::generate_context!())
        .expect("erreur au lancement de ghost link");
}
