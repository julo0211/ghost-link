// ghost link — application native (Tauri 2 + iroh).
// Session : se connecter à un pair, puis envoyer/recevoir des fichiers librement,
// avec débit, annulation et déconnexion propagée.
#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]

mod audio;
mod meta;
mod net;
#[cfg(windows)]
mod sysaudio;
mod video;

use net::Net;
use std::sync::atomic::Ordering;
use tauri::{Emitter, Manager, State};
use tauri_plugin_updater::{Update, UpdaterExt};

/// Mise à jour téléchargée en attente d'installation.
struct PendingUpdate(std::sync::Mutex<Option<Update>>);

#[tauri::command]
fn perm_code(state: State<'_, Net>) -> String {
    net::perm_code(state.inner())
}

#[tauri::command]
async fn eph_code(state: State<'_, Net>) -> Result<String, String> {
    Ok(net::eph_code(state.inner()).await)
}

#[tauri::command]
async fn rotate_eph_code(state: State<'_, Net>) -> Result<String, String> {
    net::rotate_eph(state.inner()).await.map_err(|e| e.to_string())
}

#[tauri::command]
async fn probe(state: State<'_, Net>, id: String) -> Result<bool, String> {
    Ok(net::probe(state.inner(), &id).await)
}

#[tauri::command]
async fn connect(state: State<'_, Net>, addr: String) -> Result<String, String> {
    net::connect(state.inner(), &addr).await.map_err(|e| e.to_string())
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
    let code = net::perm_code(state.inner());
    net::send_freq(&slot, &name, &code).await.map_err(|e| e.to_string())
}

#[tauri::command]
async fn send_faccept(state: State<'_, Net>, name: String) -> Result<(), String> {
    let slot = state.slot.clone();
    let code = net::perm_code(state.inner());
    net::send_faccept(&slot, &name, &code).await.map_err(|e| e.to_string())
}

#[tauri::command]
async fn open_group(state: State<'_, Net>, members: Vec<String>) -> Result<(), String> {
    net::open_group(state.inner(), members).await;
    Ok(())
}

#[tauri::command]
async fn send_gchat(state: State<'_, Net>, members: Vec<String>, gid: String, name: String, text: String) -> Result<(), String> {
    net::send_gchat(state.inner(), members, &gid, &name, &text).await.map_err(|e| e.to_string())
}

#[tauri::command]
async fn send_ginvite(state: State<'_, Net>, member: String, gid: String, name: String, members: String) -> Result<(), String> {
    net::send_ginvite(state.inner(), &member, &gid, &name, &members).await.map_err(|e| e.to_string())
}

#[tauri::command]
async fn send_gmembers(state: State<'_, Net>, members: Vec<String>, gid: String, name: String, roster: String) -> Result<(), String> {
    net::send_gmembers(state.inner(), members, &gid, &name, &roster).await.map_err(|e| e.to_string())
}

#[tauri::command]
async fn send_kick(state: State<'_, Net>, members: Vec<String>, gid: String, target: String, voter: String) -> Result<(), String> {
    net::send_kick(state.inner(), members, &gid, &target, &voter).await.map_err(|e| e.to_string())
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
fn set_streams(n: u64) {
    net::set_streams(n);
}

#[tauri::command]
async fn voice_test_start(
    voice: State<'_, audio::Voice>,
    cfg: State<'_, audio::AudioCfg>,
) -> Result<(), String> {
    let v = voice.inner().clone();
    let c = cfg.inner().clone();
    tokio::task::spawn_blocking(move || v.start(c))
        .await
        .map_err(|e| e.to_string())?
        .map_err(|e| e.to_string())
}

#[tauri::command]
fn voice_test_stop(voice: State<'_, audio::Voice>) {
    voice.stop();
}

#[tauri::command]
async fn call_start(
    net: State<'_, Net>,
    call: State<'_, audio::Call>,
    cfg: State<'_, audio::AudioCfg>,
    signal: bool,
) -> Result<(), String> {
    let conn = net::current(&net.slot)
        .await
        .ok_or_else(|| "pas connecté à un pair".to_string())?;
    let c = call.inner().clone();
    let acfg = cfg.inner().clone();
    let rt = tokio::runtime::Handle::current();
    tokio::task::spawn_blocking(move || c.start(conn, rt, acfg))
        .await
        .map_err(|e| e.to_string())?
        .map_err(|e| e.to_string())?;
    if signal {
        let slot = net.slot.clone();
        net::send_call_start(&slot).await.map_err(|e| e.to_string())?;
    }
    Ok(())
}

#[tauri::command]
async fn call_stop(
    net: State<'_, Net>,
    call: State<'_, audio::Call>,
    signal: bool,
) -> Result<(), String> {
    call.stop();
    if signal {
        let slot = net.slot.clone();
        let _ = net::send_call_stop(&slot).await;
    }
    Ok(())
}

#[tauri::command]
fn call_set_mute(call: State<'_, audio::Call>, on: bool) {
    call.set_mute(on);
}

#[tauri::command]
async fn group_call_start(
    app: tauri::AppHandle,
    net: State<'_, Net>,
    call: State<'_, audio::GroupCall>,
    cfg: State<'_, audio::AudioCfg>,
    members: Vec<String>,
    gid: String,
    announce: bool,
) -> Result<(), String> {
    let conns = net::group_conns(net.inner(), &members);
    if conns.is_empty() {
        return Err("aucun membre du groupe en ligne".to_string());
    }
    let c = call.inner().clone();
    let acfg = cfg.inner().clone();
    let rt = tokio::runtime::Handle::current();
    tokio::task::spawn_blocking(move || c.start(app, conns, rt, acfg))
        .await
        .map_err(|e| e.to_string())?
        .map_err(|e| e.to_string())?;
    if announce {
        let _ = net::send_gcall(net.inner(), members, &gid).await;
    }
    Ok(())
}

#[tauri::command]
fn group_call_stop(
    call: State<'_, audio::GroupCall>,
    sa: State<'_, audio::ScreenAudio>,
    vs: State<'_, video::VideoShare>,
) {
    call.stop();
    // Filet : le partage d'écran ne vit que DANS l'appel — si un chemin d'arrêt côté
    // front a raté screen_audio_stop / video_share_stop (course UI), ni la capture du
    // son système ni celle de l'écran ne doivent survivre au raccrochage. Idempotent.
    sa.stop();
    vs.stop();
}

#[tauri::command]
fn group_call_mute(call: State<'_, audio::GroupCall>, on: bool) {
    call.set_mute(on);
}

#[tauri::command]
fn group_call_volume(call: State<'_, audio::GroupCall>, peer: String, vol: f64) {
    call.set_gain(&peer, vol as f32);
}

// Son système du partage d'écran (repli natif quand WebView2 ne fournit pas de piste
// audio — partage d'une fenêtre) : loopback WASAPI → Opus → datagrammes du maillage.
#[tauri::command]
async fn screen_audio_start(
    net: State<'_, Net>,
    sa: State<'_, audio::ScreenAudio>,
    members: Vec<String>,
    pid: Option<u32>,
) -> Result<(), String> {
    let conns: Vec<_> = net::group_conns(net.inner(), &members)
        .into_iter()
        .map(|(_, c)| c)
        .collect();
    if conns.is_empty() {
        return Err("aucun membre du groupe en ligne".to_string());
    }
    let s = sa.inner().clone();
    tokio::task::spawn_blocking(move || s.start(conns, pid))
        .await
        .map_err(|e| e.to_string())?
        .map_err(|e| e.to_string())
}

#[tauri::command]
fn screen_audio_stop(sa: State<'_, audio::ScreenAudio>) {
    sa.stop();
}

// Coupe/rétablit LOCALEMENT le son d'écran partagé par un pair (mixage de l'appel de
// groupe) — indépendant du volume de sa voix. N'affecte que ma propre lecture.
#[tauri::command]
fn screen_audio_mute(call: State<'_, audio::GroupCall>, peer: String, on: bool) {
    call.set_screen_mute(&peer, on);
}

// Volume LOCAL du son d'écran d'un pair (le « stream qu'on regarde ») : 0.0..=2.0.
#[tauri::command]
fn screen_audio_gain(call: State<'_, audio::GroupCall>, peer: String, vol: f64) {
    call.set_screen_gain(&peer, vol as f32);
}

#[tauri::command]
async fn send_signal(state: State<'_, Net>, peer: String, data: String) -> Result<(), String> {
    net::send_signal(state.inner(), &peer, &data).await.map_err(|e| e.to_string())
}

// Partage d'écran NATIF (video.rs) : capture WGC + H.264 matériel + flux QUIC du
// maillage — aucun WebRTC/STUN, l'IP n'est jamais exposée. Renvoie { w, h, fps }
// pour que l'UI l'annonce aux membres via la signalisation existante.
#[tauri::command]
async fn video_share_start(
    net: State<'_, Net>,
    vs: State<'_, video::VideoShare>,
    app: tauri::AppHandle,
    members: Vec<String>,
    monitor: Option<String>,
    window: Option<String>,
) -> Result<serde_json::Value, String> {
    let conns = net::group_conns(net.inner(), &members);
    if conns.is_empty() {
        return Err("aucun membre du groupe en ligne".to_string());
    }
    // Fenêtre choisie (HWND décimal) → capture de fenêtre ; sinon moniteur (szDevice).
    let target = match window.as_deref() {
        Some(w) if !w.is_empty() => {
            let hwnd = w.parse::<isize>().map_err(|_| "fenêtre invalide".to_string())?;
            video::ShareTarget::Window(hwnd)
        }
        _ => video::ShareTarget::Monitor(monitor),
    };
    let v = vs.inner().clone();
    let rt = tokio::runtime::Handle::current();
    let info = tokio::task::spawn_blocking(move || v.start(app, conns, rt, target))
        .await
        .map_err(|e| e.to_string())?
        .map_err(|e| e.to_string())?;
    Ok(serde_json::json!({
        "w": info.w,
        "h": info.h,
        "fps": info.fps,
        "monitor": info.monitor,
        "monitorFound": info.monitor_found,
    }))
}

#[tauri::command]
fn video_share_stop(vs: State<'_, video::VideoShare>) {
    vs.stop();
}

/// Moniteurs disponibles pour le partage natif (picker au clic sur 🖥️).
#[tauri::command]
fn video_list_monitors() -> Vec<serde_json::Value> {
    video::list_monitors()
}

/// Fenêtres partageables (picker) : { id (HWND), name, pid }.
#[tauri::command]
fn video_list_windows() -> Vec<serde_json::Value> {
    video::list_windows()
}

/// La WebView s'abonne au flux vidéo natif entrant (un canal binaire par page).
#[tauri::command]
fn video_receive_attach(
    net: State<'_, Net>,
    vs: State<'_, video::VideoShare>,
    sa: State<'_, audio::ScreenAudio>,
    channel: tauri::ipc::Channel<tauri::ipc::InvokeResponseBody>,
) {
    // Une (ré)attache = page (re)chargée : un partage émetteur encore actif serait
    // invisible et incontrôlable depuis la nouvelle page — on le coupe, ET son
    // demi-frère audio (loopback système) avec : le laisser diffuser TOUT le son du
    // PC sans indication serait une fuite de confidentialité. Sans effet au premier
    // chargement (les deux stop() sont idempotents).
    vs.stop();
    sa.stop();
    net::video_attach(net.inner(), channel);
}

#[tauri::command]
async fn send_gfile(state: State<'_, Net>, members: Vec<String>, path: String) -> Result<(), String> {
    net::send_gfile(state.inner(), members, &path).await.map_err(|e| e.to_string())
}

#[tauri::command]
fn list_audio_devices() -> (Vec<String>, Vec<String>) {
    audio::list_devices()
}

#[tauri::command]
fn set_audio_input(cfg: State<'_, audio::AudioCfg>, name: Option<String>) {
    cfg.set_input(name);
}

#[tauri::command]
fn set_audio_output(cfg: State<'_, audio::AudioCfg>, name: Option<String>) {
    cfg.set_output(name);
}

#[tauri::command]
fn respond_incoming(net: State<'_, Net>, id: u64, accept: bool) {
    net::respond_incoming(&net.incoming, id, accept);
}

#[tauri::command]
fn respond_file(net: State<'_, Net>, id: u64, accept: bool) {
    net::respond_file(&net.settings, id, accept);
}

#[tauri::command]
fn respond_gfile(net: State<'_, Net>, id: u64, accept: bool) {
    net::respond_gfile(&net.settings, id, accept);
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
    *pending.0.lock().unwrap_or_else(|e| e.into_inner()) = update;
    Ok(version)
}

/// Télécharge et installe la mise à jour en attente, puis redémarre l'app.
#[tauri::command]
async fn install_update(
    app: tauri::AppHandle,
    pending: State<'_, PendingUpdate>,
) -> Result<(), String> {
    let update = pending.0.lock().unwrap_or_else(|e| e.into_inner()).take();
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
            app.manage(audio::Voice::default());
            app.manage(audio::Call::default());
            app.manage(audio::GroupCall::default());
            app.manage(audio::ScreenAudio::default());
            app.manage(audio::AudioCfg::default());
            app.manage(video::VideoShare::default());
            // Purge des copies nettoyées (métadonnées) laissées par la session
            // précédente — sinon elles ne partiraient qu'au prochain envoi.
            std::thread::spawn(meta::gc_temp);
            Ok(())
        })
        .invoke_handler(tauri::generate_handler![
            perm_code, eph_code, rotate_eph_code, probe, connect, send_file, send_chat, send_freq, send_faccept, open_group, send_gchat, send_ginvite, send_gmembers, send_kick,
            group_call_start, group_call_stop, group_call_mute, group_call_volume, screen_audio_start, screen_audio_stop, screen_audio_mute, screen_audio_gain, send_signal, send_gfile,
            video_share_start, video_share_stop, video_receive_attach, video_list_monitors, video_list_windows,
            fingerprint, app_version, check_update, install_update, set_download_dir,
            get_download_dir, set_only_friends, set_friends, voice_test_start, voice_test_stop,
            call_start, call_stop, call_set_mute, list_audio_devices, set_audio_input,
            set_audio_output, respond_incoming, respond_file, respond_gfile, disconnect, cancel_send, cancel_recv, set_streams
        ])
        .build(tauri::generate_context!())
        .expect("erreur au lancement de ghost link")
        .run(|app_handle, event| {
            // À la fermeture de l'app, prévenir le pair : fermeture propre de la connexion
            // pour qu'il passe en « déconnecté » immédiatement (au lieu d'attendre un timeout).
            if let tauri::RunEvent::ExitRequested { .. } = event {
                if let Some(net) = app_handle.try_state::<Net>() {
                    let slot = net.slot.clone();
                    tauri::async_runtime::block_on(async move {
                        if let Some(c) = net::current(&slot).await {
                            c.close(0u32.into(), b"bye");
                        }
                        // laisser le temps à la trame de fermeture de partir avant l'arrêt
                        tokio::time::sleep(std::time::Duration::from_millis(200)).await;
                    });
                }
            }
        });
}
