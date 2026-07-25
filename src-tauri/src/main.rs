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

/// Nettoie les métadonnées d'une image de chat AVANT envoi, et rend le résultat visible.
///
/// Sans ceci, coller une photo dans le chat transmettait son EXIF/GPS intact alors que
/// la MÊME photo glissée sur la fenêtre partait nettoyée — et le Journal restait muet,
/// donc l'utilisateur lisait l'absence de message comme « rien à nettoyer ». C'était
/// exactement l'échec silencieux que l'en-tête de meta.rs s'interdit.
///
/// Même contrat que `net::prepare_meta` côté fichier : on n'échoue JAMAIS l'envoi pour
/// un nettoyage raté, mais on ne se tait jamais non plus. L'événement émis est celui que
/// le listener `ghost-meta` de transfer.ts affiche déjà — rien à changer côté UI.
async fn clean_inline_img(app: &tauri::AppHandle, name: &str, data: Vec<u8>) -> Vec<u8> {
    let src = data.clone();
    let prep = tokio::task::spawn_blocking(move || meta::prepare_bytes(&src))
        .await
        .unwrap_or_else(|e| meta::BytesPrep::Failed(format!("préparation interrompue: {e}")));
    match prep {
        meta::BytesPrep::Cleaned(v) => {
            let _ = app.emit("ghost-meta", serde_json::json!({ "name": name, "status": "cleaned" }));
            v
        }
        meta::BytesPrep::Untouched => data,
        meta::BytesPrep::Skipped(info) => {
            let _ = app.emit("ghost-meta", serde_json::json!({ "name": name, "status": "skipped", "info": info }));
            data
        }
        meta::BytesPrep::Failed(info) => {
            let _ = app.emit("ghost-meta", serde_json::json!({ "name": name, "status": "failed", "info": info }));
            data
        }
    }
}

#[tauri::command]
async fn send_img(app: tauri::AppHandle, state: State<'_, Net>, author: String, name: String, mime: String, data: Vec<u8>) -> Result<(), String> {
    let slot = state.slot.clone();
    let data = clean_inline_img(&app, &name, data).await;
    net::send_img(&slot, &author, &name, &mime, &data).await.map_err(|e| e.to_string())
}

// Arguments nombreux mais imposés par le contrat UI (Tauri mappe chaque champ JSON sur un
// paramètre) : les regrouper dans une struct changerait la forme de l'appel côté TypeScript.
#[allow(clippy::too_many_arguments)]
#[tauri::command]
async fn send_gimg(app: tauri::AppHandle, state: State<'_, Net>, members: Vec<String>, gid: String, author: String, name: String, mime: String, data: Vec<u8>) -> Result<(), String> {
    let data = clean_inline_img(&app, &name, data).await;
    net::send_gimg(state.inner(), members, &gid, &author, &name, &mime, &data).await.map_err(|e| e.to_string())
}

/// Repli grosse image : lire les octets d'un fichier REÇU (borné) pour rendu inline via blob.
///
/// CONFINEMENT AU DOSSIER DE RÉCEPTION — indispensable. Les capabilities de l'app sont
/// volontairement minimales (`core:default` + `updater:default` : ni plugin `fs`, ni
/// `shell`, ni `dialog`), donc la WebView n'a AUCUN accès disque générique. Sans la
/// vérification ci-dessous, cette commande réintroduisait à elle seule exactement la
/// capacité que cette configuration interdit : une primitive de lecture arbitraire
/// (identity.key, documents, profils d'autres applications), exploitable par tout script
/// s'exécutant dans la vue. Le seul appel légitime (ui/src/transfer.ts) passe un chemin
/// que NOUS venons de fabriquer et d'émettre dans `ghost-recv-done` : il est toujours
/// dans le dossier de réception, le confinement ne coûte donc rien fonctionnellement.
#[tauri::command]
async fn read_image_bytes(state: State<'_, Net>, path: String) -> Result<Vec<u8>, String> {
    // canonicalize() résout liens, jonctions, « .. » et casse : la comparaison de préfixe
    // porte sur le chemin RÉEL, jamais sur la chaîne fournie par le JS.
    let dossier = std::path::PathBuf::from(net::get_download_dir(&state.settings))
        .canonicalize()
        .map_err(|e| format!("dossier de réception illisible : {e}"))?;
    let cible = std::path::Path::new(&path)
        .canonicalize()
        .map_err(|e| e.to_string())?;
    if !cible.starts_with(&dossier) {
        return Err("chemin hors du dossier de réception".into());
    }
    let meta = std::fs::metadata(&cible).map_err(|e| e.to_string())?;
    if !meta.is_file() {
        return Err("pas un fichier régulier".into());
    }
    if meta.len() > 32 * 1024 * 1024 {
        return Err("image trop grande".into()); // borne
    }
    std::fs::read(&cible).map_err(|e| e.to_string())
}

/// Définit le dossier de réception. Chaîne vide = revenir au défaut (Téléchargements).
///
/// Le chemin vient du JS et devient la RACINE d'écriture de tous les fichiers reçus.
/// `sanitize()` protège le NOM du fichier, pas la racine : sans validation ici, un script
/// dans la vue pouvait pointer le dossier Démarrage de l'utilisateur et transformer le
/// prochain fichier reçu en exécution à l'ouverture de session.
#[tauri::command]
fn set_download_dir(state: State<'_, Net>, path: String) -> Result<(), String> {
    let p = path.trim();
    if !p.is_empty() {
        let chemin = std::path::Path::new(p);
        if !chemin.is_absolute() {
            return Err("le dossier doit être un chemin absolu".into());
        }
        if !chemin.is_dir() {
            return Err("ce dossier n'existe pas".into());
        }
    }
    net::set_download_dir(&state.settings, p);
    Ok(())
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

// Beacon de présence vocale de groupe (~1 Hz, piloté par la TS) : diffuse à tous les
// membres du groupe (en ligne, même hors appel) que je suis (ou ne suis plus) dans le
// vocal, pour afficher une pastille "en appel" sans que chacun rejoigne l'appel.
#[tauri::command]
async fn voice_presence(state: State<'_, Net>, members: Vec<String>, gid: String, in_call: bool) -> Result<(), String> {
    net::send_voice_presence(state.inner(), members, &gid, in_call).await.map_err(|e| e.to_string())
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
// Arguments nombreux mais imposés par le contrat UI (Tauri mappe chaque champ JSON sur un
// paramètre) : les regrouper dans une struct changerait la forme de l'appel côté TypeScript.
#[allow(clippy::too_many_arguments)]
#[tauri::command]
async fn video_share_start(
    net: State<'_, Net>,
    vs: State<'_, video::VideoShare>,
    app: tauri::AppHandle,
    members: Vec<String>,
    monitor: Option<String>,
    window: Option<String>,
    max_fps: Option<u32>,
    max_w: Option<u32>,
    max_h: Option<u32>,
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
    // 0 (ou absent) = illimité → résolution native. Le clamp côté video.rs ne
    // sur-échantillonne jamais : une cible plus grande que l'écran reste au natif.
    let quality = video::Quality {
        fps: max_fps.filter(|f| *f > 0).unwrap_or(60),
        max_w: max_w.unwrap_or(0),
        max_h: max_h.unwrap_or(0),
    };
    let info = tokio::task::spawn_blocking(move || v.start(app, conns, rt, target, quality))
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

/// Nom de fichier annoncé dans le « trusted comment » de la signature minisign.
///
/// Ce commentaire est couvert par la signature GLOBALE (minisign signe
/// `signature || trusted_comment`) : il est AUTHENTIFIÉ, donc infalsifiable sans la clé
/// privée. Format produit par tauri :
///   `trusted comment: timestamp:1784937538\tfile:ghost-link_0.36.1_x64-setup.exe`
fn signed_file_name(signature_b64: &str) -> Option<String> {
    use base64::Engine;
    let raw = base64::engine::general_purpose::STANDARD
        .decode(signature_b64.trim())
        .ok()?;
    let txt = String::from_utf8(raw).ok()?;
    let line = txt.lines().find(|l| l.starts_with("trusted comment:"))?;
    line.split('\t')
        .find_map(|f| f.trim().strip_prefix("file:"))
        .map(|s| s.trim().to_string())
}

/// Cherche une mise à jour. Renvoie la version disponible (ou null), et la garde en attente.
///
/// SÉCURITÉ — liaison version ↔ binaire. La signature minisign prouve seulement que ces
/// octets ont été signés UN JOUR par nous ; elle ne dit RIEN du numéro de version annoncé,
/// qui est un champ libre de `latest.json`. Quiconque obtient un droit d'écriture sur les
/// releases GitHub (compte compromis, jeton `gh` volé) — SANS posséder la clé de signature
/// — pouvait donc republier un ancien installeur légitimement signé sous un numéro élevé,
/// et faire redescendre tout le parc sur une version vulnérable, en `installMode: passive`,
/// c'est-à-dire sans interaction. Le nom de fichier du « trusted comment », lui, est
/// authentifié : on exige qu'il corresponde à la version annoncée.
///
/// FAIL-OPEN DÉLIBÉRÉ si le commentaire est illisible : un futur changement de format
/// minisign ne doit pas figer les mises à jour de tout le parc (le correctif passerait
/// lui-même par une mise à jour...). Ce n'est pas une brèche : la vraie vérification de
/// signature du plugin s'exécute de toute façon au téléchargement et rejette les octets.
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
    if let Some(u) = update.as_ref() {
        if let Some(fichier) = signed_file_name(&u.signature) {
            if !fichier.contains(&u.version) {
                return Err(format!(
                    "mise à jour refusée : le binaire signé est « {fichier} », qui ne correspond pas \
                     à la version annoncée {}. Signale-le — cela ressemble à un rejeu d'un ancien \
                     installeur sous un faux numéro de version.",
                    u.version
                ));
            }
        }
    }
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
    if let Err(e) = update
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
    {
        // Échec (ex. coupure réseau) : remettre la mise à jour en attente pour qu'un
        // second clic « installer » la retrouve, au lieu d'un « aucune mise à jour ».
        *pending.0.lock().unwrap_or_else(|e| e.into_inner()) = Some(update);
        return Err(e.to_string());
    }
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
    // #12 : mode interne « nettoyer un PDF dans un SOUS-PROCESSUS ISOLÉ ». Un PDF hostile
    // (deflate-bomb) qui provoque un OOM n'abort QUE ce sous-processus — l'app parente
    // survit et traite le fichier comme Skipped. DOIT rester la toute première chose de
    // main() (avant l'init Tauri). Voir meta::clean_pdf_file / clean_pdf_worker.
    {
        let args: Vec<String> = std::env::args().collect();
        if args.len() == 4 && args[1] == "--gl-clean-pdf" {
            std::process::exit(meta::clean_pdf_worker(&args[2], &args[3]));
        }
    }
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
            perm_code, eph_code, rotate_eph_code, probe, connect, send_file, send_chat, send_freq, send_faccept, open_group, send_gchat, send_ginvite, send_gmembers, send_kick, send_img, send_gimg, read_image_bytes,
            group_call_start, group_call_stop, group_call_mute, group_call_volume, voice_presence, screen_audio_start, screen_audio_stop, screen_audio_gain, send_signal, send_gfile,
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
                    // Cloner (hors du lock) les connexions du mesh de groupe pour les
                    // fermer aussi : sinon les pairs de groupe voient l'utilisateur
                    // « en ligne » / « en appel » jusqu'au timeout QUIC/CALL_PING.
                    let group_conns: Vec<_> = net
                        .mesh
                        .lock()
                        .unwrap_or_else(|e| e.into_inner())
                        .values()
                        .map(|(_, c)| c.clone())
                        .collect();
                    tauri::async_runtime::block_on(async move {
                        if let Some(c) = net::current(&slot).await {
                            c.close(0u32.into(), b"bye");
                        }
                        for c in &group_conns {
                            c.close(0u32.into(), b"bye");
                        }
                        // laisser le temps aux trames de fermeture de partir avant l'arrêt
                        tokio::time::sleep(std::time::Duration::from_millis(200)).await;
                    });
                }
            }
        });
}

#[cfg(test)]
mod tests {
    use super::signed_file_name;

    /// Signature RÉELLE publiée dans latest.json pour v0.36.1 (clé publique du dépôt).
    /// Sert de référence de format : si tauri change la forme du « trusted comment »,
    /// ce test échoue et signale que le contrôle anti-rejeu est devenu inopérant.
    const SIG_REELLE: &str = "dW50cnVzdGVkIGNvbW1lbnQ6IHNpZ25hdHVyZSBmcm9tIHRhdXJpIHNlY3JldCBrZXkKUlVTdjd2dnU1RlBNRWJES21OamFxR2xJWjRpelkrTFlUZ1JydEJhUHFpU1NYMlZ1R3h6bndWaVR2Qko4SzVpdkRVUWVsSDBmZGlNbWtsd0Q5aUhNSGJpNDVFRlhpeDJlS3cwPQp0cnVzdGVkIGNvbW1lbnQ6IHRpbWVzdGFtcDoxNzg0OTM3NTM4CWZpbGU6Z2hvc3QtbGlua18wLjM2LjFfeDY0LXNldHVwLmV4ZQpZMXJVeUtOR3hzYlcyRUVJOG9jMi9MZERrc2xsYUdkakJISDB4VTVPMzNjUXBNa25xKzF4UkhTa1Y2WTVWQ2FybmJUQ0F6b2lDWmErcU1lNHJrbHRCQT09";

    #[test]
    fn extrait_le_nom_de_fichier_signe() {
        assert_eq!(
            signed_file_name(SIG_REELLE).as_deref(),
            Some("ghost-link_0.36.1_x64-setup.exe")
        );
    }

    #[test]
    fn le_rejeu_d_un_ancien_binaire_est_detectable() {
        // C'est TOUT le contrôle : le nom de fichier signé porte 0.36.1, donc annoncer
        // « 99.0.0 » dans latest.json avec cette signature-là ne peut pas passer.
        let fichier = signed_file_name(SIG_REELLE).unwrap();
        assert!(!fichier.contains("99.0.0"), "le rejeu doit être refusé");
        assert!(fichier.contains("0.36.1"), "la version légitime doit passer");
    }

    #[test]
    fn signature_illisible_ne_bloque_pas_les_mises_a_jour() {
        // Fail-open délibéré : un format inattendu ne doit pas figer le parc, la vraie
        // vérification de signature s'exécutant de toute façon au téléchargement.
        assert!(signed_file_name("pas du base64 !!").is_none());
        assert!(signed_file_name("").is_none());
        // Base64 valide mais sans « trusted comment » : pas de nom → pas de blocage.
        assert!(signed_file_name("aGVsbG8gd29ybGQ=").is_none());
    }
}
