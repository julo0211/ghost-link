// Partage d'écran 100 % NATIF (sans WebRTC, donc sans STUN ni exposition d'IP) :
// Windows.Graphics.Capture (écran principal, BGRA) → conversion NV12 (CPU) →
// encodeur H.264 MATÉRIEL (IMFTransform asynchrone : NVENC/AMF/QSV) → framing par
// image sur UN flux QUIC uni-directionnel iroh par pair du groupe (GKIND_VIDEO).
//
// Toute la plomberie Media Foundation reprend les pièges déjà résolus dans
// experiences-pipeline-natif/exp2 (voir plan-pipeline-natif.md) :
//   MF_TRANSFORM_ASYNC_UNLOCK, SET_D3D_MANAGER obligatoire pour NVENC, type de
//   SORTIE d'abord avec MPEG2_PROFILE=77, type d'ENTRÉE via GetInputAvailableType,
//   boucle NeedInput/HaveOutput en polling, dimensions paires, duplication des
//   trames (WGC ne livre une trame QUE si l'écran change).
//
// Contre-pression : une file bornée PAR PAIR ; si elle déborde (pair lent), on
// saute des trames et on ne reprend qu'à la prochaine image CLÉ (reprendre sur une
// delta corromprait le décodage de tout le reste du GOP).

use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};

use iroh::endpoint::Connection;
use tauri::AppHandle;

/// Cadence par défaut (Auto) : plafond 60 fps (l'adaptatif descend si le réseau sature).
const FPS_DEFAULT: u32 = 60;

/// Supplément de débit au-delà de 50 fps, en % du palier de base. Doubler la cadence ne
/// demande PAS le double de débit sur un partage d'écran (contenu souvent statique).
/// 125 % — et surtout PAS 150 % : à 150 %, le pire cas (1440p60) atteignait 18 Mb/s, soit
/// 2,25 Mio/s, au ras du seau à jetons du relais vidéo (`net::RELAY_RATE`), qui jetait
/// alors les trames — y compris les images clés, donc PLUS RIEN ne s'affichait chez les
/// pairs alors que l'émetteur croyait diffuser normalement.
const FPS_HIGH_BITRATE_PCT: u32 = 125;

/// Débit maximal qu'un partage peut demander à l'encodeur (pire cas : > 1080p à ≥ 50 fps).
/// Sert de CONTRAT avec le relais vidéo de `net.rs` : le seau à jetons doit rester
/// nettement au-dessus, sinon les trames légitimes sont jetées. Verrouillé par un test
/// dans net.rs — ne pas changer les paliers de `bitrate_for_fps` sans le relire.
// Consommée uniquement par le test d'invariant de net.rs : c'est un CONTRAT entre le
// débit vidéo et le budget du relais, pas du code appelé à l'exécution.
#[allow(dead_code)]
pub const MAX_BITRATE_BPS: u32 = 12_000_000 / 100 * FPS_HIGH_BITRATE_PCT;
/// Intervalle d'images clés (secondes) — borne le temps de « prise » d'un pair qui
/// rejoint ou qui a sauté des trames.
const KEYFRAME_SECS: u32 = 2;
/// Profondeur de la file d'envoi par pair (~2 s de vidéo) avant de sauter des trames.
const PEER_QUEUE: usize = 64;

/// Une image encodée, partagée entre tous les pairs (clone = comptage de références).
#[derive(Clone)]
pub struct Frame {
    pub id: u64,
    pub key: bool,
    pub data: bytes::Bytes,
}

/// Partage d'écran natif en cours (au plus un à la fois), sur le modèle de ScreenAudio.
#[derive(Clone, Default)]
pub struct VideoShare {
    flag: Arc<Mutex<Option<Arc<AtomicBool>>>>,
}

/// Écran réellement capturé, remonté à l'UI (nom + « écran demandé trouvé ? »).
#[derive(Clone)]
pub struct StartInfo {
    pub w: u32,
    pub h: u32,
    pub fps: u32,
    pub monitor: String,
    pub monitor_found: bool,
}

/// Ce qu'on partage : un moniteur (par szDevice stable ; None = principal) ou une
/// fenêtre précise (par HWND). Choisi dans le picker au clic sur 🖥️.
#[derive(Clone)]
pub enum ShareTarget {
    Monitor(Option<String>),
    Window(isize),
}

/// Plafond de qualité choisi par l'utilisateur. `max_w`/`max_h` bornent la
/// résolution ENCODÉE (0/0 = illimité = résolution native). Jamais d'upscale :
/// une source déjà sous le plafond n'est jamais agrandie (voir `win::clamp_dims`).
#[derive(Clone, Copy)]
pub struct Quality {
    pub fps: u32,
    pub max_w: u32,
    pub max_h: u32,
}
impl Default for Quality {
    fn default() -> Self { Quality { fps: FPS_DEFAULT, max_w: 0, max_h: 0 } }
}

impl VideoShare {
    /// Démarre (ou redémarre) le partage vers les connexions de maillage données.
    /// `monitor` : szDevice stable renvoyé par `list_monitors` (None = écran principal ;
    /// un szDevice devenu introuvable retombe sur le principal, `monitor_found=false`).
    #[cfg(windows)]
    pub fn start(
        &self,
        app: AppHandle,
        conns: Vec<(String, Connection)>,
        rt: tokio::runtime::Handle,
        target: ShareTarget,
        quality: Quality,
    ) -> anyhow::Result<StartInfo> {
        self.stop();
        let stop = Arc::new(AtomicBool::new(false));
        // Publier le drapeau AVANT l'init : un stop() qui tombe PENDANT l'init
        // (raccrochage de l'appel, filet de group_call_stop) ne doit jamais être
        // perdu — il tuera cette capture dès sa première boucle.
        *self.flag.lock().unwrap_or_else(|e| e.into_inner()) = Some(stop.clone());
        let (ready_tx, ready_rx) = std::sync::mpsc::channel::<Result<(u32, u32, String, bool), String>>();
        {
            let stop = stop.clone();
            std::thread::spawn(move || {
                win::capture_encode_thread(app, conns, rt, stop, ready_tx, target, quality);
            });
        }
        // Init WGC + NVENC : rapide en pratique ; 10 s couvre un GPU occupé.
        match ready_rx.recv_timeout(std::time::Duration::from_secs(10)) {
            Ok(Ok((w, h, monitor, monitor_found))) => {
                Ok(StartInfo { w, h, fps: quality.fps.max(1), monitor, monitor_found })
            }
            Ok(Err(e)) => {
                stop.store(true, Ordering::SeqCst);
                self.clear_if(&stop);
                Err(anyhow::anyhow!(e))
            }
            Err(_) => {
                stop.store(true, Ordering::SeqCst);
                self.clear_if(&stop);
                Err(anyhow::anyhow!("démarrage de la capture d'écran impossible (délai dépassé)"))
            }
        }
    }

    /// Retire notre drapeau de l'emplacement partagé s'il y est encore (échec d'init) —
    /// sans écraser celui d'un partage plus récent démarré entre-temps.
    #[cfg(windows)]
    fn clear_if(&self, stop: &Arc<AtomicBool>) {
        let mut g = self.flag.lock().unwrap_or_else(|e| e.into_inner());
        if g.as_ref().map(|f| Arc::ptr_eq(f, stop)).unwrap_or(false) {
            *g = None;
        }
    }

    #[cfg(not(windows))]
    pub fn start(
        &self,
        _app: AppHandle,
        _conns: Vec<(String, Connection)>,
        _rt: tokio::runtime::Handle,
        _target: ShareTarget,
        _quality: Quality,
    ) -> anyhow::Result<StartInfo> {
        Err(anyhow::anyhow!("partage d'écran natif non disponible sur cette plateforme"))
    }

    /// Coupe le partage en cours (s'il y en a un). Idempotent.
    pub fn stop(&self) {
        if let Some(f) = self.flag.lock().unwrap_or_else(|e| e.into_inner()).take() {
            f.store(true, Ordering::SeqCst);
        }
    }
}

/// Moniteurs disponibles pour le partage, dans l'ordre d'énumération Windows.
/// L'index renvoyé est celui attendu par `VideoShare::start(monitor)`.
#[cfg(windows)]
pub fn list_monitors() -> Vec<serde_json::Value> {
    win::list_monitors()
}

#[cfg(not(windows))]
pub fn list_monitors() -> Vec<serde_json::Value> {
    Vec::new()
}

/// Fenêtres partageables (top-level visibles titrées) : { id (HWND), name, pid }.
#[cfg(windows)]
pub fn list_windows() -> Vec<serde_json::Value> {
    win::list_windows()
}

#[cfg(not(windows))]
pub fn list_windows() -> Vec<serde_json::Value> {
    Vec::new()
}


/// Écrit les images d'un pair sur SON flux QUIC uni-directionnel, dans l'ordre.
/// Framing : [u64 frame_id][u8 flags bit0=keyframe][u32 len][len octets H.264 Annex-B].
/// La fin de la file (partage arrêté) ferme le flux proprement (FIN). Les écritures
/// sont bornées dans le temps : un pair qui ne lit jamais le flux (ancienne version
/// de l'appli, connexion zombie) ne doit pas épingler cette tâche pour toujours.
async fn peer_writer(
    conn: Connection,
    mut rx: tokio::sync::mpsc::Receiver<Frame>,
    stop: Arc<AtomicBool>,
) {
    const WRITE_TIMEOUT: std::time::Duration = std::time::Duration::from_secs(15);
    let opened = tokio::time::timeout(WRITE_TIMEOUT, conn.open_uni()).await;
    let Ok(Ok(mut send)) = opened else { return };
    if !matches!(
        tokio::time::timeout(WRITE_TIMEOUT, send.write_all(&[crate::net::GKIND_VIDEO])).await,
        Ok(Ok(()))
    ) {
        return;
    }
    let mut first = true;
    while let Some(f) = rx.recv().await {
        // Après stop(), la file peut encore contenir des trames : les JETER plutôt
        // que les drainer — elles chevaucheraient le flux du partage suivant.
        if stop.load(Ordering::SeqCst) {
            break;
        }
        let mut hdr = [0u8; 13];
        hdr[..8].copy_from_slice(&f.id.to_be_bytes());
        // bit0 = keyframe ; bit1 = première trame de CE flux (nouvelle session de
        // partage) : le récepteur réinitialise son décodeur là — et seulement là.
        hdr[8] = u8::from(f.key) | if first { 2 } else { 0 };
        first = false;
        hdr[9..13].copy_from_slice(&(f.data.len() as u32).to_be_bytes());
        let write = async {
            send.write_all(&hdr).await?;
            send.write_all(&f.data).await
        };
        if !matches!(tokio::time::timeout(WRITE_TIMEOUT, write).await, Ok(Ok(()))) {
            return;
        }
    }
    let _ = send.finish();
}

/// File d'envoi d'un pair + état de saut (après un débordement, attendre une image clé).
struct PeerOut {
    code: String,
    tx: tokio::sync::mpsc::Sender<Frame>,
    wait_key: bool,
    dead: bool,
}

/// Résultat d'une diffusion : pairs dont le flux vient de mourir (à signaler à
/// l'UI) et signal de congestion (au moins une file pleine — carburant du
/// contrôleur adaptatif de l'étape 3).
#[derive(Default)]
struct DispatchOutcome {
    newly_dead: Vec<String>,
    congested: bool,
}

/// Diffuse une image encodée à tous les pairs, sans jamais bloquer l'encodeur.
fn dispatch(peers: &mut [PeerOut], frame: &Frame) -> DispatchOutcome {
    let mut out = DispatchOutcome::default();
    for p in peers.iter_mut() {
        if p.dead || (p.wait_key && !frame.key) {
            continue;
        }
        match p.tx.try_send(frame.clone()) {
            Ok(()) => p.wait_key = false,
            Err(tokio::sync::mpsc::error::TrySendError::Full(_)) => {
                p.wait_key = true;
                out.congested = true;
            }
            Err(tokio::sync::mpsc::error::TrySendError::Closed(_)) => {
                p.dead = true;
                out.newly_dead.push(p.code.clone());
            }
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    fn f(id: u64, key: bool) -> Frame {
        Frame { id, key, data: bytes::Bytes::from_static(b"x") }
    }

    #[test]
    fn contre_pression_saute_jusqu_a_la_cle() {
        // File de capacité 1 : le débordement doit faire sauter les deltas suivantes
        // et ne reprendre QUE sur une image clé (sinon le GOP est corrompu).
        let (tx, mut rx) = tokio::sync::mpsc::channel::<Frame>(1);
        let mut peers = vec![PeerOut { code: "p1".into(), tx, wait_key: false, dead: false }];
        let o = dispatch(&mut peers, &f(1, true)); // remplit la file
        assert!(o.newly_dead.is_empty() && !o.congested);
        let o = dispatch(&mut peers, &f(2, false)); // Full → wait_key + congestion
        assert!(peers[0].wait_key && o.congested);
        assert_eq!(rx.try_recv().unwrap().id, 1); // on vide la file
        let o = dispatch(&mut peers, &f(3, false)); // delta : sautée malgré la place
        assert!(rx.try_recv().is_err() && !o.congested); // saut silencieux ≠ congestion
        dispatch(&mut peers, &f(4, true)); // keyframe : reprise
        assert!(!peers[0].wait_key);
        let got = rx.try_recv().unwrap();
        assert!(got.key && got.id == 4);
    }

    #[test]
    fn pair_ferme_marque_mort_et_signale() {
        let (tx, rx) = tokio::sync::mpsc::channel::<Frame>(1);
        drop(rx);
        let mut peers = vec![PeerOut { code: "p1".into(), tx, wait_key: false, dead: false }];
        assert_eq!(dispatch(&mut peers, &f(1, true)).newly_dead, vec!["p1".to_string()]);
        assert!(peers[0].dead);
        // Une fois mort : plus jamais re-signalé, et jamais compté congestionné.
        let o = dispatch(&mut peers, &f(2, true));
        assert!(o.newly_dead.is_empty() && !o.congested);
    }

    #[test]
    fn levels_for_scales_relative_to_target() {
        assert_eq!(super::win::levels_for(60)[0].0, 60);
        assert_eq!(super::win::levels_for(30)[0].0, 30);
        // le niveau 0 est toujours 100 % du débit
        assert_eq!(super::win::levels_for(60)[0].1, 100);
        // les crans inférieurs descendent
        assert!(super::win::levels_for(60)[3].0 < 60);
    }

    #[test]
    fn clamp_dims_downscale_4k_vers_1080p() {
        assert_eq!(super::win::clamp_dims(3840, 2160, 1920, 1080), (1920, 1080));
    }

    #[test]
    fn clamp_dims_jamais_d_upscale() {
        // Source déjà sous le plafond : renvoyée telle quelle, jamais agrandie.
        assert_eq!(super::win::clamp_dims(1280, 720, 1920, 1080), (1280, 720));
    }

    #[test]
    fn clamp_dims_illimite() {
        assert_eq!(super::win::clamp_dims(3840, 2160, 0, 0), (3840, 2160));
    }

    #[test]
    fn clamp_dims_ratio_non_16_9() {
        // Source 4:3 VRAIMENT différente du 16:9 du plafond : c'est la hauteur qui doit
        // piloter. (1366×768 ne conviendrait pas : c'est du 16:9 à 0,05 % près, et une
        // inversion de l'axe choisi y donnerait le MÊME résultat — test aveugle.)
        assert_eq!(super::win::clamp_dims(1600, 1200, 1280, 720), (960, 720));
        // Aucun axe ne doit jamais dépasser le plafond demandé.
        for &(nw, nh) in &[(1600u32, 1200u32), (3840, 2160), (1366, 768), (2560, 1080)] {
            let (w, h) = super::win::clamp_dims(nw, nh, 1280, 720);
            assert!(w <= 1280 && h <= 720, "{nw}x{nh} -> {w}x{h} dépasse le plafond");
            assert_eq!((w % 2, h % 2), (0, 0), "dimensions paires (NV12)");
        }
    }

    #[test]
    fn clamp_dims_plancher_encodeur() {
        // Source très étirée (bandeau 4000×70) : le fit sur le plafond ferait
        // tomber sous 64px de haut → plancher = pas de clamp du tout.
        assert_eq!(super::win::clamp_dims(4000, 70, 1280, 720), (4000, 70));
    }
}

#[cfg(windows)]
mod win {
    use super::{
        dispatch, peer_writer, Frame, PeerOut, FPS_HIGH_BITRATE_PCT, KEYFRAME_SECS, PEER_QUEUE,
    };
    #[cfg(test)]
    use super::FPS_DEFAULT;
    use std::mem::ManuallyDrop;
    use std::sync::atomic::{AtomicBool, Ordering};
    use std::sync::Arc;
    use std::time::{Duration, Instant};

    use iroh::endpoint::Connection;
    use tauri::{AppHandle, Emitter};
    use windows::core::Interface;
    use windows::Graphics::Capture::{Direct3D11CaptureFramePool, GraphicsCaptureItem};
    use windows::Graphics::DirectX::DirectXPixelFormat;
    use windows::Win32::Foundation::POINT;
    use windows::Win32::Graphics::Direct3D::D3D_DRIVER_TYPE_HARDWARE;
    use windows::Win32::Graphics::Direct3D10::ID3D10Multithread;
    use windows::Win32::Graphics::Direct3D11::{
        D3D11CreateDevice, ID3D11Device, ID3D11DeviceContext, ID3D11Texture2D,
        D3D11_BOX, D3D11_CPU_ACCESS_READ, D3D11_CREATE_DEVICE_BGRA_SUPPORT,
        D3D11_CREATE_DEVICE_VIDEO_SUPPORT, D3D11_MAPPED_SUBRESOURCE, D3D11_MAP_READ,
        D3D11_SDK_VERSION, D3D11_TEXTURE2D_DESC, D3D11_USAGE_STAGING,
    };
    use windows::Win32::Foundation::{BOOL, HWND, LPARAM, RECT};
    use windows::Win32::UI::WindowsAndMessaging::{
        EnumChildWindows, EnumWindows, GetWindowTextLengthW, GetWindowTextW,
        GetWindowThreadProcessId, IsWindow, IsWindowVisible,
    };
    use windows::Win32::Graphics::Dxgi::Common::{DXGI_FORMAT_B8G8R8A8_UNORM, DXGI_SAMPLE_DESC};
    use windows::Win32::Graphics::Dxgi::IDXGIDevice;
    use windows::Win32::Graphics::Gdi::{
        EnumDisplayMonitors, GetMonitorInfoW, MonitorFromPoint, HDC, HMONITOR, MONITORINFO,
        MONITORINFOEXW, MONITOR_DEFAULTTOPRIMARY,
    };
    /// MONITORINFOF_PRIMARY (winuser.h) — absent des bindings windows 0.58.
    const MONITORINFOF_PRIMARY: u32 = 1;
    use windows::Win32::Media::MediaFoundation::{
        ICodecAPI, IMFActivate, IMFDXGIDeviceManager, IMFMediaEventGenerator, IMFSample,
        IMFTransform, MFCreateDXGIDeviceManager, MFCreateMediaType, MFCreateMemoryBuffer,
        MFCreateSample, MFShutdown, MFStartup, MFTEnumEx, METransformHaveOutput,
        METransformNeedInput, MFMediaType_Video, MFSampleExtension_CleanPoint,
        MFT_CATEGORY_VIDEO_ENCODER, MFT_ENUM_FLAG, MFT_ENUM_FLAG_HARDWARE,
        MFT_ENUM_FLAG_SORTANDFILTER, MFT_MESSAGE_NOTIFY_BEGIN_STREAMING,
        MFT_MESSAGE_NOTIFY_END_OF_STREAM, MFT_MESSAGE_NOTIFY_START_OF_STREAM,
        MEError, MFT_MESSAGE_SET_D3D_MANAGER, MFT_OUTPUT_DATA_BUFFER, MFT_REGISTER_TYPE_INFO,
        MF_EVENT_FLAG_NO_WAIT, MF_E_NO_EVENTS_AVAILABLE, MF_MT_AVG_BITRATE,
        MF_MT_DEFAULT_STRIDE, MF_MT_FRAME_RATE, MF_MT_FRAME_SIZE, MF_MT_INTERLACE_MODE,
        MF_MT_MAJOR_TYPE, MF_MT_MPEG2_PROFILE, MF_MT_SUBTYPE, MF_TRANSFORM_ASYNC_UNLOCK,
        MF_VERSION, MFSTARTUP_FULL, MFVideoFormat_H264, MFVideoFormat_NV12,
        MFVideoInterlace_Progressive, CODECAPI_AVEncCommonMeanBitRate, CODECAPI_AVEncMPVGOPSize,
    };
    use windows::Win32::System::Com::CoTaskMemFree;
    use windows::Win32::System::WinRT::Direct3D11::{
        CreateDirect3D11DeviceFromDXGIDevice, IDirect3DDxgiInterfaceAccess,
    };
    use windows::Win32::System::WinRT::Graphics::Capture::IGraphicsCaptureItemInterop;
    use windows::Win32::System::WinRT::{RoInitialize, RO_INIT_MULTITHREADED};

    /// Débit cible selon la résolution ET la cadence (au-delà de 50 fps, plus de
    /// mouvement à encoder par seconde → ~×1.5 pour ne pas sous-bitrater du 60 fps).
    fn bitrate_for_fps(w: u32, h: u32, fps: u32) -> u32 {
        let base = {
            let px = w * h;
            if px > 1920 * 1080 {
                12_000_000
            } else if px > 1280 * 720 {
                8_000_000
            } else {
                // ≤720p (y compris 1280×720 EXACT, qui échoue le test précédent).
                // On NE baisse PAS ce palier : `bitrate_for_fps` ne voit que (w,h) et ne
                // peut pas distinguer « l'utilisateur a choisi 720p » d'un écran 720p
                // natif ou d'un partage de FENÊTRE (souvent du texte, où une baisse de
                // débit se voit tout de suite sur les glyphes). L'économie vient déjà du
                // passage de 12/8 → 5 Mb/s. Paliers du dessus : validés matériel.
                5_000_000
            }
        };
        if fps >= 50 {
            base / 100 * FPS_HIGH_BITRATE_PCT
        } else {
            base
        }
    }

    /// Échelle d'adaptation (étape 3), relative à la cadence cible choisie (niveau 0 =
    /// cible/100 % du débit). Niveau 0 = qualité max ; on descend d'un cran quand les
    /// files des pairs débordent (réseau saturé), on remonte d'un cran après 12 s de
    /// calme. Le débit NVENC est reconfiguré À CHAUD (ICodecAPI) — validé sur cette
    /// machine par le smoke test matériel.
    pub(super) fn levels_for(fps: u32) -> [(u32, u32); 4] {
        let f = fps.max(1);
        [
            (f, 100),
            ((f * 2 / 3).max(1), 66),
            ((f * 2 / 5).max(1), 40),
            ((f / 4).max(1), 25),
        ]
    }

    /// Poignées des moniteurs, dans l'ordre d'énumération Windows.
    fn monitor_handles() -> Vec<isize> {
        unsafe extern "system" fn cb(h: HMONITOR, _dc: HDC, _rc: *mut RECT, lp: LPARAM) -> BOOL {
            let v = &mut *(lp.0 as *mut Vec<isize>);
            v.push(h.0 as isize);
            true.into()
        }
        let mut out: Vec<isize> = Vec::new();
        unsafe {
            let _ = EnumDisplayMonitors(None, None, Some(cb), LPARAM(&mut out as *mut _ as isize));
        }
        out
    }

    /// Lit le szDevice ("\\.\DISPLAYn") d'un moniteur — c'est notre IDENTITÉ STABLE :
    /// contrairement à l'index d'énumération, elle ne change PAS si un autre écran est
    /// branché/débranché entre deux partages (sinon on capturerait le mauvais écran).
    unsafe fn monitor_device(h: isize) -> Option<String> {
        let mut info = MONITORINFOEXW::default();
        info.monitorInfo.cbSize = std::mem::size_of::<MONITORINFOEXW>() as u32;
        if !GetMonitorInfoW(HMONITOR(h as *mut _), &mut info.monitorInfo as *mut MONITORINFO).as_bool() {
            return None;
        }
        Some(String::from_utf16_lossy(&info.szDevice).trim_end_matches('\0').to_string())
    }

    /// "\\.\DISPLAY3" → « Écran 3 » (numéro système, indépendant de l'ordre d'énum).
    fn monitor_label(dev: &str, fallback_idx: usize) -> String {
        let num = dev.rfind(|c: char| !c.is_ascii_digit()).map(|p| &dev[p + 1..]).unwrap_or("");
        if num.is_empty() { format!("Écran {}", fallback_idx + 1) } else { format!("Écran {num}") }
    }

    /// Résout l'écran à capturer par son szDevice stable. Renvoie (handle, label,
    /// found) : found=false = szDevice demandé introuvable → repli sur le principal
    /// (l'appelant en avertit l'UI au lieu de diffuser le mauvais écran en silence).
    unsafe fn resolve_monitor(want: Option<&str>) -> (HMONITOR, String, bool) {
        let handles = monitor_handles();
        if let Some(dev) = want {
            for (i, &h) in handles.iter().enumerate() {
                if monitor_device(h).as_deref() == Some(dev) {
                    return (HMONITOR(h as *mut _), monitor_label(dev, i), true);
                }
            }
        }
        // Défaut / non trouvé : écran principal.
        let hprim = MonitorFromPoint(POINT { x: 0, y: 0 }, MONITOR_DEFAULTTOPRIMARY);
        let idx = handles.iter().position(|&h| h == hprim.0 as isize).unwrap_or(0);
        let label = monitor_device(hprim.0 as isize)
            .map(|d| monitor_label(&d, idx))
            .unwrap_or_else(|| "Écran principal".into());
        (hprim, label, want.is_none())
    }

    /// Description des moniteurs pour l'UI : { id (szDevice), name, w, h, primary }.
    pub(super) fn list_monitors() -> Vec<serde_json::Value> {
        monitor_handles()
            .into_iter()
            .enumerate()
            .filter_map(|(i, h)| unsafe {
                let mut info = MONITORINFOEXW::default();
                info.monitorInfo.cbSize = std::mem::size_of::<MONITORINFOEXW>() as u32;
                if !GetMonitorInfoW(HMONITOR(h as *mut _), &mut info.monitorInfo as *mut MONITORINFO).as_bool() {
                    return None;
                }
                let r = info.monitorInfo.rcMonitor;
                let dev = String::from_utf16_lossy(&info.szDevice).trim_end_matches('\0').to_string();
                Some(serde_json::json!({
                    "id": dev,
                    "name": monitor_label(&dev, i),
                    "w": r.right - r.left,
                    "h": r.bottom - r.top,
                    "primary": info.monitorInfo.dwFlags & MONITORINFOF_PRIMARY != 0,
                }))
            })
            .collect()
    }

    /// Titre d'une fenêtre (chaîne vide si sans titre).
    unsafe fn window_title(h: HWND) -> String {
        let len = GetWindowTextLengthW(h);
        if len <= 0 {
            return String::new();
        }
        let mut buf = vec![0u16; len as usize + 1];
        let n = GetWindowTextW(h, &mut buf);
        String::from_utf16_lossy(&buf[..n as usize])
    }

    /// PID de l'appli pour capter SON son. Les fenêtres UWP/Store sont hébergées par
    /// ApplicationFrameHost.exe : le PID top-level est alors celui de l'HÔTE (qui ne
    /// joue aucun son), et la vraie appli est une fenêtre ENFANT (CoreWindow) d'un
    /// autre process. On cherche donc un enfant dont le PID diffère de l'hôte ; sinon
    /// (appli Win32 classique) le PID top-level est déjà le bon.
    unsafe fn app_pid_of(h: HWND, host_pid: u32) -> u32 {
        unsafe extern "system" fn child_cb(h: HWND, lp: LPARAM) -> BOOL {
            let data = &mut *(lp.0 as *mut (u32, u32)); // (host_pid, résultat)
            let mut pid = 0u32;
            GetWindowThreadProcessId(h, Some(&mut pid));
            if pid != 0 && pid != data.0 {
                data.1 = pid;
                return false.into(); // trouvé → arrêter l'énumération
            }
            true.into()
        }
        let mut data = (host_pid, 0u32);
        let _ = EnumChildWindows(h, Some(child_cb), LPARAM(&mut data as *mut _ as isize));
        if data.1 != 0 {
            data.1
        } else {
            host_pid
        }
    }

    /// Fenêtres partageables pour l'UI : top-level VISIBLES et TITRÉES (on saute la
    /// nôtre). { id (HWND en décimal), name (titre), pid (résolu, cf. UWP) }.
    pub(super) fn list_windows() -> Vec<serde_json::Value> {
        unsafe extern "system" fn cb(h: HWND, lp: LPARAM) -> BOOL {
            let out = &mut *(lp.0 as *mut Vec<serde_json::Value>);
            if IsWindowVisible(h).as_bool() {
                let title = window_title(h);
                if !title.is_empty() && title != "ghost link" {
                    let mut host_pid = 0u32;
                    GetWindowThreadProcessId(h, Some(&mut host_pid));
                    out.push(serde_json::json!({
                        "id": (h.0 as isize).to_string(),
                        "name": title,
                        "pid": app_pid_of(h, host_pid),
                    }));
                }
            }
            true.into()
        }
        let mut out: Vec<serde_json::Value> = Vec::new();
        unsafe {
            let _ = EnumWindows(Some(cb), LPARAM(&mut out as *mut _ as isize));
        }
        out
    }

    /// Plafonne (nw,nh) à (mw,mh) EN GARDANT LE RATIO, sans jamais upscaler.
    /// mw==0||mh==0 = illimité. Si (nw,nh) est déjà sous le plafond, renvoyé tel
    /// quel (jamais d'agrandissement). Dimensions toujours PAIRES (NV12). Si le
    /// fit descend sous le plancher de l'encodeur matériel (64px, un côté très
    /// étiré p.ex.), on renonce au clamp plutôt que produire une image invalide.
    pub(super) fn clamp_dims(nw: u32, nh: u32, mw: u32, mh: u32) -> (u32, u32) {
        let (nw, nh) = (nw & !1, nh & !1);
        if mw == 0 || mh == 0 || nw == 0 || nh == 0 || (nw <= mw && nh <= mh) {
            return (nw, nh); // illimité, ou déjà sous le plafond : JAMAIS d'upscale
        }
        let fit_w = (mw as u64) * (nh as u64) <= (mh as u64) * (nw as u64);
        let (mut w, mut h) = if fit_w {
            (mw as u64, (mw as u64 * nh as u64 + nw as u64 / 2) / nw as u64)
        } else {
            ((mh as u64 * nw as u64 + nh as u64 / 2) / nh as u64, mh as u64)
        };
        w &= !1;
        h &= !1;
        if w < 64 || h < 64 {
            return (nw, nh); // plancher encodeur
        }
        (w as u32, h as u32)
    }

    /// Pas de rééchantillonnage source→sortie, en Q16.16 (65536 = 1:1). Calculé
    /// UNE SEULE fois au démarrage de la capture (`build_capture`) à partir des
    /// dimensions natives et des dimensions encodées, puis reste FIXE toute la
    /// session : le recalculer par trame ferait « pomper » l'image à chaque
    /// redimensionnement de la fenêtre/l'écran partagé.
    #[derive(Clone, Copy)]
    pub(super) struct Scale {
        step_x: u32,
        step_y: u32,
    }
    impl Scale {
        pub(super) fn new(src_w: u32, src_h: u32, dst_w: u32, dst_h: u32) -> Self {
            let step = |s: u32, d: u32| -> u32 {
                if d == 0 { 65536 } else { (((s as u64) << 16) / d as u64) as u32 }
            };
            Scale { step_x: step(src_w, dst_w), step_y: step(src_h, dst_h) }
        }
        fn is_identity(self) -> bool {
            self.step_x == 65536 && self.step_y == 65536
        }
    }

    /// Convertit une largeur/hauteur COUVERTE de l'espace SOURCE (capture) vers l'espace
    /// SORTIE (encodeur), avec le facteur d'échelle FIXE de la session. Fonction à part
    /// — et non recopiée dans `grab_latest` — pour qu'un test puisse l'exercer
    /// directement : c'est exactement la classe de désynchronisation capture/sortie qui
    /// avait fait retirer la mise à l'échelle par le passé. Résultat PAIR (NV12) et
    /// borné par la dimension encodée.
    pub(super) fn cov_out(src: u32, native: u32, enc: u32) -> u32 {
        if native == 0 {
            return 0;
        }
        ((src as u64 * enc as u64 / native as u64) as u32 & !1).min(enc)
    }

    /// Réciproques 1/n en Q20 (`RECIP_Q20[n] ≈ 2^20 / n`), pour remplacer les divisions
    /// entières de la moyenne d'aire par des multiplications — voir la note PERF dans
    /// `box_avg`. Dimensionnée large : l'aire d'une boîte reste petite en pratique
    /// (4K→1080p = 2×2, 4K→720p = 3×3), et au-delà `box_avg` retombe sur la division.
    static RECIP_Q20: [u32; 256] = {
        let mut t = [0u32; 256];
        let mut i = 1usize;
        while i < 256 {
            t[i] = ((1u64 << 20) / i as u64) as u32;
            i += 1;
        }
        t
    };

    /// Moyenne d'aire (filtre BOÎTE) de la boîte source qui correspond au pixel de
    /// SORTIE (ox,oy) sous l'échelle `sc`. Bornée EXPLICITEMENT par `src_cov_w` /
    /// `src_cov_h` (jamais de lecture au-delà — `src` pointe une texture mappée,
    /// un off-by-one y lirait des pixels périmés/hors zone). Filtre boîte
    /// obligatoire (PAS de plus-proche-voisin) : un partage d'écran c'est du
    /// texte, le nearest fait scintiller les glyphes et explose le débit H.264.
    /// Bornes source `[s0, s1)` d'une colonne (ou ligne) de SORTIE. Calculées UNE fois
    /// par trame pour chaque colonne/ligne (O(w+h)) au lieu d'être recalculées pour
    /// chacun des w×h pixels : ce recalcul (multiplications 64 bits + clamps par pixel)
    /// coûtait à lui seul l'essentiel du temps de conversion.
    fn spans(n_out: usize, step: u32, cov: usize) -> Vec<(u32, u32)> {
        (0..n_out)
            .map(|i| {
                let s0 = ((i as u64 * step as u64) >> 16) as usize;
                let s1 = ((((i + 1) as u64 * step as u64) >> 16) as usize)
                    .max(s0 + 1)
                    .min(cov);
                // saturating : `cov == 0` (zone non couverte) donne une boîte vide, que
                // box_avg traite comme du noir — jamais d'underflow.
                let s0 = s0.min(s1.saturating_sub(1));
                (s0 as u32, s1 as u32)
            })
            .collect()
    }

    #[inline(always)]
    unsafe fn box_avg(
        src: *const u8,
        pitch: usize,
        (sx0, sx1): (usize, usize),
        (sy0, sy1): (usize, usize),
    ) -> (i32, i32, i32) {
        if sx1 <= sx0 || sy1 <= sy0 {
            return (0, 0, 0); // hors couverture
        }
        // Cas DOMINANT d'un downscale modéré (2K→1080p : facteur 1,33) : la boîte ne
        // couvre qu'un seul pixel → lecture directe, ni accumulation ni moyenne.
        if sx1 - sx0 == 1 && sy1 - sy0 == 1 {
            let p = src.add(sy0 * pitch + sx0 * 4);
            return (*p.add(2) as i32, *p.add(1) as i32, *p as i32);
        }
        let mut rs = 0i32;
        let mut gs = 0i32;
        let mut bs = 0i32;
        for sy in sy0..sy1 {
            // Arithmétique de pointeur directe : reconstruire un slice par ligne ET par
            // pixel de sortie empêchait toute optimisation de la boucle.
            let mut p = src.add(sy * pitch + sx0 * 4);
            for _ in sx0..sx1 {
                bs += *p as i32;
                gs += *p.add(1) as i32;
                rs += *p.add(2) as i32;
                p = p.add(4);
            }
        }
        // L'aire se déduit des bornes : inutile de compter pixel par pixel.
        let n = (sx1 - sx0) * (sy1 - sy0);
        // PERF : diviser ici coûterait 3 divisions entières par pixel de SORTIE (~6 M
        // par trame en 1080p). On multiplie par une réciproque pré-calculée (arrondi au
        // plus proche, donc au moins aussi juste qu'une division tronquée) ; au-delà de
        // la table on retombe sur la division. Les sommes tiennent en i32 : rs×r vaut
        // toujours ≈ 255 × 2^20, très en dessous de la limite.
        if n < RECIP_Q20.len() {
            let r = RECIP_Q20[n] as i32;
            const HALF: i32 = 1 << 19;
            return (
                (rs.wrapping_mul(r).wrapping_add(HALF)) >> 20,
                (gs.wrapping_mul(r).wrapping_add(HALF)) >> 20,
                (bs.wrapping_mul(r).wrapping_add(HALF)) >> 20,
            );
        }
        let n = n as i32;
        (rs / n, gs / n, bs / n)
    }

    /// Conversion BGRA (avec pitch) → NV12 (BT.709, plage limitée), par blocs de
    /// sortie 2×2, avec mise à l'échelle CPU optionnelle (filtre boîte).
    /// `w`/`h` = dimensions du tampon ENCODEUR (dimensions de SORTIE, PAIRES).
    /// `cov_w`/`cov_h` (pairs, ≤ w/h) = zone couverte par la source, EN ESPACE
    /// SORTIE : au-delà on écrit du noir — sert au letterbox/crop d'une fenêtre
    /// redimensionnée dans le tampon FIXE de l'encodeur.
    /// `sc` = échelle source→sortie (voir `Scale`) ; `src_cov_w`/`src_cov_h` =
    /// bornes DURES de lecture, EN ESPACE SOURCE (taille réelle de la zone
    /// couverte dans la texture mappée — jamais dépassées).
    /// Chemin rapide : si `sc` est l'identité, ce code fait EXACTEMENT ce que
    /// l'ancienne version (1:1, pas de scaler) faisait — zéro régression.
    /// Boucle chaude : voir l'opt-level 1 du profil dev dans Cargo.toml.
    #[allow(clippy::too_many_arguments)] // dimensions capture/sortie/échelle : chacune a un rôle distinct, les regrouper masquerait le contrat (voir doc ci-dessus)
    fn bgra_to_nv12(
        src: *const u8,
        pitch: usize,
        w: usize,
        h: usize,
        cov_w: usize,
        cov_h: usize,
        sc: Scale,
        src_cov_w: usize,
        src_cov_h: usize,
        out: &mut [u8],
    ) {
        if sc.is_identity() {
            bgra_to_nv12_identity(src, pitch, w, h, cov_w, cov_h, out);
            return;
        }
        // Bornes de boîte par colonne / ligne de SORTIE : calculées une seule fois ici
        // (voir `spans`) puis simplement indexées dans la boucle chaude.
        let sx_tab = spans(w, sc.step_x, src_cov_w);
        let sy_tab = spans(h, sc.step_y, src_cov_h);
        let (y_plane, uv_plane) = out.split_at_mut(w * h);
        for by in 0..h / 2 {
            let oy0 = by * 2;
            let uvrow = &mut uv_plane[by * w..(by + 1) * w];
            // Bloc de 2 lignes ENTIÈREMENT hors couverture → noir (Y=16, chroma neutre).
            if oy0 >= cov_h {
                for x in 0..w {
                    y_plane[oy0 * w + x] = 16;
                    y_plane[(oy0 + 1) * w + x] = 16;
                }
                for c in uvrow.iter_mut() {
                    *c = 128;
                }
                continue;
            }
            let (yrow0, yrow1) = {
                let (a, b) = y_plane[oy0 * w..(oy0 + 2) * w].split_at_mut(w);
                (a, b)
            };
            for bx in 0..w / 2 {
                let ox0 = bx * 2;
                // Colonnes hors couverture → noir.
                if ox0 >= cov_w {
                    yrow0[ox0] = 16;
                    yrow0[ox0 + 1] = 16;
                    yrow1[ox0] = 16;
                    yrow1[ox0 + 1] = 16;
                    uvrow[ox0] = 128;
                    uvrow[ox0 + 1] = 128;
                    continue;
                }
                let mut rs = 0i32;
                let mut gs = 0i32;
                let mut bs = 0i32;
                for (dy, yrow) in [(0usize, &mut *yrow0), (1usize, &mut *yrow1)] {
                    let (y0, y1) = sy_tab[oy0 + dy];
                    for dx in 0..2usize {
                        let ox = ox0 + dx;
                        let (x0, x1) = sx_tab[ox];
                        let (r, g, b) = unsafe {
                            box_avg(src, pitch, (x0 as usize, x1 as usize), (y0 as usize, y1 as usize))
                        };
                        rs += r;
                        gs += g;
                        bs += b;
                        // BT.709 limité : Y = 16 + (47R + 157G + 16B) / 256
                        yrow[ox] = (16 + ((47 * r + 157 * g + 16 * b) >> 8)).clamp(0, 255) as u8;
                    }
                }
                // Moyenne 2×2 pour la chroma (rs/gs/bs = sommes de 4 valeurs moyennées).
                let u = 128 + ((-26 * rs - 87 * gs + 112 * bs) >> 10);
                let v = 128 + ((112 * rs - 102 * gs - 10 * bs) >> 10);
                uvrow[ox0] = u.clamp(0, 255) as u8;
                uvrow[ox0 + 1] = v.clamp(0, 255) as u8;
            }
        }
    }

    /// Chemin 1:1 (pas de mise à l'échelle) : code INCHANGÉ par rapport à la
    /// version pré-scaler — un pixel de sortie = un pixel de source, lu
    /// directement (pas de moyenne d'aire). `cover_w`/`cover_h` (pairs, ≤ w/h) =
    /// zone RÉELLEMENT couverte par la source ; au-delà, noir (letterbox/crop
    /// d'une fenêtre redimensionnée dans le tampon FIXE de l'encodeur).
    fn bgra_to_nv12_identity(
        src: *const u8,
        pitch: usize,
        w: usize,
        h: usize,
        cover_w: usize,
        cover_h: usize,
        out: &mut [u8],
    ) {
        let (y_plane, uv_plane) = out.split_at_mut(w * h);
        for by in 0..h / 2 {
            let y0 = by * 2;
            let uvrow = &mut uv_plane[by * w..(by + 1) * w];
            // Bloc de 2 lignes ENTIÈREMENT hors couverture → noir (Y=16, chroma neutre).
            if y0 >= cover_h {
                for x in 0..w {
                    y_plane[y0 * w + x] = 16;
                    y_plane[(y0 + 1) * w + x] = 16;
                }
                for c in uvrow.iter_mut() {
                    *c = 128;
                }
                continue;
            }
            let row0 = unsafe { std::slice::from_raw_parts(src.add(y0 * pitch), cover_w * 4) };
            let row1 = unsafe { std::slice::from_raw_parts(src.add((y0 + 1) * pitch), cover_w * 4) };
            let (yrow0, yrow1) = {
                let (a, b) = y_plane[y0 * w..(y0 + 2) * w].split_at_mut(w);
                (a, b)
            };
            for bx in 0..w / 2 {
                // Colonne hors couverture → noir.
                if bx * 2 >= cover_w {
                    yrow0[bx * 2] = 16;
                    yrow0[bx * 2 + 1] = 16;
                    yrow1[bx * 2] = 16;
                    yrow1[bx * 2 + 1] = 16;
                    uvrow[bx * 2] = 128;
                    uvrow[bx * 2 + 1] = 128;
                    continue;
                }
                let x0 = bx * 2 * 4;
                let mut rs = 0i32;
                let mut gs = 0i32;
                let mut bs = 0i32;
                for (row, yrow) in [(row0, &mut *yrow0), (row1, &mut *yrow1)] {
                    for k in 0..2 {
                        let px = &row[x0 + k * 4..x0 + k * 4 + 4];
                        let b = px[0] as i32;
                        let g = px[1] as i32;
                        let r = px[2] as i32;
                        rs += r;
                        gs += g;
                        bs += b;
                        // BT.709 limité : Y = 16 + (47R + 157G + 16B) / 256
                        yrow[bx * 2 + k] = (16 + ((47 * r + 157 * g + 16 * b) >> 8)).clamp(0, 255) as u8;
                    }
                }
                // Moyenne 2×2 pour la chroma (rs/gs/bs sont des sommes de 4 pixels).
                let u = 128 + ((-26 * rs - 87 * gs + 112 * bs) >> 10);
                let v = 128 + ((112 * rs - 102 * gs - 10 * bs) >> 10);
                uvrow[bx * 2] = u.clamp(0, 255) as u8;
                uvrow[bx * 2 + 1] = v.clamp(0, 255) as u8;
            }
        }
    }

    /// Détection d'image clé de secours : NALU 5 (IDR) ou 7 (SPS) après un start code.
    fn looks_like_keyframe(bytes: &[u8]) -> bool {
        bytes
            .windows(5)
            .any(|w| w[..4] == [0, 0, 0, 1] && matches!(w[4] & 0x1F, 5 | 7))
    }

    struct Encoder {
        activate: IMFActivate,
        transform: IMFTransform,
        gen: IMFMediaEventGenerator,
        /// ICodecAPI de l'encodeur (si exposée) : réglages À CHAUD du débit moyen et
        /// du GOP — le levier du contrôleur adaptatif (étape 3).
        codec_api: Option<ICodecAPI>,
    }

    impl Encoder {
        /// Change le débit moyen cible pendant l'encodage. Best-effort : false si
        /// l'encodeur ne le supporte pas (le contrôleur garde alors le seul levier
        /// fps — moins efficace, mais jamais bloquant).
        unsafe fn set_bitrate(&self, bps: u32) -> bool {
            let Some(ca) = &self.codec_api else { return false };
            let v = windows::core::VARIANT::from(bps);
            ca.SetValue(&CODECAPI_AVEncCommonMeanBitRate, &v).is_ok()
        }

        /// Change l'intervalle d'images clés (en trames) pendant l'encodage.
        unsafe fn set_gop(&self, frames: u32) -> bool {
            let Some(ca) = &self.codec_api else { return false };
            let v = windows::core::VARIANT::from(frames);
            ca.SetValue(&CODECAPI_AVEncMPVGOPSize, &v).is_ok()
        }

        /// Libération COMPLÈTE de l'encodeur matériel : relâcher transform/gen puis
        /// `ShutdownObject` sur l'activation. Sans ça, la session NVENC (limitées à
        /// quelques-unes par GPU GeForce) fuit à CHAQUE partage — au bout de N
        /// partages, plus aucun encodeur matériel disponible sur la machine.
        unsafe fn shutdown(self) {
            let Encoder { activate, transform, gen, codec_api } = self;
            drop(codec_api);
            drop(gen);
            drop(transform);
            let _ = activate.ShutdownObject();
        }
    }

    /// Garde RAII : ShutdownObject sur l'activation si build_encoder échoue APRÈS
    /// ActivateObject (chaque `?` de la configuration fuirait la session NVENC sinon).
    /// Désamorcée (take) sur le chemin de succès — Encoder::shutdown prend le relais.
    struct ActivateGuard(Option<IMFActivate>);
    impl Drop for ActivateGuard {
        fn drop(&mut self) {
            if let Some(a) = self.0.take() {
                unsafe {
                    let _ = a.ShutdownObject();
                }
            }
        }
    }

    /// Active le premier encodeur H.264 MATÉRIEL et le configure (voir pièges exp2).
    /// `fps` : cadence cible (plafond choisi par l'utilisateur) — pilote la cadence
    /// annoncée à l'encodeur, le GOP initial et le facteur du débit de base.
    unsafe fn build_encoder(
        device: &ID3D11Device,
        w: u32,
        h: u32,
        fps: u32,
    ) -> anyhow::Result<Encoder> {
        let reg = MFT_REGISTER_TYPE_INFO {
            guidMajorType: MFMediaType_Video,
            guidSubtype: MFVideoFormat_H264,
        };
        let mut acts: *mut Option<IMFActivate> = std::ptr::null_mut();
        let mut n: u32 = 0;
        MFTEnumEx(
            MFT_CATEGORY_VIDEO_ENCODER,
            MFT_ENUM_FLAG(MFT_ENUM_FLAG_HARDWARE.0 | MFT_ENUM_FLAG_SORTANDFILTER.0),
            None,
            Some(&reg),
            &mut acts,
            &mut n,
        )?;
        anyhow::ensure!(
            n > 0 && !acts.is_null(),
            "aucun encodeur H.264 matériel sur cette machine"
        );
        let list = std::slice::from_raw_parts_mut(acts, n as usize);
        let activate = list[0]
            .clone()
            .ok_or_else(|| anyhow::anyhow!("encodeur matériel inactivable"))?;
        let transform: IMFTransform = activate.ActivateObject()?;
        // Dès ici, tout échec (`?`) doit rendre la session NVENC : garde RAII.
        let mut guard = ActivateGuard(Some(activate));
        // Relâcher TOUTES les activations énumérées (MFTEnumEx les a AddRef-ées ;
        // notre clone de la première garde la sienne) puis libérer le tableau.
        for a in list.iter_mut() {
            std::ptr::drop_in_place(a);
        }
        CoTaskMemFree(Some(acts as *const _));

        // Mode asynchrone + device D3D (NVENC refuse de travailler sans les deux).
        let attrs = transform.GetAttributes()?;
        attrs.SetUINT32(&MF_TRANSFORM_ASYNC_UNLOCK, 1)?;
        let mut reset_token = 0u32;
        let mut mgr: Option<IMFDXGIDeviceManager> = None;
        MFCreateDXGIDeviceManager(&mut reset_token, &mut mgr)?;
        let mgr = mgr.ok_or_else(|| anyhow::anyhow!("DXGIDeviceManager indisponible"))?;
        mgr.ResetDevice(device, reset_token)?;
        let mgr_ptr: usize = std::mem::transmute_copy(&mgr);
        transform.ProcessMessage(MFT_MESSAGE_SET_D3D_MANAGER, mgr_ptr)?;

        // Type de SORTIE d'abord (profil Main explicite), puis ENTRÉE NV12 énumérée.
        let out_ty = MFCreateMediaType()?;
        out_ty.SetGUID(&MF_MT_MAJOR_TYPE, &MFMediaType_Video)?;
        out_ty.SetGUID(&MF_MT_SUBTYPE, &MFVideoFormat_H264)?;
        out_ty.SetUINT32(&MF_MT_AVG_BITRATE, bitrate_for_fps(w, h, fps))?;
        out_ty.SetUINT64(&MF_MT_FRAME_SIZE, ((w as u64) << 32) | h as u64)?;
        out_ty.SetUINT64(&MF_MT_FRAME_RATE, ((fps as u64) << 32) | 1)?;
        out_ty.SetUINT32(&MF_MT_INTERLACE_MODE, MFVideoInterlace_Progressive.0 as u32)?;
        out_ty.SetUINT32(&MF_MT_MPEG2_PROFILE, 77)?; // H.264 Main
        transform
            .SetOutputType(0, &out_ty, 0)
            .map_err(|e| anyhow::anyhow!("SetOutputType: {e}"))?;
        let mut in_ty = None;
        for i in 0.. {
            match transform.GetInputAvailableType(0, i) {
                Ok(t) => {
                    if t.GetGUID(&MF_MT_SUBTYPE)? == MFVideoFormat_NV12 {
                        in_ty = Some(t);
                        break;
                    }
                }
                Err(_) => break,
            }
        }
        let in_ty = in_ty.ok_or_else(|| anyhow::anyhow!("l'encodeur ne propose pas NV12"))?;
        in_ty.SetUINT64(&MF_MT_FRAME_SIZE, ((w as u64) << 32) | h as u64)?;
        in_ty.SetUINT64(&MF_MT_FRAME_RATE, ((fps as u64) << 32) | 1)?;
        in_ty.SetUINT32(&MF_MT_DEFAULT_STRIDE, w)?;
        transform
            .SetInputType(0, &in_ty, 0)
            .map_err(|e| anyhow::anyhow!("SetInputType: {e}"))?;

        // ICodecAPI : GOP initial + leviers à chaud du contrôleur adaptatif
        // (best-effort : tous les encodeurs ne l'exposent pas).
        let codec_api = transform.cast::<ICodecAPI>().ok();
        if let Some(ca) = &codec_api {
            let gop = windows::core::VARIANT::from(fps * KEYFRAME_SECS);
            let _ = ca.SetValue(&CODECAPI_AVEncMPVGOPSize, &gop);
        }

        let gen: IMFMediaEventGenerator = transform.cast()?;
        transform.ProcessMessage(MFT_MESSAGE_NOTIFY_BEGIN_STREAMING, 0)?;
        transform.ProcessMessage(MFT_MESSAGE_NOTIFY_START_OF_STREAM, 0)?;
        // Succès : désamorcer la garde, Encoder::shutdown fera le ShutdownObject.
        let activate = guard.0.take().expect("garde d'activation déjà consommée");
        Ok(Encoder { activate, transform, gen, codec_api })
    }

    struct Capture {
        pool: Direct3D11CaptureFramePool,
        session: windows::Graphics::Capture::GraphicsCaptureSession,
        staging: ID3D11Texture2D,
        ctx: ID3D11DeviceContext,
        /// Dimensions de CAPTURE (natives : ce que WGC délivre, taille de la
        /// texture `staging` mappée). PAS ce qui part sur le réseau — voir `enc_*`.
        w: u32,
        h: u32,
        /// Dimensions ENCODÉES (ce que `build_encoder`/`encode_loop` utilisent,
        /// ce qui sort réellement de l'encodeur et part sur le réseau) — issues
        /// de `clamp_dims(w, h, quality.max_w, quality.max_h)`. Égales à `w`/`h`
        /// quand aucune mise à l'échelle n'est demandée (Quality::max_w/h = 0).
        enc_w: u32,
        enc_h: u32,
        /// Échelle source (w×h) → sortie (enc_w×enc_h), calculée UNE fois ici et
        /// figée pour toute la session (voir `Scale`).
        scale: Scale,
        /// Nom de l'écran RÉELLEMENT capturé (pour l'UI de l'émetteur).
        label: String,
        /// false = l'écran demandé (szDevice) était introuvable → on a replié sur le
        /// principal ; l'UI l'annonce pour ne pas diffuser le mauvais écran en silence.
        found: bool,
        /// Posé par l'événement Closed de l'item WGC (moniteur débranché, session
        /// terminée par le système). Sans lui, la mort de la source serait
        /// indiscernable d'un écran statique : image figée diffusée pour toujours.
        closed: Arc<AtomicBool>,
    }

    /// Capture WGC de la CIBLE demandée (moniteur par szDevice stable, ou fenêtre par
    /// HWND) + texture de relecture CPU (dimensions paires). `quality` fixe le plafond
    /// de résolution ENCODÉE (`clamp_dims`) — calculé UNE fois ici, figé pour la session.
    unsafe fn build_capture(
        device: &ID3D11Device,
        target: &super::ShareTarget,
        quality: super::Quality,
    ) -> anyhow::Result<Capture> {
        let ctx = device.GetImmediateContext()?;
        let dxgi: IDXGIDevice = device.cast()?;
        let inspectable = CreateDirect3D11DeviceFromDXGIDevice(&dxgi)?;
        let winrt_dev: windows::Graphics::DirectX::Direct3D11::IDirect3DDevice =
            inspectable.cast()?;
        let interop = windows::core::factory::<GraphicsCaptureItem, IGraphicsCaptureItemInterop>()?;
        let (item, label, found): (GraphicsCaptureItem, String, bool) = match target {
            super::ShareTarget::Monitor(dev) => {
                let (hmon, label, found) = resolve_monitor(dev.as_deref());
                (interop.CreateForMonitor(hmon)?, label, found)
            }
            super::ShareTarget::Window(hwnd) => {
                let h = HWND(*hwnd as *mut _);
                if !IsWindow(h).as_bool() {
                    anyhow::bail!("fenêtre introuvable (fermée ?) — relance le partage");
                }
                let title = window_title(h);
                let label = if title.is_empty() { "Fenêtre".to_string() } else { title };
                (interop.CreateForWindow(h)?, label, true)
            }
        };
        let size = item.Size()?;
        let w = (size.Width as u32) & !1;
        let h = (size.Height as u32) & !1;
        anyhow::ensure!(w >= 64 && h >= 64, "écran trop petit pour être capturé");
        let pool = Direct3D11CaptureFramePool::CreateFreeThreaded(
            &winrt_dev,
            DirectXPixelFormat::B8G8R8A8UIntNormalized,
            2,
            size,
        )?;
        let session = pool.CreateCaptureSession(&item)?;
        let closed = Arc::new(AtomicBool::new(false));
        {
            let closed = closed.clone();
            item.Closed(&windows::Foundation::TypedEventHandler::new(move |_, _| {
                closed.store(true, Ordering::SeqCst);
                Ok(())
            }))?;
        }
        session.StartCapture()?;
        let desc = D3D11_TEXTURE2D_DESC {
            Width: w,
            Height: h,
            MipLevels: 1,
            ArraySize: 1,
            Format: DXGI_FORMAT_B8G8R8A8_UNORM,
            SampleDesc: DXGI_SAMPLE_DESC { Count: 1, Quality: 0 },
            Usage: D3D11_USAGE_STAGING,
            BindFlags: 0,
            CPUAccessFlags: D3D11_CPU_ACCESS_READ.0 as u32,
            MiscFlags: 0,
        };
        let mut staging: Option<ID3D11Texture2D> = None;
        device.CreateTexture2D(&desc, None, Some(&mut staging))?;
        let staging = staging.ok_or_else(|| anyhow::anyhow!("texture de relecture impossible"))?;
        // Résolution ENCODÉE : bornée au plafond choisi, JAMAIS d'upscale, figée
        // pour toute la session (recalculer par trame ferait pomper l'image).
        let (enc_w, enc_h) = clamp_dims(w, h, quality.max_w, quality.max_h);
        let scale = Scale::new(w, h, enc_w, enc_h);
        Ok(Capture { pool, session, staging, ctx, w, h, enc_w, enc_h, scale, label, found, closed })
    }

    /// S'il y a une (ou plusieurs) nouvelle(s) trame(s) WGC, copie la plus récente dans
    /// `staging` et la convertit en NV12 dans `nv12`. Renvoie Ok(true) si `nv12` a
    /// changé, Ok(false) si aucune nouvelle trame. Ne renvoie JAMAIS d'Err fatal : un
    /// changement de taille de la source (fenêtre redimensionnée, résolution d'écran
    /// modifiée) est absorbé par letterbox/crop dans le tampon FIXE de l'encodeur
    /// (cap.w×cap.h) — rétréci → bords noirs, agrandi → rogné — sans arrêt ni
    /// reconfiguration NVENC. `fails` compte les échecs de copie CONSÉCUTIFS
    /// (transitoires tolérés, persistants remontés par l'appelant via le seuil de bail).
    unsafe fn grab_latest(cap: &Capture, nv12: &mut [u8], fails: &mut u32) -> anyhow::Result<bool> {
        // Drainer : ne garder que la trame la plus récente (les autres sont refermées).
        let mut latest = None;
        while let Ok(f) = cap.pool.TryGetNextFrame() {
            if let Some(prev) = latest.replace(f) {
                let _ = prev.Close();
            }
        }
        let Some(frame) = latest else { return Ok(false) };
        // Taille COURANTE de la source (une fenêtre partagée change de taille), EN
        // ESPACE SOURCE — bornes dures de lecture de la texture mappée. On la
        // letterbox/crop dans le tampon FIXE de l'encodeur : rétréci → bords noirs,
        // agrandi → rogné. Jamais d'arrêt ni de reconfiguration NVENC.
        let (src_cw, src_ch) = match frame.ContentSize() {
            Ok(s) => (
                ((s.Width as u32) & !1).min(cap.w),
                ((s.Height as u32) & !1).min(cap.h),
            ),
            Err(_) => (cap.w, cap.h),
        };
        // Même conversion EN ESPACE SORTIE (résolution encodée), au même facteur
        // fixe `cap.scale` que la conversion NV12 ci-dessous — sinon la couverture
        // annoncée au scaler ne correspondrait plus à ce qu'il lit réellement.
        let cov_w = cov_out(src_cw, cap.w, cap.enc_w);
        let cov_h = cov_out(src_ch, cap.h, cap.enc_h);
        let ok = (|| -> anyhow::Result<()> {
            let surface = frame.Surface()?;
            let access: IDirect3DDxgiInterfaceAccess = surface.cast()?;
            let tex: ID3D11Texture2D = access.GetInterface()?;
            // CopySubresourceRegion (pas CopyResource) : ne copier que la zone couverte
            // (bornée à la texture source ET au tampon fixe), EN ESPACE SOURCE.
            let src_box = D3D11_BOX {
                left: 0,
                top: 0,
                front: 0,
                right: src_cw.max(2),
                bottom: src_ch.max(2),
                back: 1,
            };
            cap.ctx
                .CopySubresourceRegion(&cap.staging, 0, 0, 0, 0, &tex, 0, Some(&src_box));
            let mut mapped = D3D11_MAPPED_SUBRESOURCE::default();
            cap.ctx.Map(&cap.staging, 0, D3D11_MAP_READ, 0, Some(&mut mapped))?;
            bgra_to_nv12(
                mapped.pData as *const u8,
                mapped.RowPitch as usize,
                cap.enc_w as usize,
                cap.enc_h as usize,
                cov_w as usize,
                cov_h as usize,
                cap.scale,
                src_cw as usize,
                src_ch as usize,
                nv12,
            );
            cap.ctx.Unmap(&cap.staging, 0);
            anyhow::Ok(())
        })()
        .is_ok();
        let _ = frame.Close();
        if ok {
            *fails = 0;
        } else {
            *fails += 1;
        }
        Ok(ok)
    }

    /// Thread principal du partage : capture + conversion + encodage + diffusion.
    /// `ready` reçoit UNE fois le résultat de l'init ((w,h) ou erreur) ; ensuite le
    /// thread vit jusqu'au drapeau `stop` (ou une erreur d'encodage, signalée à l'UI).
    pub(super) fn capture_encode_thread(
        app: AppHandle,
        conns: Vec<(String, Connection)>,
        rt: tokio::runtime::Handle,
        stop: Arc<AtomicBool>,
        ready: std::sync::mpsc::Sender<Result<(u32, u32, String, bool), String>>,
        target: super::ShareTarget,
        quality: super::Quality,
    ) {
        unsafe {
            // Apartment WinRT multithread pour WGC ; toléré s'il est déjà initialisé.
            let _ = RoInitialize(RO_INIT_MULTITHREADED);
            if let Err(e) = MFStartup(MF_VERSION, MFSTARTUP_FULL) {
                let _ = ready.send(Err(format!("Media Foundation indisponible: {e}")));
                return;
            }
            let run = (|| -> anyhow::Result<(Capture, Encoder, ID3D11Device)> {
                let mut device: Option<ID3D11Device> = None;
                D3D11CreateDevice(
                    None,
                    D3D_DRIVER_TYPE_HARDWARE,
                    None,
                    D3D11_CREATE_DEVICE_BGRA_SUPPORT | D3D11_CREATE_DEVICE_VIDEO_SUPPORT,
                    None,
                    D3D11_SDK_VERSION,
                    Some(&mut device),
                    None,
                    None,
                )?;
                let device = device.ok_or_else(|| anyhow::anyhow!("pas de device D3D11"))?;
                // Requis avec le DXGI Device Manager (l'encodeur travaille sur d'autres threads).
                let mt: ID3D10Multithread = device.cast()?;
                let _ = mt.SetMultithreadProtected(true);
                let cap = build_capture(&device, &target, quality)?;
                // L'encodeur est construit aux dimensions ENCODÉES (cap.enc_w/enc_h),
                // PAS aux dimensions de capture natives (cap.w/cap.h) : cap.enc_* est ce
                // que bgra_to_nv12 remplit réellement dans le tampon NV12 (voir grab_latest
                // / encode_loop) — les désynchroniser reproduit le bug historique de
                // trames corrompues (encodeur configuré à une taille, tampon à une autre).
                let enc = build_encoder(&device, cap.enc_w, cap.enc_h, quality.fps.max(1))?;
                Ok((cap, enc, device))
            })();
            let (cap, enc, _device) = match run {
                Ok(v) => v,
                Err(e) => {
                    let _ = ready.send(Err(e.to_string()));
                    let _ = MFShutdown();
                    return;
                }
            };
            // Dimensions ENCODÉES (après clamp_dims) : l'UI affiche la vraie résolution
            // diffusée, pas la résolution native de la source.
            let _ = ready.send(Ok((cap.enc_w, cap.enc_h, cap.label.clone(), cap.found)));

            // Un flux QUIC par pair, alimenté par une file bornée (contre-pression).
            let mut peers: Vec<PeerOut> = conns
                .into_iter()
                .map(|(code, conn)| {
                    let (tx, rx) = tokio::sync::mpsc::channel::<Frame>(PEER_QUEUE);
                    rt.spawn(peer_writer(conn, rx, stop.clone()));
                    PeerOut { code, tx, wait_key: false, dead: false }
                })
                .collect();

            let reason = encode_loop(&app, &cap, &enc, &stop, &mut peers, quality);
            // Fin propre : prévenir l'UI si on s'arrête sur une ERREUR (et non stop()).
            if let Err(e) = reason {
                if !stop.load(Ordering::SeqCst) {
                    let _ = app.emit(
                        "ghost-video-ended",
                        serde_json::json!({ "reason": e.to_string() }),
                    );
                }
            }
            // Teardown DANS L'ORDRE : d'abord signaler la fin de flux et relâcher tous
            // les objets Media Foundation, MFShutdown en DERNIER (relâcher un objet MF
            // après MFShutdown est un comportement indéfini).
            let _ = enc.transform.ProcessMessage(MFT_MESSAGE_NOTIFY_END_OF_STREAM, 0);
            drop(peers); // ferme les files → les peer_writer terminent leurs flux (FIN)
            let _ = cap.session.Close();
            let _ = cap.pool.Close();
            enc.shutdown(); // rend la session NVENC au GPU
            drop(cap);
            drop(_device);
            let _ = MFShutdown();
        }
    }

    /// Boucle NeedInput/HaveOutput avec duplication de trame, cadence ADAPTATIVE
    /// (étape 3 : les débordements des files par pair font descendre l'échelle
    /// `levels_for(quality.fps)` — fps ET débit NVENC à chaud — ; 12 s de calme la
    /// font remonter) et stats émises chaque seconde vers l'UI (ghost-video-stats).
    unsafe fn encode_loop(
        app: &AppHandle,
        cap: &Capture,
        enc: &Encoder,
        stop: &AtomicBool,
        peers: &mut [PeerOut],
        quality: super::Quality,
    ) -> anyhow::Result<()> {
        // ENCODÉES, pas natives : c'est ce que bgra_to_nv12 remplit (voir grab_latest)
        // et ce que build_encoder a configuré — doivent rester en accord (sinon
        // c'est exactement le bug historique de trames corrompues).
        let (w, h) = (cap.enc_w as usize, cap.enc_h as usize);
        let mut nv12 = vec![0u8; w * h * 3 / 2]; // noir NV12 = Y=0/UV=0 acceptable au 1er tick
        for uv in nv12[w * h..].iter_mut() {
            *uv = 128;
        }
        let base_bitrate = bitrate_for_fps(cap.enc_w, cap.enc_h, quality.fps);
        let mut level: usize = 0;
        let mut frame_interval =
            Duration::from_nanos(1_000_000_000 / levels_for(quality.fps)[level].0 as u64);
        // « dyn » = le débit est-il RÉELLEMENT reconfigurable à chaud ? Optimiste tant
        // qu'aucune baisse n'a été tentée ; corrigé au 1er set_bitrate (un encodeur peut
        // exposer ICodecAPI mais rejeter AVEncCommonMeanBitRate en cours de flux). Sinon
        // l'UI annoncerait une baisse de débit qui n'a pas eu lieu (seul le fps a bougé).
        let mut dynamic_ok = enc.codec_api.is_some();
        let mut next_due = Instant::now();
        let t0 = Instant::now();
        let mut last_ts_100ns: i64 = -1;
        let mut out_id: u64 = 0;
        let mut grab_fails: u32 = 0;
        // Fenêtre de stats/contrôle (1 s) : trames et octets encodés, congestion vue.
        let mut win_start = Instant::now();
        let mut win_frames: u32 = 0;
        let mut win_bytes: u64 = 0;
        let mut win_congested = false;
        let mut last_congestion: Option<Instant> = None;
        let mut last_level_change = Instant::now();
        while !stop.load(Ordering::SeqCst) {
            let ev = match enc.gen.GetEvent(MF_EVENT_FLAG_NO_WAIT) {
                Ok(e) => e,
                Err(e) if e.code() == MF_E_NO_EVENTS_AVAILABLE => {
                    std::thread::sleep(Duration::from_millis(1));
                    continue;
                }
                Err(e) => anyhow::bail!("encodeur: {e}"),
            };
            let ty = ev.GetType()? as i32;
            if ty == MEError.0 {
                // Erreur asynchrone (GPU perdu, session tuée…) : sans ce cas, la
                // boucle tournerait à vide et le partage gèlerait en silence.
                let hr = ev.GetStatus().map(|s| s.0).unwrap_or(0);
                anyhow::bail!("erreur de l'encodeur matériel (0x{hr:08x})");
            }
            if ty == METransformNeedInput.0 {
                if cap.closed.load(Ordering::SeqCst) {
                    anyhow::bail!("écran capturé déconnecté (moniteur débranché ?) — relance le partage");
                }
                // Cadence : attendre le prochain tick (l'encodeur va plus vite que nous).
                let now = Instant::now();
                if next_due > now {
                    std::thread::sleep(next_due - now);
                }
                next_due += frame_interval;
                // Si on a pris du retard (machine chargée), repartir d'un pas propre.
                if next_due + frame_interval < Instant::now() {
                    next_due = Instant::now();
                }
                // Nouvelle trame d'écran s'il y en a une, sinon on renvoie la dernière.
                grab_latest(cap, &mut nv12, &mut grab_fails)?;
                // Trois secondes d'échecs de copie consécutifs (device D3D perdu…) :
                // arrêter avec une raison visible plutôt que diffuser une image figée.
                // Seuil sur la cadence COURANTE (adaptée) et non la cible : sinon, à bas
                // niveau, atteindre « fps_cible×3 » échecs prendrait bien plus de 3 s.
                if grab_fails > levels_for(quality.fps)[level].0 * 3 {
                    anyhow::bail!("copie de l'écran en échec répété (GPU/pilote)");
                }
                let mb = MFCreateMemoryBuffer(nv12.len() as u32)?;
                {
                    let mut dst: *mut u8 = std::ptr::null_mut();
                    mb.Lock(&mut dst, None, None)?;
                    std::ptr::copy_nonoverlapping(nv12.as_ptr(), dst, nv12.len());
                    mb.Unlock()?;
                    mb.SetCurrentLength(nv12.len() as u32)?;
                }
                let sample = MFCreateSample()?;
                sample.AddBuffer(&mb)?;
                // Horodatage TEMPS RÉEL (la cadence varie avec le niveau) — strictement
                // croissant, exigence du pilotage du débit par l'encodeur.
                let mut ts = (t0.elapsed().as_nanos() / 100) as i64;
                if ts <= last_ts_100ns {
                    ts = last_ts_100ns + 1;
                }
                last_ts_100ns = ts;
                sample.SetSampleTime(ts)?;
                sample.SetSampleDuration(frame_interval.as_nanos() as i64 / 100)?;
                enc.transform.ProcessInput(0, &sample, 0)?;
            } else if ty == METransformHaveOutput.0 {
                let mut out = [MFT_OUTPUT_DATA_BUFFER {
                    dwStreamID: 0,
                    pSample: ManuallyDrop::new(None),
                    dwStatus: 0,
                    pEvents: ManuallyDrop::new(None),
                }];
                let mut status = 0u32;
                enc.transform.ProcessOutput(0, &mut out, &mut status)?;
                let sample = ManuallyDrop::take(&mut out[0].pSample);
                let _ = ManuallyDrop::take(&mut out[0].pEvents);
                let Some(sample) = sample else { continue };
                let (key, data) = read_sample(&sample)?;
                out_id += 1;
                win_frames += 1;
                win_bytes += data.len() as u64;
                let outcome = dispatch(peers, &Frame { id: out_id, key, data: bytes::Bytes::from(data) });
                win_congested |= outcome.congested;
                for code in outcome.newly_dead {
                    // L'UI de l'émetteur doit savoir qu'un pair ne reçoit plus rien.
                    let _ = app.emit("ghost-video-peer-dead", &code);
                }
                // Plus AUCUN destinataire vivant : inutile de continuer capture +
                // conversion NV12 + NVENC à plein régime pour personne (les pairs sont
                // figés au démarrage, aucun ne peut réapparaître). On sort — le chemin
                // ghost-video-ended de l'appelant en informe l'UI.
                if !peers.is_empty() && peers.iter().all(|p| p.dead) {
                    anyhow::bail!("plus aucun destinataire du partage");
                }
            }

            // ---- Tick 1 s : contrôleur adaptatif + stats vers l'UI ----
            if win_start.elapsed() < Duration::from_secs(1) {
                continue;
            }
            // Un pair encore en attente d'image clé compte comme congestion : ses
            // débordements ne se re-manifestent qu'aux tentatives de keyframe.
            let waiting = peers.iter().any(|p| !p.dead && p.wait_key);
            let congested = win_congested || waiting;
            if congested {
                last_congestion = Some(Instant::now());
            }
            let since_change = last_level_change.elapsed();
            let calm = last_congestion.map(|t| t.elapsed() >= Duration::from_secs(12)).unwrap_or(true);
            let levels = levels_for(quality.fps);
            let new_level = if congested && level + 1 < levels.len() && since_change >= Duration::from_secs(2) {
                level + 1
            } else if !congested && calm && level > 0 && since_change >= Duration::from_secs(12) {
                level - 1
            } else {
                level
            };
            if new_level != level {
                level = new_level;
                last_level_change = Instant::now();
                let (fps_l, pct) = levels[level];
                frame_interval = Duration::from_nanos(1_000_000_000 / fps_l as u64);
                // Débit + GOP à chaud (best-effort : sans reconfiguration réelle, seul
                // le fps bouge — ça soulage l'encodeur mais pas le réseau, le saut de
                // trames fait alors le reste, comme avant l'étape 3). Le retour RÉEL du
                // 1er set_bitrate corrige « dyn » pour que l'UI ne mente pas.
                if enc.codec_api.is_some() {
                    dynamic_ok = enc.set_bitrate(base_bitrate / 100 * pct);
                }
                let _ = enc.set_gop(fps_l * KEYFRAME_SECS);
            }
            let alive = peers.iter().filter(|p| !p.dead).count();
            let ok = peers.iter().filter(|p| !p.dead && !p.wait_key).count();
            let _ = app.emit(
                "ghost-video-stats",
                serde_json::json!({
                    "fps": win_frames,
                    "kbps": win_bytes * 8 / 1000,
                    "peers": alive,
                    "peersOk": ok,
                    "level": level,
                    "pct": levels_for(quality.fps)[level].1,
                    "dyn": dynamic_ok,
                    "w": cap.enc_w,
                    "h": cap.enc_h,
                }),
            );
            win_start = Instant::now();
            win_frames = 0;
            win_bytes = 0;
            win_congested = false;
        }
        Ok(())
    }

    #[cfg(test)]
    mod tests {
        use super::*;

        /// Échelle identité W×H → W×H, pour les tests de non-régression du chemin
        /// rapide (aucun scaler, code = ancienne version 1:1).
        fn identity(w: u32, h: u32) -> Scale {
            Scale::new(w, h, w, h)
        }

        /// Balayage de robustesse : aucune combinaison réaliste (écran/fenêtre, cible,
        /// couverture partielle d'une fenêtre redimensionnée) ne doit paniquer ni lire
        /// hors de la source. Un panic ici tuerait le thread de capture — le partage
        /// s'arrêterait alors que l'audio, sur un autre thread, continuerait.
        #[test]
        fn scaler_aucune_combinaison_ne_panique() {
            const SRC: &[(usize, usize)] =
                &[(2560, 1440), (1920, 1080), (3840, 2160), (1366, 768), (1280, 720), (800, 600)];
            const CIBLE: &[(u32, u32)] = &[(0, 0), (1920, 1080), (1280, 720)];
            for &(sw, sh) in SRC {
                // Pitch volontairement plus large que la ligne (cas réel des textures D3D).
                let pitch = sw * 4 + 64;
                let src = vec![77u8; pitch * sh];
                for &(mw, mh) in CIBLE {
                    let (ew, eh) = super::clamp_dims(sw as u32, sh as u32, mw, mh);
                    let (ew, eh) = (ew as usize, eh as usize);
                    let sc = Scale::new(sw as u32, sh as u32, ew as u32, eh as u32);
                    let mut out = vec![0u8; ew * eh * 3 / 2];
                    // Couvertures : pleine, moitié, quart, une seule ligne/colonne, nulle.
                    for &(num, den) in &[(1usize, 1usize), (1, 2), (1, 4), (1, 100), (0, 1)] {
                        let scw = (sw * num / den) & !1;
                        let sch = (sh * num / den) & !1;
                        let cw = super::cov_out(scw as u32, sw as u32, ew as u32) as usize;
                        let ch = super::cov_out(sch as u32, sh as u32, eh as u32) as usize;
                        bgra_to_nv12(src.as_ptr(), pitch, ew, eh, cw, ch, sc, scw, sch, &mut out);
                    }
                }
            }
        }

        /// Banc d'essai du coût CPU de la conversion, sur le cas qui a posé problème en
        /// réel : un écran 2K partagé en 1080p oscillait à 40-60 fps alors que le même
        /// écran en NATIF (chemin identité) tenait 60 fps stable.
        /// `grab_latest` étant appelé SYNCHRONEMENT depuis la boucle d'encodage, ce temps
        /// plafonne directement la cadence : il doit rester bien sous 16,6 ms (60 fps).
        /// À lancer EN RELEASE, sinon la mesure n'a aucun sens :
        ///   cargo test --release -- --ignored perf_conversion --nocapture
        #[test]
        #[ignore = "mesure de perf (lancer en --release)"]
        fn perf_conversion() {
            const SW: usize = 2560;
            const SH: usize = 1440;
            let src = vec![90u8; SW * SH * 4];
            let bench = |label: &str, dw: usize, dh: usize| {
                let sc = Scale::new(SW as u32, SH as u32, dw as u32, dh as u32);
                let mut out = vec![0u8; dw * dh * 3 / 2];
                let run = |out: &mut [u8]| {
                    bgra_to_nv12(src.as_ptr(), SW * 4, dw, dh, dw, dh, sc, SW, SH, out)
                };
                run(&mut out); // chauffe (caches, pages)
                const N: u32 = 20;
                let t = std::time::Instant::now();
                for _ in 0..N {
                    run(&mut out);
                }
                let per = t.elapsed() / N;
                println!(
                    "{label:<34} {:>7.2} ms/trame  ({:>5.1} fps max)",
                    per.as_secs_f64() * 1000.0,
                    1.0 / per.as_secs_f64()
                );
                per
            };
            let natif = bench("2560x1440 NATIF (identité)", SW, SH);
            let p1080 = bench("2560x1440 -> 1920x1080", 1920, 1080);
            let p720 = bench("2560x1440 -> 1280x720", 1280, 720);
            println!(
                "ratio downscale/natif : 1080p x{:.2}, 720p x{:.2}",
                p1080.as_secs_f64() / natif.as_secs_f64(),
                p720.as_secs_f64() / natif.as_secs_f64()
            );
        }

        /// Convertit un buffer BGRA uniforme et vérifie Y/U/V (BT.709 limité).
        fn convert_uniform(b: u8, g: u8, r: u8) -> (u8, u8, u8) {
            const W: usize = 8;
            const H: usize = 8;
            let mut src = vec![0u8; W * H * 4];
            for px in src.chunks_exact_mut(4) {
                px[0] = b;
                px[1] = g;
                px[2] = r;
                px[3] = 255;
            }
            let mut out = vec![0u8; W * H * 3 / 2];
            bgra_to_nv12(src.as_ptr(), W * 4, W, H, W, H, identity(W as u32, H as u32), W, H, &mut out);
            (out[0], out[W * H], out[W * H + 1])
        }

        #[test]
        fn nv12_blanc_noir_rouge() {
            // Blanc → Y≈235, chroma neutre ; noir → Y=16 ; rouge → V nettement > 128.
            let (y, u, v) = convert_uniform(255, 255, 255);
            assert!((233..=237).contains(&y), "Y blanc = {y}");
            assert!((126..=130).contains(&u) && (126..=130).contains(&v));
            let (y, u, v) = convert_uniform(0, 0, 0);
            assert!((15..=17).contains(&y), "Y noir = {y}");
            assert!((126..=130).contains(&u) && (126..=130).contains(&v));
            let (y, _u, v) = convert_uniform(0, 0, 255);
            assert!((55..=70).contains(&y), "Y rouge = {y}");
            assert!(v > 200, "V rouge = {v}");
        }

        #[test]
        fn nv12_pitch_plus_large_que_la_ligne() {
            // Le pitch D3D dépasse souvent w*4 : les octets de bourrage ne doivent
            // pas contaminer la conversion.
            const W: usize = 4;
            const H: usize = 4;
            const PITCH: usize = W * 4 + 12;
            let mut src = vec![0xEEu8; PITCH * H];
            for y in 0..H {
                for x in 0..W {
                    let o = y * PITCH + x * 4;
                    src[o] = 255; // bleu... en fait blanc :
                    src[o + 1] = 255;
                    src[o + 2] = 255;
                    src[o + 3] = 255;
                }
            }
            let mut out = vec![0u8; W * H * 3 / 2];
            bgra_to_nv12(src.as_ptr(), PITCH, W, H, W, H, identity(W as u32, H as u32), W, H, &mut out);
            for &y in &out[..W * H] {
                assert!((233..=237).contains(&y), "Y = {y}");
            }
        }

        #[test]
        fn nv12_letterbox_borde_de_noir() {
            // Fenêtre 4×4 rétrécie couvrant seulement 2×2 d'un tampon 4×4 : le coin
            // haut-gauche = blanc, le reste = noir (Y=16, chroma neutre).
            const W: usize = 4;
            const H: usize = 4;
            let mut src = vec![255u8; W * H * 4]; // tout blanc opaque
            let mut out = vec![0u8; W * H * 3 / 2];
            bgra_to_nv12(src.as_mut_ptr(), W * 4, W, H, 2, 2, identity(W as u32, H as u32), 2, 2, &mut out);
            // Y : (0,0) et (1,1) couverts (~235), (2,*) et (*,2) noirs (16).
            assert!((233..=237).contains(&out[0]), "Y couvert = {}", out[0]);
            assert_eq!(out[2], 16, "Y bord droit doit être noir");
            assert_eq!(out[2 * W], 16, "Y bord bas doit être noir");
            // Chroma du bloc couvert (0,0) neutre-ish ; bloc bord = 128/128.
            assert_eq!(out[W * H + 2], 128);
            assert_eq!(out[W * H + 3], 128);
        }

        #[test]
        fn nv12_scale_downscale_8x8_vers_4x4_blanc() {
            // (a) tout blanc, downscale 2× dans les deux axes : tous les Y ≈ 235
            // (la moyenne d'aire d'un bloc uniforme reste la même valeur).
            const SW: usize = 8;
            const SH: usize = 8;
            let src = vec![255u8; SW * SH * 4];
            let sc = Scale::new(SW as u32, SH as u32, 4, 4);
            let mut out = vec![0u8; 4 * 4 * 3 / 2];
            bgra_to_nv12(src.as_ptr(), SW * 4, 4, 4, 4, 4, sc, SW, SH, &mut out);
            for &y in &out[..4 * 4] {
                assert!((233..=237).contains(&y), "Y = {y}");
            }
        }

        #[test]
        fn nv12_scale_frontiere_moyenne() {
            // (b) 3 colonnes blanches puis 5 noires sur une source 8 large, downscale
            // 2× : la boîte de la colonne de sortie ox=1 couvre les colonnes source
            // 2 (blanche) et 3 (noire) → doit tomber au milieu de la plage Y valide,
            // pas à une extrémité comme le nearest-neighbor le ferait.
            const SW: usize = 8;
            const SH: usize = 8;
            let mut src = vec![0u8; SW * SH * 4];
            for y in 0..SH {
                for x in 0..SW {
                    let o = (y * SW + x) * 4;
                    let v = if x < 3 { 255 } else { 0 };
                    src[o] = v;
                    src[o + 1] = v;
                    src[o + 2] = v;
                    src[o + 3] = 255;
                }
            }
            let sc = Scale::new(SW as u32, SH as u32, 4, 4);
            let mut out = vec![0u8; 4 * 4 * 3 / 2];
            bgra_to_nv12(src.as_ptr(), SW * 4, 4, 4, 4, 4, sc, SW, SH, &mut out);
            let y_full_white = out[0];
            let y_boundary = out[1];
            let y_full_black = out[3];
            assert!((233..=237).contains(&y_full_white), "Y blanc = {y_full_white}");
            assert!((15..=17).contains(&y_full_black), "Y noir = {y_full_black}");
            assert!(
                (60..=180).contains(&y_boundary),
                "Y frontière = {y_boundary} (ni blanc ni noir pur : moyenne d'aire attendue)"
            );
        }

        #[test]
        fn nv12_scale_letterbox_a_l_echelle() {
            // (c) Source native 8×8, mais la fenêtre ne couvre que sa moitié gauche
            // (4×8 en espace SOURCE). Downscale 2× vers un tampon encodeur 4×4 : en
            // espace SORTIE, la couverture devient 2×4 (moitié gauche du tampon), le
            // reste doit rester letterbox noir — exactement la formule de conversion
            // de couverture source→sortie utilisée par grab_latest.
            const SW: usize = 8;
            const SH: usize = 8;
            let src = vec![255u8; SW * SH * 4]; // tout blanc (seule la zone couverte compte)
            let sc = Scale::new(SW as u32, SH as u32, 4, 4);
            let (src_cov_w, src_cov_h) = (4usize, 8usize);
            // On appelle la fonction de PRODUCTION (celle qu'utilise grab_latest) au lieu
            // de recopier sa formule : une recopie s'auto-validerait et laisserait passer
            // une inversion source/sortie — le bug qui avait fait retirer la downscale.
            let cov_w = super::cov_out(src_cov_w as u32, SW as u32, 4) as usize;
            let cov_h = super::cov_out(src_cov_h as u32, SH as u32, 4) as usize;
            assert_eq!((cov_w, cov_h), (2, 4), "couverture sortie attendue = moitié gauche");
            // Le sens de la conversion doit être source→sortie : une fenêtre rétrécie
            // couvre MOINS que le tampon (inverser les termes donnerait 8, clampé à 4).
            assert_eq!(super::cov_out(960, 3840, 1920), 480, "4K→1080p, fenêtre au quart");
            let mut out = vec![0u8; 4 * 4 * 3 / 2];
            bgra_to_nv12(src.as_ptr(), SW * 4, 4, 4, cov_w, cov_h, sc, src_cov_w, src_cov_h, &mut out);
            for oy in 0..4 {
                assert!((233..=237).contains(&out[oy * 4]), "colonne 0 (couverte) doit être blanche");
                assert!((233..=237).contains(&out[oy * 4 + 1]), "colonne 1 (couverte) doit être blanche");
                assert_eq!(out[oy * 4 + 2], 16, "colonne 2 (hors couverture) doit être noire");
                assert_eq!(out[oy * 4 + 3], 16, "colonne 3 (hors couverture) doit être noire");
            }
            // Chroma du bloc bordure (colonnes 2-3, lignes 0-1) = neutre.
            assert_eq!(out[4 * 4 + 2], 128);
            assert_eq!(out[4 * 4 + 3], 128);
        }

        #[test]
        fn nv12_scale_ratio_non_entier_ne_panique_pas() {
            // (d) 6→4 : ratio non entier (step_x/step_y = 1.5 en Q16.16). Ne doit
            // jamais paniquer ni lire hors de la zone source déclarée (src_cov=6×6) ;
            // source uniforme gris moyen → sortie uniforme (sanity, pas de garbage).
            const SW: usize = 6;
            const SH: usize = 6;
            let src = vec![128u8; SW * SH * 4];
            let sc = Scale::new(SW as u32, SH as u32, 4, 4);
            let mut out = vec![0u8; 4 * 4 * 3 / 2];
            bgra_to_nv12(src.as_ptr(), SW * 4, 4, 4, 4, 4, sc, SW, SH, &mut out);
            for &y in &out[..4 * 4] {
                assert!((120..=132).contains(&y), "Y gris moyen = {y}");
            }
        }

        #[test]
        fn detection_keyframe_annexb() {
            // SPS (NALU 7) après start code → keyframe ; slice non-IDR (1) → non.
            assert!(looks_like_keyframe(&[0, 0, 0, 1, 0x67, 0x4d, 0x00]));
            assert!(looks_like_keyframe(&[0x09, 0x10, 0, 0, 0, 1, 0x65]));
            assert!(!looks_like_keyframe(&[0, 0, 0, 1, 0x41, 0x9a]));
            assert!(!looks_like_keyframe(&[0, 0, 0]));
        }

        #[test]
        fn enumeration_fenetres_ne_panique_pas() {
            // Ne doit jamais paniquer ; chaque entrée a un id/pid non vides.
            for w in list_windows() {
                assert!(!w["id"].as_str().unwrap_or("").is_empty());
                assert!(w["pid"].as_u64().unwrap_or(0) > 0, "pid manquant: {w:?}");
            }
        }

        /// Smoke matériel de la capture de FENÊTRE (CreateForWindow + letterbox) : prend
        /// la 1re fenêtre partageable et encode quelques trames. `cargo test -- --ignored`.
        #[test]
        #[ignore = "matériel : GPU + une fenêtre visible requis"]
        fn smoke_window_capture() {
            let wins = list_windows();
            let Some(first) = wins.first() else {
                eprintln!("aucune fenêtre — test ignoré de fait");
                return;
            };
            let hwnd: isize = first["id"].as_str().unwrap().parse().unwrap();
            unsafe {
                let _ = RoInitialize(RO_INIT_MULTITHREADED);
                MFStartup(MF_VERSION, MFSTARTUP_FULL).unwrap();
                let mut device: Option<ID3D11Device> = None;
                D3D11CreateDevice(
                    None,
                    D3D_DRIVER_TYPE_HARDWARE,
                    None,
                    D3D11_CREATE_DEVICE_BGRA_SUPPORT | D3D11_CREATE_DEVICE_VIDEO_SUPPORT,
                    None,
                    D3D11_SDK_VERSION,
                    Some(&mut device),
                    None,
                    None,
                )
                .unwrap();
                let device = device.unwrap();
                let mt: ID3D10Multithread = device.cast().unwrap();
                let _ = mt.SetMultithreadProtected(true);
                let cap = build_capture(
                    &device,
                    &crate::video::ShareTarget::Window(hwnd),
                    crate::video::Quality::default(),
                )
                .expect("capture de fenêtre");
                let enc = build_encoder(&device, cap.enc_w, cap.enc_h, FPS_DEFAULT).expect("encodeur");
                let (w, h) = (cap.enc_w as usize, cap.enc_h as usize);
                let mut nv12 = vec![0u8; w * h * 3 / 2];
                let frame_dur: i64 = 10_000_000 / FPS_DEFAULT as i64;
                let mut fed = 0u64;
                let mut got = 0u64;
                let deadline = Instant::now() + Duration::from_secs(10);
                while got < 8 && Instant::now() < deadline {
                    let ev = match enc.gen.GetEvent(MF_EVENT_FLAG_NO_WAIT) {
                        Ok(e) => e,
                        Err(e) if e.code() == MF_E_NO_EVENTS_AVAILABLE => {
                            std::thread::sleep(Duration::from_millis(1));
                            continue;
                        }
                        Err(e) => panic!("GetEvent: {e}"),
                    };
                    let ty = ev.GetType().unwrap() as i32;
                    if ty == METransformNeedInput.0 {
                        let mut fails = 0u32;
                        let _ = grab_latest(&cap, &mut nv12, &mut fails);
                        let mb = MFCreateMemoryBuffer(nv12.len() as u32).unwrap();
                        let mut dst: *mut u8 = std::ptr::null_mut();
                        mb.Lock(&mut dst, None, None).unwrap();
                        std::ptr::copy_nonoverlapping(nv12.as_ptr(), dst, nv12.len());
                        mb.Unlock().unwrap();
                        mb.SetCurrentLength(nv12.len() as u32).unwrap();
                        let sample = MFCreateSample().unwrap();
                        sample.AddBuffer(&mb).unwrap();
                        sample.SetSampleTime(fed as i64 * frame_dur).unwrap();
                        sample.SetSampleDuration(frame_dur).unwrap();
                        enc.transform.ProcessInput(0, &sample, 0).unwrap();
                        fed += 1;
                    } else if ty == METransformHaveOutput.0 {
                        let mut out = [MFT_OUTPUT_DATA_BUFFER {
                            dwStreamID: 0,
                            pSample: ManuallyDrop::new(None),
                            dwStatus: 0,
                            pEvents: ManuallyDrop::new(None),
                        }];
                        let mut status = 0u32;
                        enc.transform.ProcessOutput(0, &mut out, &mut status).unwrap();
                        let sample = ManuallyDrop::take(&mut out[0].pSample);
                        let _ = ManuallyDrop::take(&mut out[0].pEvents);
                        if let Some(sample) = sample {
                            let (_key, data) = read_sample(&sample).unwrap();
                            assert!(
                                data.starts_with(&[0, 0, 0, 1]) || data.starts_with(&[0, 0, 1]),
                                "sortie sans start code Annex-B"
                            );
                            got += 1;
                        }
                    }
                }
                let _ = cap.session.Close();
                let _ = cap.pool.Close();
                enc.shutdown();
                drop(cap);
                drop(device);
                let _ = MFShutdown();
                assert!(got >= 8, "seulement {got} trames encodées depuis « {} »", first["name"]);
                println!("✅ smoke fenêtre {}x{} « {} » : {got} trames H.264", w, h, first["name"]);
            }
        }

        #[test]
        fn enumeration_moniteurs() {
            // Ne doit jamais paniquer ; sur une session avec affichage, il y a au
            // moins un moniteur et exactement un « principal ».
            let mons = list_monitors();
            if !mons.is_empty() {
                let primaries = mons.iter().filter(|m| m["primary"] == true).count();
                assert_eq!(primaries, 1, "un et un seul écran principal: {mons:?}");
                assert!(mons[0]["w"].as_i64().unwrap_or(0) > 0);
            }
        }

        /// Smoke test MATÉRIEL de la chaîne complète capture→NV12→NVENC (nécessite
        /// un GPU avec encodeur H.264 et un écran) : `cargo test -- --ignored`.
        #[test]
        #[ignore = "matériel : GPU + écran requis"]
        fn smoke_capture_encode() {
            unsafe {
                let _ = RoInitialize(RO_INIT_MULTITHREADED);
                MFStartup(MF_VERSION, MFSTARTUP_FULL).unwrap();
                let mut device: Option<ID3D11Device> = None;
                D3D11CreateDevice(
                    None,
                    D3D_DRIVER_TYPE_HARDWARE,
                    None,
                    D3D11_CREATE_DEVICE_BGRA_SUPPORT | D3D11_CREATE_DEVICE_VIDEO_SUPPORT,
                    None,
                    D3D11_SDK_VERSION,
                    Some(&mut device),
                    None,
                    None,
                )
                .unwrap();
                let device = device.unwrap();
                let mt: ID3D10Multithread = device.cast().unwrap();
                let _ = mt.SetMultithreadProtected(true);
                let cap = build_capture(
                    &device,
                    &crate::video::ShareTarget::Monitor(None),
                    crate::video::Quality::default(),
                )
                .expect("capture WGC");
                let enc = build_encoder(&device, cap.enc_w, cap.enc_h, FPS_DEFAULT).expect("encodeur matériel");
                let (w, h) = (cap.enc_w as usize, cap.enc_h as usize);
                let mut nv12 = vec![0u8; w * h * 3 / 2];
                let frame_dur: i64 = 10_000_000 / FPS_DEFAULT as i64;
                let mut fed: u64 = 0;
                let mut outs: Vec<(bool, usize)> = Vec::new();
                // Étape 3 : valider le débit À CHAUD — 60 trames à débit de base,
                // puis reconfiguration à 1 Mb/s et 60 trames de plus.
                let total_wanted = 120usize;
                let mut lowered = false;
                let mut dyn_ok = false;
                let deadline = Instant::now() + Duration::from_secs(20);
                while outs.len() < total_wanted && Instant::now() < deadline {
                    if outs.len() >= 60 && !lowered {
                        lowered = true;
                        dyn_ok = enc.set_bitrate(1_000_000);
                        let _ = enc.set_gop(FPS_DEFAULT * KEYFRAME_SECS);
                    }
                    let ev = match enc.gen.GetEvent(MF_EVENT_FLAG_NO_WAIT) {
                        Ok(e) => e,
                        Err(e) if e.code() == MF_E_NO_EVENTS_AVAILABLE => {
                            std::thread::sleep(Duration::from_millis(1));
                            continue;
                        }
                        Err(e) => panic!("GetEvent: {e}"),
                    };
                    let ty = ev.GetType().unwrap() as i32;
                    if ty == METransformNeedInput.0 {
                        let mut fails = 0u32;
                        let _ = grab_latest(&cap, &mut nv12, &mut fails);
                        let mb = MFCreateMemoryBuffer(nv12.len() as u32).unwrap();
                        let mut dst: *mut u8 = std::ptr::null_mut();
                        mb.Lock(&mut dst, None, None).unwrap();
                        std::ptr::copy_nonoverlapping(nv12.as_ptr(), dst, nv12.len());
                        mb.Unlock().unwrap();
                        mb.SetCurrentLength(nv12.len() as u32).unwrap();
                        let sample = MFCreateSample().unwrap();
                        sample.AddBuffer(&mb).unwrap();
                        sample.SetSampleTime(fed as i64 * frame_dur).unwrap();
                        sample.SetSampleDuration(frame_dur).unwrap();
                        enc.transform.ProcessInput(0, &sample, 0).unwrap();
                        fed += 1;
                    } else if ty == METransformHaveOutput.0 {
                        let mut out = [MFT_OUTPUT_DATA_BUFFER {
                            dwStreamID: 0,
                            pSample: ManuallyDrop::new(None),
                            dwStatus: 0,
                            pEvents: ManuallyDrop::new(None),
                        }];
                        let mut status = 0u32;
                        enc.transform.ProcessOutput(0, &mut out, &mut status).unwrap();
                        let sample = ManuallyDrop::take(&mut out[0].pSample);
                        let _ = ManuallyDrop::take(&mut out[0].pEvents);
                        if let Some(sample) = sample {
                            let (key, data) = read_sample(&sample).unwrap();
                            assert!(
                                data.starts_with(&[0, 0, 0, 1]) || data.starts_with(&[0, 0, 1]),
                                "sortie sans start code Annex-B"
                            );
                            outs.push((key, data.len()));
                        }
                    }
                }
                let _ = cap.session.Close();
                let _ = cap.pool.Close();
                enc.shutdown();
                drop(cap);
                drop(device);
                let _ = MFShutdown();
                assert!(outs.len() >= 60, "seulement {} images encodées ({} nourries)", outs.len(), fed);
                assert!(outs[0].0, "la première image encodée doit être une keyframe");
                // Vérification du levier adaptatif : débit AVANT vs APRÈS la baisse
                // (moyennes hors keyframes pour ne pas biaiser). Informatif d'abord,
                // mais si SetValue a dit OK, la baisse doit être réelle.
                let avg = |s: &[(bool, usize)]| {
                    let d: Vec<usize> = s.iter().filter(|(k, _)| !k).map(|(_, l)| *l).collect();
                    if d.is_empty() { 0 } else { d.iter().sum::<usize>() / d.len() }
                };
                let before = avg(&outs[10..60.min(outs.len())]);
                let after = if outs.len() > 80 { avg(&outs[80..]) } else { 0 };
                println!(
                    "✅ smoke {}x{} : {} images, 1re = clé | débit à chaud: SetValue={} | delta moyen {}o → {}o",
                    w, h, outs.len(),
                    if dyn_ok { "OK" } else { "REFUSÉ" },
                    before, after
                );
                if dyn_ok && after > 0 {
                    assert!(
                        after < before,
                        "SetValue(bitrate) accepté mais sans effet mesurable ({before}o → {after}o)"
                    );
                }
            }
        }

        /// Smoke test MATÉRIEL du chemin AVEC mise à l'échelle (720p) : c'est le
        /// SEUL test qui aurait attrapé le bug historique (encodeur construit aux
        /// dimensions cibles, tampon NV12 rempli aux dimensions de capture natives
        /// → trames corrompues). Vérifie que l'encodeur produit bien un flux Annex-B
        /// valide (start code) une fois le scaler dans la boucle. `cargo test -- --ignored`.
        #[test]
        #[ignore = "matériel : GPU + écran requis"]
        fn smoke_capture_encode_scaled_720p() {
            unsafe {
                let _ = RoInitialize(RO_INIT_MULTITHREADED);
                MFStartup(MF_VERSION, MFSTARTUP_FULL).unwrap();
                let mut device: Option<ID3D11Device> = None;
                D3D11CreateDevice(
                    None,
                    D3D_DRIVER_TYPE_HARDWARE,
                    None,
                    D3D11_CREATE_DEVICE_BGRA_SUPPORT | D3D11_CREATE_DEVICE_VIDEO_SUPPORT,
                    None,
                    D3D11_SDK_VERSION,
                    Some(&mut device),
                    None,
                    None,
                )
                .unwrap();
                let device = device.unwrap();
                let mt: ID3D10Multithread = device.cast().unwrap();
                let _ = mt.SetMultithreadProtected(true);
                let quality = crate::video::Quality { fps: 30, max_w: 1280, max_h: 720 };
                let cap = build_capture(&device, &crate::video::ShareTarget::Monitor(None), quality)
                    .expect("capture WGC");
                let enc = build_encoder(&device, cap.enc_w, cap.enc_h, quality.fps).expect("encodeur matériel");
                assert!(
                    cap.enc_w <= 1280 && cap.enc_h <= 720,
                    "downscale non appliqué : enc={}x{} (natif {}x{})",
                    cap.enc_w, cap.enc_h, cap.w, cap.h
                );
                let (w, h) = (cap.enc_w as usize, cap.enc_h as usize);
                let mut nv12 = vec![0u8; w * h * 3 / 2];
                let frame_dur: i64 = 10_000_000 / quality.fps as i64;
                let mut fed: u64 = 0;
                let mut got: usize = 0;
                let deadline = Instant::now() + Duration::from_secs(15);
                while got < 6 && Instant::now() < deadline {
                    let ev = match enc.gen.GetEvent(MF_EVENT_FLAG_NO_WAIT) {
                        Ok(e) => e,
                        Err(e) if e.code() == MF_E_NO_EVENTS_AVAILABLE => {
                            std::thread::sleep(Duration::from_millis(1));
                            continue;
                        }
                        Err(e) => panic!("GetEvent: {e}"),
                    };
                    let ty = ev.GetType().unwrap() as i32;
                    if ty == METransformNeedInput.0 {
                        let mut fails = 0u32;
                        let _ = grab_latest(&cap, &mut nv12, &mut fails);
                        let mb = MFCreateMemoryBuffer(nv12.len() as u32).unwrap();
                        let mut dst: *mut u8 = std::ptr::null_mut();
                        mb.Lock(&mut dst, None, None).unwrap();
                        std::ptr::copy_nonoverlapping(nv12.as_ptr(), dst, nv12.len());
                        mb.Unlock().unwrap();
                        mb.SetCurrentLength(nv12.len() as u32).unwrap();
                        let sample = MFCreateSample().unwrap();
                        sample.AddBuffer(&mb).unwrap();
                        sample.SetSampleTime(fed as i64 * frame_dur).unwrap();
                        sample.SetSampleDuration(frame_dur).unwrap();
                        enc.transform.ProcessInput(0, &sample, 0).unwrap();
                        fed += 1;
                    } else if ty == METransformHaveOutput.0 {
                        let mut out = [MFT_OUTPUT_DATA_BUFFER {
                            dwStreamID: 0,
                            pSample: ManuallyDrop::new(None),
                            dwStatus: 0,
                            pEvents: ManuallyDrop::new(None),
                        }];
                        let mut status = 0u32;
                        enc.transform.ProcessOutput(0, &mut out, &mut status).unwrap();
                        let sample = ManuallyDrop::take(&mut out[0].pSample);
                        let _ = ManuallyDrop::take(&mut out[0].pEvents);
                        if let Some(sample) = sample {
                            let (_key, data) = read_sample(&sample).unwrap();
                            // C'est LA vérification qui aurait attrapé le bug historique :
                            // un tampon NV12 mal dimensionné produit un flux H.264 corrompu
                            // (l'encodeur n'émet même pas un Annex-B valide, ou plante).
                            assert!(
                                data.starts_with(&[0, 0, 0, 1]) || data.starts_with(&[0, 0, 1]),
                                "sortie sans start code Annex-B — tampon NV12/encodeur désynchronisés ?"
                            );
                            got += 1;
                        }
                    }
                }
                let (native_w, native_h) = (cap.w, cap.h);
                let _ = cap.session.Close();
                let _ = cap.pool.Close();
                enc.shutdown();
                drop(cap);
                drop(device);
                let _ = MFShutdown();
                assert!(got >= 6, "seulement {got} trames encodées en 720p ({fed} nourries)");
                println!(
                    "✅ smoke downscale {}x{} (natif {}x{}) : {got} trames H.264 valides",
                    w, h, native_w, native_h
                );
            }
        }
    }

    /// Extrait (keyframe?, octets H.264 Annex-B) d'un échantillon encodé.
    unsafe fn read_sample(sample: &IMFSample) -> anyhow::Result<(bool, Vec<u8>)> {
        // ConvertToContiguousBuffer (et non GetBufferByIndex(0)) : un échantillon
        // peut porter PLUSIEURS buffers — n'en lire qu'un tronquerait la trame.
        let buf = sample.ConvertToContiguousBuffer()?;
        let mut p: *mut u8 = std::ptr::null_mut();
        let mut len = 0u32;
        buf.Lock(&mut p, None, Some(&mut len))?;
        let data = std::slice::from_raw_parts(p, len as usize).to_vec();
        buf.Unlock()?;
        let key = match sample.GetUINT32(&MFSampleExtension_CleanPoint) {
            Ok(v) => v == 1,
            Err(_) => looks_like_keyframe(&data),
        };
        Ok((key, data))
    }
}
