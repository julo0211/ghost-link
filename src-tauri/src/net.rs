// Cœur réseau P2P de ghost link — iroh (QUIC, hole-punching + relais chiffré).
// Modèle « session » : on se connecte une fois, puis on s'envoie autant de fichiers
// qu'on veut (dans les deux sens). Avec débit, annulation et déconnexion propagée.

use std::collections::{HashMap, HashSet};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::{Arc, Mutex as StdMutex};

use iroh::{
    endpoint::{presets, Connection},
    protocol::{AcceptError, ProtocolHandler, Router},
    Endpoint, EndpointAddr, EndpointId, SecretKey,
};
use tauri::{AppHandle, Emitter};
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::sync::Mutex;

pub const ALPN: &[u8] = b"ghost-link/file/0";
// Protocole léger de présence : si la connexion s'établit, le pair est en ligne.
pub const PRESENCE_ALPN: &[u8] = b"ghost-link/presence/0";
const CHUNK: usize = 256 * 1024;
// Premier octet de chaque flux bi-directionnel : type de message.
const KIND_FILE: u8 = 1;
const KIND_CHAT: u8 = 2;
const KIND_FREQ: u8 = 3; // demande d'ami
const KIND_FACCEPT: u8 = 4; // acceptation d'ami
const KIND_CALL_START: u8 = 5; // début d'appel vocal
const KIND_CALL_STOP: u8 = 6; // fin d'appel vocal
const KIND_HELLO: u8 = 7; // poignée de main applicative : l'initiateur n'est « connecté » qu'après l'ack du pair
const KIND_FDATA: u8 = 8; // flux de données d'un transfert (multi-flux parallèles)
const KIND_IMG: u8 = 9; // image inline 1-à-1 (octets)
const NSTREAMS: u64 = 4; // nombre de flux parallèles par fichier
static FILE_SEQ: AtomicU64 = AtomicU64::new(1); // identifiants de transfert
static STREAMS: AtomicU64 = AtomicU64::new(NSTREAMS); // nb de flux parallèles, réglable à chaud (1..=8)

/// Règle le nombre de flux parallèles par transfert (borné 1..=8).
pub fn set_streams(n: u64) {
    STREAMS.store(n.clamp(1, 8), Ordering::SeqCst);
}

// --- Groupes (maillage, ALPN séparé du 1-à-1) ---
pub const GROUP_ALPN: &[u8] = b"ghost-link/group/0";
const GKIND_CHAT: u8 = 1; // message de channel de groupe
const GKIND_INVITE: u8 = 2; // invitation à un groupe
const GKIND_CALL: u8 = 3; // signal de début d'appel de groupe
const GKIND_SIGNAL: u8 = 4; // signalisation WebRTC (vidéo) pair-à-pair
const GKIND_GFILE: u8 = 5; // fichier diffusé dans le groupe (flux de contrôle)
const GKIND_GFDATA: u8 = 6; // flux de données d'un fichier de groupe (multi-flux)
pub const GKIND_VIDEO: u8 = 7; // partage d'écran NATIF (H.264 sur flux uni, video.rs)
const GKIND_GMEMBERS: u8 = 8; // sync de roster de groupe (ajout de membres → union)
const GKIND_KICK: u8 = 9; // un vote d'exclusion (vote-kick décentralisé)
const GKIND_GIMG: u8 = 11; // image inline de groupe (octets)
/// Taille maximale d'UNE image H.264 reçue (une keyframe 1440p à 12 Mb/s fait ~1-2 Mo) :
/// borne les allocations pilotées par le réseau (même famille de garde que GL-1).
const VIDEO_FRAME_MAX: usize = 8 * 1024 * 1024;
/// Borne dure d'allocation pour une image inline reçue (spec : inline ≤ 5 Mo côté UI).
const MAX_IMG_WIRE: usize = 8 * 1024 * 1024;

#[derive(Default)]
pub struct ConnState {
    generation: u64,
    conn: Option<Connection>,
}
pub type Slot = Arc<Mutex<ConnState>>;

/// Maillage de groupe : code permanent du pair → (jeton unique, connexion).
pub type Mesh = Arc<StdMutex<HashMap<String, (u64, Connection)>>>;
static MESH_SEQ: AtomicU64 = AtomicU64::new(1);

/// Canal binaire vers la WebView pour la vidéo native reçue (video_receive_attach).
/// Un seul récepteur : le dernier attach (rechargement de page) remplace le précédent.
pub type VideoRx = Arc<StdMutex<Option<tauri::ipc::Channel<tauri::ipc::InvokeResponseBody>>>>;

/// Codes dont une connexion de maillage est EN COURS d'établissement.
/// Évite que deux appels concurrents (invitation + open_group, ou deux groupes
/// partageant un membre au démarrage) ne composent le même pair en parallèle —
/// la 2ᵉ connexion fermait alors la 1ʳᵉ, faisant parfois perdre l'invitation.
pub type Connecting = Arc<StdMutex<HashSet<String>>>;

pub async fn current(slot: &Slot) -> Option<Connection> {
    slot.lock().await.conn.clone()
}

/// Réglages partagés, modifiables à chaud depuis l'UI.
#[derive(Clone, Default)]
pub struct Settings {
    pub download_dir: Arc<StdMutex<Option<PathBuf>>>,
    pub only_friends: Arc<AtomicBool>,
    pub friends: Arc<StdMutex<HashSet<String>>>,
    pub file_pending: Arc<StdMutex<HashMap<u64, tokio::sync::oneshot::Sender<bool>>>>,
    pub file_counter: Arc<AtomicU64>,
    // Offres de fichiers de GROUPE en attente d'autorisation (comme le 1-à-1).
    pub gfile_pending: Arc<StdMutex<HashMap<u64, tokio::sync::oneshot::Sender<bool>>>>,
    pub gfile_counter: Arc<AtomicU64>,
    // F5 : horodatage de la derniere demande de connexion par pair (anti-spam de la banniere).
    pub rate: Arc<StdMutex<HashMap<String, std::time::Instant>>>,
}

impl Settings {
    /// Dossier de réception courant (configuré, sinon Téléchargements, sinon temp).
    fn recv_dir(&self) -> PathBuf {
        self.download_dir
            .lock()
            .unwrap()
            .clone()
            .unwrap_or_else(|| dirs::download_dir().unwrap_or_else(std::env::temp_dir))
    }
    /// Un pair entrant est-il autorisé ? (toujours oui si le filtre amis est désactivé)
    fn allows(&self, peer_id: &str) -> bool {
        if !self.only_friends.load(Ordering::SeqCst) {
            return true;
        }
        self.friends.lock().unwrap_or_else(|e| e.into_inner()).contains(peer_id)
    }
}

/// Connexions entrantes en attente d'autorisation (id → canal de réponse Accepter/Refuser).
#[derive(Clone, Default)]
pub struct Incoming {
    pending: Arc<StdMutex<HashMap<u64, tokio::sync::oneshot::Sender<bool>>>>,
    counter: Arc<AtomicU64>,
}

/// Réponse de l'utilisateur à une demande de connexion entrante.
pub fn respond_incoming(incoming: &Incoming, id: u64, accept: bool) {
    if let Some(tx) = incoming.pending.lock().unwrap_or_else(|e| e.into_inner()).remove(&id) {
        let _ = tx.send(accept);
    }
}

#[derive(Clone)]
pub struct Ghost {
    pub app: AppHandle,
    pub slot: Slot,
    pub recv_cancel: Arc<AtomicBool>,
    pub settings: Settings,
    pub incoming: Incoming,
}

impl std::fmt::Debug for Ghost {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str("Ghost")
    }
}

impl ProtocolHandler for Ghost {
    async fn accept(&self, connection: Connection) -> Result<(), AcceptError> {
        let peer = connection.remote_id().to_string();
        // F5 : anti-spam — ignorer les connexions repetees d'un meme pair trop rapprochees.
        {
            let mut r = self.settings.rate.lock().unwrap_or_else(|e| e.into_inner());
            let now = std::time::Instant::now();
            if let Some(t) = r.get(&peer) {
                if now.duration_since(*t) < std::time::Duration::from_secs(2) {
                    connection.close(0u32.into(), b"rate-limited");
                    return Ok(());
                }
            }
            r.insert(peer.clone(), now);
        }
        // Filtre « amis uniquement » : on refuse les pairs inconnus avant tout.
        if !self.settings.allows(&peer) {
            connection.close(0u32.into(), b"not-a-friend");
            let _ = self.app.emit("ghost-refused", &peer);
            return Ok(());
        }
        // Demander l'autorisation à l'utilisateur (toujours), avec délai de 45 s.
        let id = self.incoming.counter.fetch_add(1, Ordering::SeqCst);
        let (tx, rx) = tokio::sync::oneshot::channel::<bool>();
        self.incoming.pending.lock().unwrap_or_else(|e| e.into_inner()).insert(id, tx);
        let _ = self
            .app
            .emit("ghost-incoming", serde_json::json!({ "id": id, "peer": peer }));
        let accepted = matches!(
            tokio::time::timeout(std::time::Duration::from_secs(45), rx).await,
            Ok(Ok(true))
        );
        self.incoming.pending.lock().unwrap_or_else(|e| e.into_inner()).remove(&id);
        if !accepted {
            connection.close(0u32.into(), b"refused");
            let _ = self
                .app
                .emit("ghost-incoming-cancel", serde_json::json!({ "id": id }));
            return Ok(());
        }
        run_conn(
            self.app.clone(),
            self.slot.clone(),
            self.recv_cancel.clone(),
            self.settings.clone(),
            connection,
        )
        .await;
        Ok(())
    }
}

/// Handler de présence : ne fait rien — une poignée de main réussie suffit à prouver qu'on est en ligne.
#[derive(Debug, Clone)]
pub struct Presence;

impl ProtocolHandler for Presence {
    async fn accept(&self, _connection: Connection) -> Result<(), AcceptError> {
        Ok(())
    }
}

pub struct Net {
    pub app: AppHandle,
    pub perm: Endpoint,
    _perm_router: Router,
    pub eph: Arc<Mutex<Eph>>,
    pub slot: Slot,
    pub send_cancel: Arc<AtomicBool>,
    pub recv_cancel: Arc<AtomicBool>,
    pub settings: Settings,
    pub incoming: Incoming,
    pub mesh: Mesh,
    pub connecting: Connecting,
    pub video_rx: VideoRx,
}

/// Identité éphémère : clé aléatoire en mémoire, remplaçable à chaud (rotation).
pub struct Eph {
    endpoint: Endpoint,
    _router: Router,
}

/// Emplacement du fichier d'identité (clé secrète ed25519), dans le dossier de données de l'app.
fn identity_path() -> PathBuf {
    let base = dirs::data_local_dir().unwrap_or_else(std::env::temp_dir);
    base.join("ghost-link").join("identity.key")
}

/// Ancien emplacement (Roaming) — pour migrer une identite creee avant le passage en Local.
fn legacy_identity_path() -> PathBuf {
    let base = dirs::data_dir().unwrap_or_else(std::env::temp_dir);
    base.join("ghost-link").join("identity.key")
}

/// Espace disque disponible (octets) pour ce dossier, si mesurable.
fn free_space(dir: &Path) -> Option<u64> {
    fs2::available_space(dir).ok()
}

/// Charge la clé secrète persistante, ou en crée une (et la sauvegarde) au premier lancement.
/// C'est elle qui fixe l'identité du nœud — donc le « code ami » — de façon stable dans le temps.
fn load_or_create_secret() -> SecretKey {
    let path = identity_path();
    // Migration : si une cle existe a l'ancien emplacement (Roaming) mais pas dans Local, la deplacer.
    if !path.exists() {
        let legacy = legacy_identity_path();
        if legacy != path && legacy.exists() {
            if let Some(parent) = path.parent() {
                let _ = std::fs::create_dir_all(parent);
            }
            if std::fs::rename(&legacy, &path).is_err() {
                if let Ok(b) = std::fs::read(&legacy) {
                    if std::fs::write(&path, &b).is_ok() {
                        let _ = std::fs::remove_file(&legacy);
                    }
                }
            }
        }
    }
    if let Ok(bytes) = std::fs::read(&path) {
        // Priorité au format chiffré (DPAPI) ; sinon ancien format clair (32 octets) → migration.
        if let Some(arr) = decrypt_key(&bytes) {
            return SecretKey::from_bytes(&arr);
        }
        if bytes.len() == 32 {
            let mut arr = [0u8; 32];
            arr.copy_from_slice(&bytes);
            let sk = SecretKey::from_bytes(&arr);
            save_secret(&path, &sk); // ré-écrit la clé chiffrée
            return sk;
        }
    }
    let sk = SecretKey::generate();
    save_secret(&path, &sk);
    sk
}

/// Écrit la clé d'identité, chiffrée au repos. Sous Windows via DPAPI : déchiffrable
/// uniquement par la même session Windows (illisible via une copie du fichier ou un autre compte).
fn save_secret(path: &Path, sk: &SecretKey) {
    if let Some(parent) = path.parent() {
        let _ = std::fs::create_dir_all(parent);
    }
    let data = encrypt_key(&sk.to_bytes());
    if std::fs::write(path, &data).is_ok() {
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            let _ = std::fs::set_permissions(path, std::fs::Permissions::from_mode(0o600));
        }
    }
}

#[cfg(windows)]
fn encrypt_key(plain: &[u8]) -> Vec<u8> {
    use windows_sys::Win32::Foundation::LocalFree;
    use windows_sys::Win32::Security::Cryptography::{CryptProtectData, CRYPT_INTEGER_BLOB};
    unsafe {
        let in_blob = CRYPT_INTEGER_BLOB {
            cbData: plain.len() as u32,
            pbData: plain.as_ptr() as *mut u8,
        };
        let mut out_blob = CRYPT_INTEGER_BLOB {
            cbData: 0,
            pbData: std::ptr::null_mut(),
        };
        let ok = CryptProtectData(
            &in_blob as *const CRYPT_INTEGER_BLOB,
            std::ptr::null(),
            std::ptr::null(),
            std::ptr::null(),
            std::ptr::null(),
            0,
            &mut out_blob as *mut CRYPT_INTEGER_BLOB,
        );
        if ok != 0 && !out_blob.pbData.is_null() {
            let v = std::slice::from_raw_parts(out_blob.pbData, out_blob.cbData as usize).to_vec();
            let _ = LocalFree(out_blob.pbData as *mut core::ffi::c_void);
            v
        } else {
            // Échec DPAPI : on ne bloque pas l'app (retombe sur le format clair).
            plain.to_vec()
        }
    }
}

#[cfg(windows)]
fn decrypt_key(stored: &[u8]) -> Option<[u8; 32]> {
    use windows_sys::Win32::Foundation::LocalFree;
    use windows_sys::Win32::Security::Cryptography::{CryptUnprotectData, CRYPT_INTEGER_BLOB};
    unsafe {
        let in_blob = CRYPT_INTEGER_BLOB {
            cbData: stored.len() as u32,
            pbData: stored.as_ptr() as *mut u8,
        };
        let mut out_blob = CRYPT_INTEGER_BLOB {
            cbData: 0,
            pbData: std::ptr::null_mut(),
        };
        let ok = CryptUnprotectData(
            &in_blob as *const CRYPT_INTEGER_BLOB,
            std::ptr::null_mut(),
            std::ptr::null(),
            std::ptr::null(),
            std::ptr::null(),
            0,
            &mut out_blob as *mut CRYPT_INTEGER_BLOB,
        );
        if ok == 0 || out_blob.pbData.is_null() {
            return None;
        }
        let slice = std::slice::from_raw_parts(out_blob.pbData, out_blob.cbData as usize);
        let res = if slice.len() == 32 {
            let mut arr = [0u8; 32];
            arr.copy_from_slice(slice);
            Some(arr)
        } else {
            None
        };
        let _ = LocalFree(out_blob.pbData as *mut core::ffi::c_void);
        res
    }
}

#[cfg(not(windows))]
fn encrypt_key(plain: &[u8]) -> Vec<u8> {
    plain.to_vec()
}

#[cfg(not(windows))]
fn decrypt_key(stored: &[u8]) -> Option<[u8; 32]> {
    if stored.len() == 32 {
        let mut arr = [0u8; 32];
        arr.copy_from_slice(stored);
        Some(arr)
    } else {
        None
    }
}

/// Fichier mémorisant le dossier de réception choisi.
fn download_dir_path() -> PathBuf {
    let base = dirs::data_dir().unwrap_or_else(std::env::temp_dir);
    base.join("ghost-link").join("download_dir.txt")
}
fn load_download_dir() -> Option<PathBuf> {
    let raw = std::fs::read_to_string(download_dir_path()).ok()?;
    let t = raw.trim();
    if t.is_empty() {
        None
    } else {
        Some(PathBuf::from(t))
    }
}
fn save_download_dir(dir: &str) {
    let path = download_dir_path();
    if let Some(parent) = path.parent() {
        let _ = std::fs::create_dir_all(parent);
    }
    let _ = std::fs::write(&path, dir.trim());
}

/// Définit (et mémorise) le dossier de réception. Chaîne vide = défaut (Téléchargements).
pub fn set_download_dir(settings: &Settings, path: &str) {
    let p = path.trim();
    *settings.download_dir.lock().unwrap_or_else(|e| e.into_inner()) = if p.is_empty() { None } else { Some(PathBuf::from(p)) };
    save_download_dir(p);
}
pub fn get_download_dir(settings: &Settings) -> String {
    settings.recv_dir().to_string_lossy().to_string()
}
pub fn set_only_friends(settings: &Settings, on: bool) {
    settings.only_friends.store(on, Ordering::SeqCst);
}
pub fn set_friends(settings: &Settings, codes: Vec<String>) {
    let mut s = settings.friends.lock().unwrap_or_else(|e| e.into_inner());
    s.clear();
    for c in codes {
        let c = c.trim();
        if !c.is_empty() {
            s.insert(c.to_string());
        }
    }
}

/// Réponse de l'utilisateur à une offre de fichier entrante (true = accepter, false = refuser).
pub fn respond_file(settings: &Settings, id: u64, accept: bool) {
    if let Some(tx) = settings.file_pending.lock().unwrap_or_else(|e| e.into_inner()).remove(&id) {
        let _ = tx.send(accept);
    }
}

/// Réponse de l'utilisateur à une offre de fichier de GROUPE entrante.
pub fn respond_gfile(settings: &Settings, id: u64, accept: bool) {
    if let Some(tx) = settings.gfile_pending.lock().unwrap_or_else(|e| e.into_inner()).remove(&id) {
        let _ = tx.send(accept);
    }
}

/// Construit un endpoint iroh (fenêtres QUIC élargies pour viser ~1 Gbps).
async fn build_endpoint(secret: SecretKey) -> anyhow::Result<Endpoint> {
    let transport = iroh::endpoint::QuicTransportConfig::builder()
        .stream_receive_window(iroh::endpoint::VarInt::from_u32(16 * 1024 * 1024))
        .receive_window(iroh::endpoint::VarInt::from_u32(64 * 1024 * 1024))
        .send_window(64 * 1024 * 1024)
        // Gros transferts : un fichier de 30 Go+ est haché (SHA-256) en entier AVANT
        // d'ouvrir le moindre flux ; sans keep-alive, la connexion reste inactive
        // pendant ce calcul et l'idle-timeout par défaut la coupe → l'envoi échoue.
        // Keep-alive 10 s + idle-timeout généreux (3 min) gardent la connexion vivante.
        .keep_alive_interval(std::time::Duration::from_secs(10))
        .max_idle_timeout(Some(std::time::Duration::from_secs(180).try_into().unwrap()))
        .build();
    Endpoint::builder(presets::N0)
        .secret_key(secret)
        .transport_config(transport)
        .bind()
        .await
        .map_err(|e| anyhow::anyhow!("bind iroh: {e}"))
}

/// Démarre le Router (protocoles fichier/chat/voix + présence) sur un endpoint.
#[allow(clippy::too_many_arguments)]
fn build_router(
    endpoint: &Endpoint,
    app: &AppHandle,
    slot: &Slot,
    recv_cancel: &Arc<AtomicBool>,
    settings: &Settings,
    incoming: &Incoming,
    mesh: &Mesh,
    video_rx: &VideoRx,
) -> Router {
    Router::builder(endpoint.clone())
        .accept(
            ALPN,
            Ghost {
                app: app.clone(),
                slot: slot.clone(),
                recv_cancel: recv_cancel.clone(),
                settings: settings.clone(),
                incoming: incoming.clone(),
            },
        )
        .accept(
            GROUP_ALPN,
            GroupHandler {
                app: app.clone(),
                mesh: mesh.clone(),
                settings: settings.clone(),
                video_rx: video_rx.clone(),
            },
        )
        .accept(PRESENCE_ALPN, Presence)
        .spawn()
}

// ===== Maillage de groupe (chat à plusieurs) =====

#[derive(Clone)]
pub struct GroupHandler {
    pub app: AppHandle,
    pub mesh: Mesh,
    pub settings: Settings,
    pub video_rx: VideoRx,
}
impl std::fmt::Debug for GroupHandler {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str("GroupHandler")
    }
}
impl ProtocolHandler for GroupHandler {
    async fn accept(&self, connection: Connection) -> Result<(), AcceptError> {
        let peer = connection.remote_id().to_string();
        // Le maillage de groupe n'accepte que les amis connus.
        let is_friend = self.settings.friends.lock().unwrap_or_else(|e| e.into_inner()).contains(&peer);
        if !is_friend {
            connection.close(0u32.into(), b"not-a-friend");
            return Ok(());
        }
        run_mesh_conn(self.app.clone(), self.mesh.clone(), self.settings.clone(), self.video_rx.clone(), peer, connection).await;
        Ok(())
    }
}

async fn read_lp16<R: AsyncReadExt + Unpin>(recv: &mut R) -> anyhow::Result<String> {
    let mut l = [0u8; 2];
    recv.read_exact(&mut l).await?;
    let n = u16::from_be_bytes(l) as usize;
    let mut b = vec![0u8; n];
    recv.read_exact(&mut b).await?;
    Ok(String::from_utf8_lossy(&b).to_string())
}
async fn read_lp32<R: AsyncReadExt + Unpin>(recv: &mut R) -> anyhow::Result<String> {
    let mut l = [0u8; 4];
    recv.read_exact(&mut l).await?;
    let n = u32::from_be_bytes(l) as usize;
    if n > 512 * 1024 {
        anyhow::bail!("message trop long");
    }
    let mut b = vec![0u8; n];
    recv.read_exact(&mut b).await?;
    Ok(String::from_utf8_lossy(&b).to_string())
}
async fn write_lp16<W: AsyncWriteExt + Unpin>(send: &mut W, s: &str) -> anyhow::Result<()> {
    let b = s.as_bytes();
    send.write_all(&(b.len() as u16).to_be_bytes()).await?;
    send.write_all(b).await?;
    Ok(())
}
async fn write_lp32<W: AsyncWriteExt + Unpin>(send: &mut W, s: &str) -> anyhow::Result<()> {
    let b = s.as_bytes();
    send.write_all(&(b.len() as u32).to_be_bytes()).await?;
    send.write_all(b).await?;
    Ok(())
}
async fn write_lp32_bytes<W: AsyncWriteExt + Unpin>(send: &mut W, b: &[u8]) -> anyhow::Result<()> {
    send.write_all(&(b.len() as u32).to_be_bytes()).await?;
    send.write_all(b).await?;
    Ok(())
}
async fn read_lp32_bytes<R: AsyncReadExt + Unpin>(recv: &mut R, max: usize) -> anyhow::Result<Vec<u8>> {
    let mut l = [0u8; 4];
    recv.read_exact(&mut l).await?;
    let n = u32::from_be_bytes(l) as usize;
    if n == 0 || n > max {
        anyhow::bail!("image trop grande"); // borne AVANT alloc
    }
    let mut b = vec![0u8; n];
    recv.read_exact(&mut b).await?;
    Ok(b)
}
fn mime_ok(m: &str) -> bool {
    matches!(m, "image/png" | "image/jpeg" | "image/gif" | "image/webp")
}

/// Seau à jetons du relais vidéo d'UN pair (partagé entre ses flux) : (budget, dernier refill).
type RelayBudget = Arc<StdMutex<(i64, std::time::Instant)>>;
/// Débit de relais soutenu par pair : SOUS les ~3,5 Mo/s que la WebView sait consommer
/// (mesure exp3), au-dessus des ~1,5 Mo/s du flux légitime le plus lourd (1440p, 12 Mb/s).
const RELAY_RATE: i64 = 2_621_440; // 2,5 Mio/s
const RELAY_BURST: i64 = 6 * 1024 * 1024;

/// Relaie les images d'un flux vidéo natif entrant vers la WebView (canal binaire).
/// Message poussé au JS : [u8 peer_len][peer][u8 flags][u64 frame_id][octets H.264].
/// Sans canal attaché (UI pas prête), les images sont lues et jetées (drainage).
///
/// Garde-fou anti-flood (même famille que GL-1) : le canal Tauri n'a AUCUNE
/// contre-pression — un pair qui pousse plus vite que ce que la WebView consomme
/// ferait grossir la file du renderer sans limite. Le budget est PARTAGÉ entre les
/// flux d'un même pair (sinon 2 flux = 2× le débit) ; une image au-dessus du budget
/// est lue puis JETÉE, et les deltas suivantes avec elle jusqu'à la prochaine image
/// clé (les relayer donnerait un GOP au référentiel manquant, décodé en bouillie).
async fn recv_video_frames(
    from: &str,
    recv: &mut iroh::endpoint::RecvStream,
    video_rx: &VideoRx,
    relay_budget: &RelayBudget,
) {
    let peer = from.as_bytes();
    if peer.len() > 255 {
        return;
    }
    let mut wait_key = false;
    loop {
        // Framing émetteur : [u64 frame_id][u8 flags][u32 len][len octets].
        let mut hdr = [0u8; 13];
        if AsyncReadExt::read_exact(recv, &mut hdr).await.is_err() {
            return; // fin du partage (FIN) ou connexion perdue
        }
        let len = u32::from_be_bytes([hdr[9], hdr[10], hdr[11], hdr[12]]) as usize;
        if len == 0 || len > VIDEO_FRAME_MAX {
            return; // taille aberrante : on coupe (borne anti-allocation, cf. GL-1)
        }
        let pl = peer.len();
        let mut msg = vec![0u8; 1 + pl + 1 + 8 + len];
        msg[0] = pl as u8;
        msg[1..1 + pl].copy_from_slice(peer);
        msg[1 + pl] = hdr[8]; // flags (bit0 = keyframe, bit1 = nouvelle session)
        msg[2 + pl..10 + pl].copy_from_slice(&hdr[..8]); // frame_id
        if AsyncReadExt::read_exact(recv, &mut msg[10 + pl..]).await.is_err() {
            return;
        }
        let key = hdr[8] & 1 == 1;
        if wait_key && !key {
            continue; // GOP amputé par un rejet précédent : attendre la prochaine clé
        }
        let allowed = {
            let mut b = relay_budget.lock().unwrap_or_else(|e| e.into_inner());
            let now = std::time::Instant::now();
            b.0 = (b.0 + (now - b.1).as_micros() as i64 * RELAY_RATE / 1_000_000).min(RELAY_BURST);
            b.1 = now;
            if b.0 < len as i64 {
                false
            } else {
                b.0 -= len as i64;
                true
            }
        };
        if !allowed {
            wait_key = true; // image jetée (flood ou WebView à la traîne)
            continue;
        }
        wait_key = false;
        let ch = video_rx.lock().unwrap_or_else(|e| e.into_inner()).clone();
        if let Some(ch) = ch {
            let _ = ch.send(tauri::ipc::InvokeResponseBody::Raw(msg));
        }
    }
}

/// Boucle de réception d'une connexion de maillage (un pair du groupe).
async fn run_mesh_conn(app: AppHandle, mesh: Mesh, settings: Settings, video_rx: VideoRx, peer: String, connection: Connection) {
    let token = MESH_SEQ.fetch_add(1, Ordering::SeqCst);
    if let Some((_, old)) = mesh
        .lock()
        .unwrap_or_else(|e| e.into_inner())
        .insert(peer.clone(), (token, connection.clone()))
    {
        old.close(0u32.into(), b"reconnect");
    }
    let _ = app.emit("ghost-mesh-up", &peer);
    let inbounds: Inbounds = Arc::new(StdMutex::new(HashMap::new()));

    // Flux UNI-directionnels entrants : vidéo native (GKIND_VIDEO). Tâche sœur de la
    // boucle bi ci-dessous, abandonnée quand la connexion tombe (uni_task.abort()).
    let uni_task = {
        let conn = connection.clone();
        let from = peer.clone();
        let video_rx = video_rx.clone();
        let app = app.clone();
        tokio::spawn(async move {
            // Au plus 2 flux vidéo décodés en même temps par pair : le cas légitime
            // en vaut 1 (+1 pour le chevauchement stop/relance) — au-delà, c'est un
            // pair hostile qui cherche à saturer CPU/mémoire : flux ignorés. Le seau
            // à jetons du relais est LUI AUSSI par pair (partagé entre ses flux).
            let active = Arc::new(AtomicU64::new(0));
            let relay_budget: RelayBudget =
                Arc::new(StdMutex::new((RELAY_BURST, std::time::Instant::now())));
            while let Ok(mut recv) = conn.accept_uni().await {
                let from = from.clone();
                let video_rx = video_rx.clone();
                let app = app.clone();
                let active = active.clone();
                let relay_budget = relay_budget.clone();
                tokio::spawn(async move {
                    let mut kind = [0u8; 1];
                    if AsyncReadExt::read_exact(&mut recv, &mut kind).await.is_err() {
                        return;
                    }
                    if kind[0] == GKIND_VIDEO {
                        if active.fetch_add(1, Ordering::SeqCst) >= 2 {
                            active.fetch_sub(1, Ordering::SeqCst);
                            return;
                        }
                        recv_video_frames(&from, &mut recv, &video_rx, &relay_budget).await;
                        active.fetch_sub(1, Ordering::SeqCst);
                        // Le flux est fini (arrêt, erreur, connexion) : le dire à l'UI,
                        // sinon la vignette resterait figée en ayant l'air vivante.
                        let _ = app.emit("ghost-video-rx-end", &from);
                    }
                    // Autre type : ignoré (compat ascendante + sécurité).
                });
            }
        })
    };

    loop {
        match connection.accept_bi().await {
            Ok((mut send, mut recv)) => {
                let a = app.clone();
                let from = peer.clone();
                let settings = settings.clone();
                let inbounds = inbounds.clone();
                tokio::spawn(async move {
                    let mut kind = [0u8; 1];
                    if recv.read_exact(&mut kind).await.is_err() {
                        return;
                    }
                    if kind[0] == GKIND_CHAT {
                        if let (Ok(gid), Ok(author), Ok(text)) = (
                            read_lp16(&mut recv).await,
                            read_lp16(&mut recv).await,
                            read_lp32(&mut recv).await,
                        ) {
                            let _ = a.emit("ghost-gchat", serde_json::json!({ "group": gid, "author": author, "text": text, "from": from }));
                        }
                    } else if kind[0] == GKIND_INVITE {
                        if let (Ok(gid), Ok(name), Ok(members)) = (
                            read_lp16(&mut recv).await,
                            read_lp16(&mut recv).await,
                            read_lp32(&mut recv).await,
                        ) {
                            let _ = a.emit("ghost-ginvite", serde_json::json!({ "id": gid, "name": name, "members": members, "from": from }));
                        }
                    } else if kind[0] == GKIND_CALL {
                        if let Ok(gid) = read_lp16(&mut recv).await {
                            let _ = a.emit("ghost-gcall", serde_json::json!({ "group": gid, "from": from }));
                        }
                    } else if kind[0] == GKIND_SIGNAL {
                        if let Ok(data) = read_lp32(&mut recv).await {
                            let _ = a.emit("ghost-signal", serde_json::json!({ "from": from, "data": data }));
                        }
                    } else if kind[0] == GKIND_GMEMBERS {
                        // Sync de roster : un membre a ajouté des gens → l'UI fait l'union.
                        if let (Ok(gid), Ok(name), Ok(members)) = (
                            read_lp16(&mut recv).await,
                            read_lp16(&mut recv).await,
                            read_lp32(&mut recv).await,
                        ) {
                            let _ = a.emit("ghost-gmembers", serde_json::json!({ "group": gid, "name": name, "members": members, "from": from }));
                        }
                    } else if kind[0] == GKIND_KICK {
                        // Un vote d'exclusion : [gid][cible][votant]. L'UI tallie et applique.
                        if let (Ok(gid), Ok(target), Ok(voter)) = (
                            read_lp16(&mut recv).await,
                            read_lp16(&mut recv).await,
                            read_lp16(&mut recv).await,
                        ) {
                            let _ = a.emit("ghost-kick", serde_json::json!({ "group": gid, "target": target, "voter": voter, "from": from }));
                        }
                    } else if kind[0] == GKIND_GIMG {
                        let parsed: anyhow::Result<(String, String, String, String, Vec<u8>)> = async {
                            let gid = read_lp16(&mut recv).await?;
                            let author = read_lp16(&mut recv).await?;
                            let name = read_lp16(&mut recv).await?;
                            let mime = read_lp16(&mut recv).await?;
                            if !mime_ok(&mime) {
                                anyhow::bail!("mime refusé");
                            }
                            let data = read_lp32_bytes(&mut recv, MAX_IMG_WIRE).await?;
                            Ok((gid, author, name, mime, data))
                        }
                        .await;
                        if let Ok((gid, author, name, mime, data)) = parsed {
                            use base64::Engine;
                            let b64 = base64::engine::general_purpose::STANDARD.encode(&data);
                            let _ = a.emit("ghost-gchat-img", serde_json::json!({ "group": gid, "author": author, "name": name, "mime": mime, "dataB64": b64, "from": from }));
                        }
                    } else if kind[0] == GKIND_GFILE {
                        let _ = recv_gfile(&a, &settings, &from, &mut send, &mut recv, &inbounds).await;
                    } else if kind[0] == GKIND_GFDATA {
                        // Flux de données d'un fichier de groupe : [u64 id][u64 offset][u64 len] puis octets.
                        let hdr: anyhow::Result<(u64, u64, u64)> = async {
                            let mut b = [0u8; 8];
                            AsyncReadExt::read_exact(&mut recv, &mut b).await?;
                            let id = u64::from_be_bytes(b);
                            AsyncReadExt::read_exact(&mut recv, &mut b).await?;
                            let offset = u64::from_be_bytes(b);
                            AsyncReadExt::read_exact(&mut recv, &mut b).await?;
                            let len = u64::from_be_bytes(b);
                            anyhow::Ok((id, offset, len))
                        }
                        .await;
                        let (id, offset, len) = match hdr { Ok(v) => v, Err(_) => return };
                        let mut inb = None;
                        for _ in 0..200 {
                            if let Some(x) = inbounds.lock().unwrap_or_else(|e| e.into_inner()).get(&id) {
                                inb = Some(x.clone());
                                break;
                            }
                            tokio::time::sleep(std::time::Duration::from_millis(50)).await;
                        }
                        let inb = match inb { Some(x) => x, None => return };
                        // GL-1 : borne le flux contre la taille acceptée (cf. chemin 1-à-1).
                        match offset.checked_add(len) {
                            Some(end) if end <= inb.size => {}
                            _ => return,
                        }
                        let mut buf = vec![0u8; CHUNK];
                        let mut pos = offset;
                        let mut remaining = len;
                        while remaining > 0 {
                            if inb.cancelled.load(Ordering::SeqCst) { return; }
                            let want = std::cmp::min(CHUNK as u64, remaining) as usize;
                            if AsyncReadExt::read_exact(&mut recv, &mut buf[..want]).await.is_err() { return; }
                            {
                                use tokio::io::AsyncSeekExt;
                                let mut f = inb.file.lock().await;
                                if f.seek(std::io::SeekFrom::Start(pos)).await.is_err()
                                    || AsyncWriteExt::write_all(&mut *f, &buf[..want]).await.is_err() { return; }
                            }
                            pos += want as u64;
                            remaining -= want as u64;
                            inb.received.fetch_add(want as u64, Ordering::SeqCst);
                        }
                    }
                });
            }
            Err(_) => break,
        }
    }
    uni_task.abort();
    {
        let mut m = mesh.lock().unwrap_or_else(|e| e.into_inner());
        if m.get(&peer).map(|(t, _)| *t == token).unwrap_or(false) {
            m.remove(&peer);
        }
    }
    let _ = app.emit("ghost-mesh-down", &peer);
}

/// Renvoie la connexion de maillage vers `code`, en l'ouvrant si besoin (ALPN groupe).
/// Garde anti-course : si un autre appel compose déjà ce pair, on patiente que sa
/// connexion apparaisse plutôt que d'en ouvrir une 2ᵉ (qui fermerait la 1ʳᵉ et
/// pouvait faire perdre l'invitation portée par cette connexion).
async fn ensure_mesh(net: &Net, code: &str) -> anyhow::Result<Connection> {
    if let Some((_, c)) = net.mesh.lock().unwrap_or_else(|e| e.into_inner()).get(code) {
        return Ok(c.clone());
    }
    // Réserver le dial, ou attendre brièvement qu'un dial concurrent aboutisse.
    let mut waited = 0u64;
    loop {
        {
            if let Some((_, c)) = net.mesh.lock().unwrap_or_else(|e| e.into_inner()).get(code) {
                return Ok(c.clone());
            }
            let mut connecting = net.connecting.lock().unwrap_or_else(|e| e.into_inner());
            if !connecting.contains(code) {
                connecting.insert(code.to_string());
                break; // c'est nous qui composons ce pair
            }
        }
        if waited >= 8000 {
            anyhow::bail!("connexion de groupe déjà en cours");
        }
        tokio::time::sleep(std::time::Duration::from_millis(80)).await;
        waited += 80;
    }
    let dialed = async {
        let id: EndpointId = code.trim().parse().map_err(|_| anyhow::anyhow!("code invalide"))?;
        let addr = EndpointAddr::from(id);
        tokio::time::timeout(
            std::time::Duration::from_secs(8),
            net.perm.connect(addr, GROUP_ALPN),
        )
        .await
        .map_err(|_| anyhow::anyhow!("délai dépassé"))?
        .map_err(|e| anyhow::anyhow!("connexion groupe: {e}"))
    }
    .await;
    net.connecting.lock().unwrap_or_else(|e| e.into_inner()).remove(code);
    let conn = dialed?;
    let app = net.app.clone();
    let mesh = net.mesh.clone();
    let settings = net.settings.clone();
    let video_rx = net.video_rx.clone();
    let peer = code.to_string();
    let c2 = conn.clone();
    tokio::spawn(async move { run_mesh_conn(app, mesh, settings, video_rx, peer, c2).await });
    Ok(conn)
}

/// Ouvre/rejoint un groupe : se connecte à chaque membre en ligne, en parallèle (non bloquant).
pub async fn open_group(net: &Net, members: Vec<String>) {
    for code in members {
        let code = code.trim().to_string();
        if code.is_empty() {
            continue;
        }
        // Sauter si déjà connecté OU déjà en cours de connexion (garde anti double-dial).
        {
            if net.mesh.lock().unwrap_or_else(|e| e.into_inner()).contains_key(&code) {
                continue;
            }
            let mut connecting = net.connecting.lock().unwrap_or_else(|e| e.into_inner());
            if connecting.contains(&code) {
                continue;
            }
            connecting.insert(code.clone());
        }
        let perm = net.perm.clone();
        let mesh = net.mesh.clone();
        let app = net.app.clone();
        let settings = net.settings.clone();
        let video_rx = net.video_rx.clone();
        let connecting = net.connecting.clone();
        tokio::spawn(async move {
            let id: EndpointId = match code.parse() {
                Ok(i) => i,
                Err(_) => {
                    connecting.lock().unwrap_or_else(|e| e.into_inner()).remove(&code);
                    return;
                }
            };
            let addr = EndpointAddr::from(id);
            let conn = match tokio::time::timeout(
                std::time::Duration::from_secs(8),
                perm.connect(addr, GROUP_ALPN),
            )
            .await
            {
                Ok(Ok(c)) => c,
                _ => {
                    connecting.lock().unwrap_or_else(|e| e.into_inner()).remove(&code);
                    return; // membre hors ligne / échec
                }
            };
            connecting.lock().unwrap_or_else(|e| e.into_inner()).remove(&code);
            run_mesh_conn(app, mesh, settings, video_rx, code, conn).await;
        });
    }
}

/// Diffuse un message de channel aux membres du groupe présents dans le maillage.
pub async fn send_gchat(
    net: &Net,
    members: Vec<String>,
    gid: &str,
    author: &str,
    text: &str,
) -> anyhow::Result<()> {
    let targets: Vec<Connection> = {
        let m = net.mesh.lock().unwrap_or_else(|e| e.into_inner());
        members
            .iter()
            .filter_map(|code| m.get(code.trim()).map(|(_, c)| c.clone()))
            .collect()
    };
    for conn in targets {
        if let Ok((mut send, _recv)) = conn.open_bi().await {
            let _ = send.write_all(&[GKIND_CHAT]).await;
            let _ = write_lp16(&mut send, gid).await;
            let _ = write_lp16(&mut send, author).await;
            let _ = write_lp32(&mut send, text).await;
            let _ = send.finish();
        }
    }
    Ok(())
}

/// Diffuse une image inline (octets) aux membres du groupe présents.
pub async fn send_gimg(net: &Net, members: Vec<String>, gid: &str, author: &str, name: &str, mime: &str, data: &[u8]) -> anyhow::Result<()> {
    let targets: Vec<Connection> = {
        let m = net.mesh.lock().unwrap_or_else(|e| e.into_inner());
        members.iter().filter_map(|c| m.get(c.trim()).map(|(_, c)| c.clone())).collect()
    };
    for conn in targets {
        if let Ok((mut send, _r)) = conn.open_bi().await {
            let _ = send.write_all(&[GKIND_GIMG]).await;
            let _ = write_lp16(&mut send, gid).await;
            let _ = write_lp16(&mut send, author).await;
            let _ = write_lp16(&mut send, name).await;
            let _ = write_lp16(&mut send, mime).await;
            let _ = write_lp32_bytes(&mut send, data).await;
            let _ = send.finish();
        }
    }
    Ok(())
}

/// Diffuse le roster à jour d'un groupe aux membres présents (ajout de membres → ils
/// font l'union). `roster` = CSV de TOUS les membres (le nouvel état). Best-effort :
/// un membre hors ligne rattrapera à une rediffusion ultérieure.
pub async fn send_gmembers(
    net: &Net,
    members: Vec<String>,
    gid: &str,
    name: &str,
    roster: &str,
) -> anyhow::Result<()> {
    let targets: Vec<Connection> = {
        let m = net.mesh.lock().unwrap_or_else(|e| e.into_inner());
        members
            .iter()
            .filter_map(|code| m.get(code.trim()).map(|(_, c)| c.clone()))
            .collect()
    };
    for conn in targets {
        if let Ok((mut send, _recv)) = conn.open_bi().await {
            let _ = send.write_all(&[GKIND_GMEMBERS]).await;
            let _ = write_lp16(&mut send, gid).await;
            let _ = write_lp16(&mut send, name).await;
            let _ = write_lp32(&mut send, roster).await;
            let _ = send.finish();
        }
    }
    Ok(())
}

/// Diffuse MON vote d'exclusion de `target` aux membres du groupe en ligne. Chaque
/// client tallie de son côté ; le quorum (60 % des en-ligne) déclenche le retrait.
pub async fn send_kick(
    net: &Net,
    members: Vec<String>,
    gid: &str,
    target: &str,
    voter: &str,
) -> anyhow::Result<()> {
    let targets: Vec<Connection> = {
        let m = net.mesh.lock().unwrap_or_else(|e| e.into_inner());
        members
            .iter()
            .filter_map(|code| m.get(code.trim()).map(|(_, c)| c.clone()))
            .collect()
    };
    for conn in targets {
        if let Ok((mut send, _recv)) = conn.open_bi().await {
            let _ = send.write_all(&[GKIND_KICK]).await;
            let _ = write_lp16(&mut send, gid).await;
            let _ = write_lp16(&mut send, target).await;
            let _ = write_lp16(&mut send, voter).await;
            let _ = send.finish();
        }
    }
    Ok(())
}

/// Envoie une invitation de groupe à un membre (le connecte au maillage si besoin).
pub async fn send_ginvite(
    net: &Net,
    member: &str,
    gid: &str,
    name: &str,
    members_csv: &str,
) -> anyhow::Result<()> {
    let conn = ensure_mesh(net, member).await?;
    let (mut send, _recv) = conn
        .open_bi()
        .await
        .map_err(|e| anyhow::anyhow!("flux: {e}"))?;
    send.write_all(&[GKIND_INVITE])
        .await
        .map_err(|e| anyhow::anyhow!("envoi: {e}"))?;
    write_lp16(&mut send, gid).await?;
    write_lp16(&mut send, name).await?;
    write_lp32(&mut send, members_csv).await?;
    send.finish().map_err(|e| anyhow::anyhow!("finish: {e}"))?;
    Ok(())
}

/// Renvoie les connexions du maillage vers les membres donnés (ceux présents en ligne).
pub fn group_conns(net: &Net, members: &[String]) -> Vec<(String, Connection)> {
    let m = net.mesh.lock().unwrap_or_else(|e| e.into_inner());
    members
        .iter()
        .filter_map(|code| {
            let code = code.trim();
            m.get(code).map(|(_, c)| (code.to_string(), c.clone()))
        })
        .collect()
}

/// Annonce le début d'un appel de groupe aux membres (ils proposeront de rejoindre).
pub async fn send_gcall(net: &Net, members: Vec<String>, gid: &str) -> anyhow::Result<()> {
    let targets: Vec<Connection> = {
        let m = net.mesh.lock().unwrap_or_else(|e| e.into_inner());
        members
            .iter()
            .filter_map(|code| m.get(code.trim()).map(|(_, c)| c.clone()))
            .collect()
    };
    for conn in targets {
        if let Ok((mut send, _recv)) = conn.open_bi().await {
            let _ = send.write_all(&[GKIND_CALL]).await;
            let _ = write_lp16(&mut send, gid).await;
            let _ = send.finish();
        }
    }
    Ok(())
}

/// Envoie un message de signalisation WebRTC (vidéo) à UN pair précis du maillage.
pub async fn send_signal(net: &Net, peer: &str, data: &str) -> anyhow::Result<()> {
    let conn = net.mesh.lock().unwrap_or_else(|e| e.into_inner()).get(peer.trim()).map(|(_, c)| c.clone());
    let conn = conn.ok_or_else(|| anyhow::anyhow!("pair non connecté"))?;
    let (mut send, _recv) = conn
        .open_bi()
        .await
        .map_err(|e| anyhow::anyhow!("flux: {e}"))?;
    send.write_all(&[GKIND_SIGNAL])
        .await
        .map_err(|e| anyhow::anyhow!("envoi: {e}"))?;
    write_lp32(&mut send, data).await?;
    send.finish().map_err(|e| anyhow::anyhow!("finish: {e}"))?;
    Ok(())
}

/// Reçoit un fichier de groupe : entête → DEMANDE D'AUTORISATION (comme le 1-à-1) → octets.
async fn recv_gfile<R: AsyncReadExt + Unpin, W: AsyncWriteExt + Unpin>(
    app: &AppHandle,
    settings: &Settings,
    from: &str,
    send: &mut W,
    recv: &mut R,
    inbounds: &Inbounds,
) -> anyhow::Result<()> {
    // En-tête (flux de contrôle) : [u64 id][u16 nom_len][nom][u64 taille][32 hash][u8 nflux]
    let mut b8 = [0u8; 8];
    recv.read_exact(&mut b8).await?;
    let id = u64::from_be_bytes(b8);
    let mut l2 = [0u8; 2];
    recv.read_exact(&mut l2).await?;
    let nlen = u16::from_be_bytes(l2) as usize;
    let mut nbuf = vec![0u8; nlen];
    recv.read_exact(&mut nbuf).await?;
    let name = sanitize(&String::from_utf8_lossy(&nbuf));
    recv.read_exact(&mut b8).await?;
    let size = u64::from_be_bytes(b8);
    let mut hash = [0u8; 32];
    recv.read_exact(&mut hash).await?;
    let mut nflux = [0u8; 1];
    recv.read_exact(&mut nflux).await?;

    // SEC-2 : refuser si l'espace disque libre est insuffisant (marge 64 Mo).
    if let Some(free) = free_space(&settings.recv_dir()) {
        if size > free.saturating_sub(64 * 1024 * 1024) {
            let _ = send.write_all(&[0u8]).await;
            let _ = send.flush().await;
            let _ = app.emit("ghost-recv-nospace", serde_json::json!({ "name": name, "size": size, "from": from }));
            return Ok(());
        }
    }

    // Demander l'autorisation AVANT de recevoir (plus de téléchargement silencieux).
    let offer_id = settings.gfile_counter.fetch_add(1, Ordering::SeqCst);
    let (otx, orx) = tokio::sync::oneshot::channel::<bool>();
    settings.gfile_pending.lock().unwrap_or_else(|e| e.into_inner()).insert(offer_id, otx);
    let _ = app.emit("ghost-grecv-offer", serde_json::json!({ "id": offer_id, "name": name, "size": size, "from": from }));
    let accepted = matches!(
        tokio::time::timeout(std::time::Duration::from_secs(120), orx).await,
        Ok(Ok(true))
    );
    settings.gfile_pending.lock().unwrap_or_else(|e| e.into_inner()).remove(&offer_id);
    if !accepted {
        let _ = send.write_all(&[0u8]).await; // refus
        let _ = send.flush().await;
        let _ = app.emit("ghost-grecv-rejected", serde_json::json!({ "id": offer_id, "name": name, "from": from }));
        return Ok(());
    }
    send.write_all(&[1u8]).await?; // accepté
    send.flush().await?;

    // Pré-allouer le fichier + enregistrer le transfert (les flux GKIND_GFDATA le rempliront).
    let dir = settings.recv_dir();
    let dest = unique_path(&dir, &name);
    let created = async {
        let f = tokio::fs::File::create(&dest).await?;
        f.set_len(size).await?;
        anyhow::Ok(f)
    }
    .await;
    let file = match created { Ok(f) => f, Err(_) => return Ok(()) };
    let inb = Arc::new(Inbound {
        file: tokio::sync::Mutex::new(file),
        received: AtomicU64::new(0),
        cancelled: AtomicBool::new(false),
        size,
    });
    inbounds.lock().unwrap_or_else(|e| e.into_inner()).insert(id, inb.clone());
    let _ = app.emit("ghost-grecv-start", serde_json::json!({ "name": name, "size": size, "from": from }));

    // Attendre le réassemblage complet (GL-LF-1 : inactivité 5 min mesurée en temps réel,
    // pour ne pas abandonner un gros fichier de groupe sur une pause réseau passagère).
    let mut last_got = 0u64;
    let mut last_progress = std::time::Instant::now();
    let mut done = false;
    loop {
        let got = inb.received.load(Ordering::SeqCst);
        if got >= size { done = true; break; }
        if got != last_got { last_got = got; last_progress = std::time::Instant::now(); }
        if last_progress.elapsed().as_secs() >= 300 { break; }
        tokio::time::sleep(std::time::Duration::from_millis(100)).await;
    }
    inbounds.lock().unwrap_or_else(|e| e.into_inner()).remove(&id);
    {
        let mut f = inb.file.lock().await;
        let _ = AsyncWriteExt::flush(&mut *f).await;
    }
    let ok_hash = done && sha256_file(&dest).await.map(|h| h == hash).unwrap_or(false);
    if ok_hash {
        let _ = app.emit("ghost-grecv-done", serde_json::json!({ "name": name, "from": from, "path": dest.to_string_lossy() }));
    } else {
        let _ = tokio::fs::remove_file(&dest).await;
        let _ = app.emit("ghost-grecv-corrupt", serde_json::json!({ "name": name, "from": from }));
    }
    Ok(())
}

/// Envoie un fichier à tous les membres en ligne du groupe (un flux par membre, sans accusé).
pub async fn send_gfile(net: &Net, members: Vec<String>, path: &str) -> anyhow::Result<()> {
    let conns = group_conns(net, &members);
    if conns.is_empty() {
        anyhow::bail!("aucun membre du groupe en ligne");
    }
    let p = Path::new(path);
    // Le NOM affiché vient toujours du fichier original ; les octets lus viennent de
    // la copie NETTOYÉE de ses métadonnées (voir meta.rs) quand il y en a une.
    let name = p
        .file_name()
        .and_then(|s| s.to_str())
        .unwrap_or("fichier")
        .to_string();
    let read_path = prepare_meta(&net.app, path, &name).await;
    let size = tokio::fs::metadata(Path::new(&read_path))
        .await
        .map_err(|e| anyhow::anyhow!("fichier introuvable: {e}"))?
        .len();
    let app = net.app.clone();
    for (peer, conn) in conns {
        let app = app.clone();
        let name = name.clone();
        let path = read_path.clone();
        tokio::spawn(async move {
            if send_one_gfile(&conn, &path, &name, size).await.is_ok() {
                let _ = app.emit("ghost-gsent", serde_json::json!({ "name": name, "to": peer }));
            }
        });
    }
    Ok(())
}

/// Nettoie les métadonnées du fichier avant envoi (meta.rs) et prévient l'UI du
/// résultat. Renvoie le chemin à LIRE : la copie nettoyée, ou l'original si rien à
/// changer / nettoyage impossible (dans ce dernier cas l'UI est avertie — on n'échoue
/// jamais l'envoi pour ça, mais on ne se tait jamais non plus).
async fn prepare_meta(app: &AppHandle, path: &str, name: &str) -> String {
    let owned = path.to_string();
    let prep = tokio::task::spawn_blocking(move || crate::meta::prepare(Path::new(&owned)))
        .await
        .unwrap_or_else(|e| crate::meta::Prep::Failed(format!("préparation interrompue: {e}")));
    match &prep {
        crate::meta::Prep::Cleaned(_) => {
            let _ = app.emit("ghost-meta", serde_json::json!({ "name": name, "status": "cleaned" }));
        }
        crate::meta::Prep::Skipped(info) => {
            let _ = app.emit("ghost-meta", serde_json::json!({ "name": name, "status": "skipped", "info": info }));
        }
        crate::meta::Prep::Failed(info) => {
            let _ = app.emit("ghost-meta", serde_json::json!({ "name": name, "status": "failed", "info": info }));
        }
        crate::meta::Prep::Untouched => {}
    }
    match prep {
        crate::meta::Prep::Cleaned(tmp) => tmp.to_string_lossy().to_string(),
        _ => path.to_string(),
    }
}

async fn send_one_gfile(conn: &Connection, path: &str, name: &str, size: u64) -> anyhow::Result<()> {
    // Flux de CONTRÔLE : entête + accord. Les octets passent par N flux GKIND_GFDATA parallèles.
    let hash = sha256_file(Path::new(path)).await.map_err(|e| anyhow::anyhow!("hash: {e}"))?;
    let id = FILE_SEQ.fetch_add(1, Ordering::SeqCst);
    let nstreams = STREAMS.load(Ordering::SeqCst).clamp(1, 8);
    let (mut send, mut recv) = conn
        .open_bi()
        .await
        .map_err(|e| anyhow::anyhow!("flux: {e}"))?;
    let mut head = Vec::with_capacity(1 + 8 + 2 + name.len() + 8 + 32 + 1);
    head.push(GKIND_GFILE);
    head.extend_from_slice(&id.to_be_bytes());
    let nb = name.as_bytes();
    head.extend_from_slice(&(nb.len() as u16).to_be_bytes());
    head.extend_from_slice(nb);
    head.extend_from_slice(&size.to_be_bytes());
    head.extend_from_slice(&hash);
    head.push(nstreams as u8);
    send.write_all(&head).await?;
    // Attendre la décision du destinataire (accepté / refusé) avant d'envoyer les octets.
    let mut decision = [0u8; 1];
    AsyncReadExt::read_exact(&mut recv, &mut decision)
        .await
        .map_err(|e| anyhow::anyhow!("le pair n'a pas répondu: {e}"))?;
    if decision[0] != 1 {
        anyhow::bail!("refusé par le pair");
    }
    // Découper en NSTREAMS tranches contiguës, une par flux parallèle.
    let part = if size == 0 { 0 } else { (size + nstreams - 1) / nstreams };
    let mut tasks = Vec::new();
    for i in 0..nstreams {
        let offset = i * part;
        if offset >= size {
            break;
        }
        let len = std::cmp::min(part, size - offset);
        tasks.push(tokio::spawn(send_one_gpart(conn.clone(), id, offset, len, path.to_string())));
    }
    let mut first_err: Option<anyhow::Error> = None;
    for t in tasks {
        match t.await {
            Ok(Ok(())) => {}
            Ok(Err(e)) => {
                if first_err.is_none() {
                    first_err = Some(e);
                }
            }
            Err(e) => {
                if first_err.is_none() {
                    first_err = Some(anyhow::anyhow!("tâche d'envoi: {e}"));
                }
            }
        }
    }
    if let Some(e) = first_err {
        let _ = send.reset(0u32.into());
        return Err(e);
    }
    let _ = send.finish();
    Ok(())
}

/// Envoie une tranche [offset, offset+len) d'un fichier de groupe sur son propre flux.
/// En-tête : [GKIND_GFDATA][u64 id][u64 offset][u64 len] puis les octets.
async fn send_one_gpart(conn: Connection, id: u64, offset: u64, len: u64, path: String) -> anyhow::Result<()> {
    use tokio::io::AsyncSeekExt;
    let (mut send, _recv) = conn
        .open_bi()
        .await
        .map_err(|e| anyhow::anyhow!("flux: {e}"))?;
    let mut hdr = Vec::with_capacity(25);
    hdr.push(GKIND_GFDATA);
    hdr.extend_from_slice(&id.to_be_bytes());
    hdr.extend_from_slice(&offset.to_be_bytes());
    hdr.extend_from_slice(&len.to_be_bytes());
    send.write_all(&hdr).await?;
    let mut file = tokio::fs::File::open(&path).await?;
    file.seek(std::io::SeekFrom::Start(offset)).await?;
    let mut buf = vec![0u8; CHUNK];
    let mut remaining = len;
    while remaining > 0 {
        let want = std::cmp::min(CHUNK as u64, remaining) as usize;
        AsyncReadExt::read_exact(&mut file, &mut buf[..want]).await?;
        send.write_all(&buf[..want]).await?;
        remaining -= want as u64;
    }
    send.finish().map_err(|e| anyhow::anyhow!("finish: {e}"))?;
    Ok(())
}

/// Code permanent (identité stable, à partager seulement avec les amis).
pub fn perm_code(net: &Net) -> String {
    net.perm.addr().id.to_string()
}

/// Code éphémère du moment (jetable, régénéré à chaque lancement).
pub async fn eph_code(net: &Net) -> String {
    net.eph.lock().await.endpoint.addr().id.to_string()
}

/// Régénère le code éphémère : nouvel endpoint aléatoire, l'ancien est jeté.
pub async fn rotate_eph(net: &Net) -> anyhow::Result<String> {
    let endpoint = build_endpoint(SecretKey::generate()).await?;
    let router = build_router(
        &endpoint,
        &net.app,
        &net.slot,
        &net.recv_cancel,
        &net.settings,
        &net.incoming,
        &net.mesh,
        &net.video_rx,
    );
    let id = endpoint.addr().id.to_string();
    let mut g = net.eph.lock().await;
    *g = Eph { endpoint, _router: router };
    Ok(id)
}

pub async fn start(app: AppHandle) -> anyhow::Result<Net> {
    let slot: Slot = Arc::new(Mutex::new(ConnState::default()));
    let send_cancel = Arc::new(AtomicBool::new(false));
    let recv_cancel = Arc::new(AtomicBool::new(false));
    let settings = Settings::default();
    if let Some(dir) = load_download_dir() {
        *settings.download_dir.lock().unwrap_or_else(|e| e.into_inner()) = Some(dir);
    }
    let incoming = Incoming::default();
    let mesh: Mesh = Arc::new(StdMutex::new(HashMap::new()));
    let connecting: Connecting = Arc::new(StdMutex::new(HashSet::new()));
    let video_rx: VideoRx = Arc::new(StdMutex::new(None));

    // Identité PERMANENTE : clé persistante = code ami stable.
    let perm = build_endpoint(load_or_create_secret()).await?;
    let _perm_router = build_router(&perm, &app, &slot, &recv_cancel, &settings, &incoming, &mesh, &video_rx);

    // Identité ÉPHÉMÈRE : clé aléatoire en mémoire, régénérée à chaque lancement.
    let eph_ep = build_endpoint(SecretKey::generate()).await?;
    let eph_router = build_router(&eph_ep, &app, &slot, &recv_cancel, &settings, &incoming, &mesh, &video_rx);
    let eph = Arc::new(Mutex::new(Eph {
        endpoint: eph_ep,
        _router: eph_router,
    }));

    Ok(Net {
        app,
        perm,
        _perm_router,
        eph,
        slot,
        send_cancel,
        recv_cancel,
        settings,
        incoming,
        mesh,
        connecting,
        video_rx,
    })
}

/// Attache (ou remplace) le canal binaire WebView qui recevra la vidéo native.
pub fn video_attach(net: &Net, channel: tauri::ipc::Channel<tauri::ipc::InvokeResponseBody>) {
    *net.video_rx.lock().unwrap_or_else(|e| e.into_inner()) = Some(channel);
}

/// Sonde un ami par son code : tente une connexion légère (ALPN présence) avec un délai borné.
/// Renvoie true s'il est joignable (donc en ligne), false sinon.
pub async fn probe(net: &Net, id_str: &str) -> bool {
    let id: EndpointId = match id_str.trim().parse() {
        Ok(i) => i,
        Err(_) => return false,
    };
    let addr = EndpointAddr::from(id);
    // On sonde via l'identité permanente (les amis nous connaissent par elle).
    match tokio::time::timeout(
        std::time::Duration::from_secs(3),
        net.perm.connect(addr, PRESENCE_ALPN),
    )
    .await
    {
        Ok(Ok(conn)) => {
            conn.close(0u32.into(), b"bye");
            true
        }
        _ => false,
    }
}

pub async fn connect(net: &Net, input: &str) -> anyhow::Result<String> {
    let input = input.trim();
    // Accepte soit une adresse complète (JSON), soit un simple « code » (EndpointId).
    let addr: EndpointAddr = match serde_json::from_str::<EndpointAddr>(input) {
        Ok(a) => a,
        Err(_) => {
            let id: EndpointId = input
                .parse()
                .map_err(|_| anyhow::anyhow!("code ou adresse invalide"))?;
            EndpointAddr::from(id)
        }
    };
    // Auto : ami connu → identité PERMANENTE (il nous reconnaît, marche avec « amis uniquement ») ;
    // inconnu → identité ÉPHÉMÈRE (on ne révèle pas notre code permanent).
    let target = addr.id.to_string();
    let is_friend = net.settings.friends.lock().unwrap_or_else(|e| e.into_inner()).contains(&target);
    let conn = if is_friend {
        net.perm.connect(addr, ALPN).await
    } else {
        let ep = net.eph.lock().await.endpoint.clone();
        ep.connect(addr, ALPN).await
    }
    .map_err(|e| anyhow::anyhow!("connexion: {e}"))?;

    // Poignée de main applicative : on n'est « connecté » qu'une fois que le pair a
    // ACCEPTÉ (côté récepteur, l'ack n'est envoyé qu'après le clic « Accepter »).
    // Évite le faux « Connecté » suivi d'un « Déconnecté » quand le pair refuse.
    {
        let (mut s, mut r) = conn
            .open_bi()
            .await
            .map_err(|e| anyhow::anyhow!("ouverture du flux: {e}"))?;
        AsyncWriteExt::write_all(&mut s, &[KIND_HELLO])
            .await
            .map_err(|e| anyhow::anyhow!("envoi: {e}"))?;
        let _ = s.finish();
        let mut ack = [0u8; 1];
        match tokio::time::timeout(
            std::time::Duration::from_secs(50),
            AsyncReadExt::read_exact(&mut r, &mut ack),
        )
        .await
        {
            Ok(Ok(_)) if ack[0] == 1 => {}
            _ => {
                conn.close(0u32.into(), b"no-hello");
                return Err(anyhow::anyhow!("connexion refusée ou pair injoignable"));
            }
        }
    }

    let peer = conn.remote_id().to_string();
    let app2 = net.app.clone();
    let slot2 = net.slot.clone();
    let rc = net.recv_cancel.clone();
    let st = net.settings.clone();
    tokio::spawn(async move { run_conn(app2, slot2, rc, st, conn).await });
    Ok(peer)
}

/// Ferme la connexion en cours et prévient l'UI locale (le pair est prévenu par la fermeture QUIC).
pub async fn disconnect(app: &AppHandle, slot: &Slot) {
    let peer = {
        let mut g = slot.lock().await;
        g.generation += 1;
        match g.conn.take() {
            Some(c) => {
                let id = c.remote_id().to_string();
                c.close(0u32.into(), b"bye");
                Some(id)
            }
            None => None,
        }
    };
    if let Some(p) = peer {
        let _ = app.emit("ghost-disconnected", p);
    }
}

/// Transfert de fichier entrant en cours de réassemblage (plusieurs flux écrivent à leur offset).
struct Inbound {
    file: tokio::sync::Mutex<tokio::fs::File>,
    received: AtomicU64,
    cancelled: AtomicBool,
    size: u64, // GL-1 : taille déclarée/acceptée — borne les écritures des flux de données.
}
type Inbounds = Arc<StdMutex<HashMap<u64, Arc<Inbound>>>>;

/// SHA-256 d'un fichier (vérification d'intégrité après réassemblage multi-flux).
async fn sha256_file(path: &Path) -> anyhow::Result<[u8; 32]> {
    use sha2::{Digest, Sha256};
    let mut f = tokio::fs::File::open(path).await?;
    let mut h = Sha256::new();
    let mut buf = vec![0u8; 1 << 20];
    loop {
        let n = AsyncReadExt::read(&mut f, &mut buf).await?;
        if n == 0 {
            break;
        }
        h.update(&buf[..n]);
    }
    let out = h.finalize();
    let mut arr = [0u8; 32];
    arr.copy_from_slice(&out);
    Ok(arr)
}

async fn run_conn(app: AppHandle, slot: Slot, recv_cancel: Arc<AtomicBool>, settings: Settings, connection: Connection) {
    let peer = connection.remote_id().to_string();
    let mygen = {
        let mut g = slot.lock().await;
        if let Some(old) = g.conn.take() {
            old.close(0u32.into(), b"reconnect");
        }
        g.generation += 1;
        g.conn = Some(connection.clone());
        g.generation
    };
    let _ = app.emit("ghost-connected", &peer);
    let inbounds: Inbounds = Arc::new(StdMutex::new(HashMap::new()));

    loop {
        match connection.accept_bi().await {
            Ok((mut send, mut recv)) => {
                let a = app.clone();
                let cancel = recv_cancel.clone();
                let settings = settings.clone();
                let inbounds = inbounds.clone();
                tokio::spawn(async move {
                    // Premier octet : type de flux (1 = fichier, 2 = chat).
                    let mut kind = [0u8; 1];
                    if AsyncReadExt::read_exact(&mut recv, &mut kind).await.is_err() {
                        return;
                    }
                    if kind[0] == KIND_CHAT {
                        // [u16 nom_len][nom][u32 texte_len][texte]
                        let parsed: anyhow::Result<(String, String)> = async {
                            let mut l2 = [0u8; 2];
                            AsyncReadExt::read_exact(&mut recv, &mut l2).await?;
                            let nlen = u16::from_be_bytes(l2) as usize;
                            let mut nbuf = vec![0u8; nlen];
                            AsyncReadExt::read_exact(&mut recv, &mut nbuf).await?;
                            let mut l4 = [0u8; 4];
                            AsyncReadExt::read_exact(&mut recv, &mut l4).await?;
                            let len = u32::from_be_bytes(l4) as usize;
                            if len > 256 * 1024 {
                                anyhow::bail!("message trop long");
                            }
                            let mut tbuf = vec![0u8; len];
                            AsyncReadExt::read_exact(&mut recv, &mut tbuf).await?;
                            Ok((
                                String::from_utf8_lossy(&nbuf).to_string(),
                                String::from_utf8_lossy(&tbuf).to_string(),
                            ))
                        }
                        .await;
                        if let Ok((name, text)) = parsed {
                            let _ = a.emit("ghost-chat", serde_json::json!({ "name": name, "text": text }));
                        }
                        return;
                    }
                    if kind[0] == KIND_IMG {
                        // [u16 author][u16 name][u16 mime][u32 data]
                        let parsed: anyhow::Result<(String, String, String, Vec<u8>)> = async {
                            let author = read_lp16(&mut recv).await?;
                            let name = read_lp16(&mut recv).await?;
                            let mime = read_lp16(&mut recv).await?;
                            if !mime_ok(&mime) {
                                anyhow::bail!("mime refusé");
                            }
                            let data = read_lp32_bytes(&mut recv, MAX_IMG_WIRE).await?;
                            Ok((author, name, mime, data))
                        }
                        .await;
                        if let Ok((author, name, mime, data)) = parsed {
                            use base64::Engine;
                            let b64 = base64::engine::general_purpose::STANDARD.encode(&data);
                            let _ = a.emit("ghost-chat-img", serde_json::json!({ "author": author, "name": name, "mime": mime, "dataB64": b64 }));
                        }
                        return;
                    }
                    if kind[0] == KIND_FREQ || kind[0] == KIND_FACCEPT {
                        // [u16 nom_len][nom] puis (depuis 0.14.1) [u16 code_len][code permanent]
                        let (name, code) = async {
                            let mut l2 = [0u8; 2];
                            AsyncReadExt::read_exact(&mut recv, &mut l2).await?;
                            let nlen = u16::from_be_bytes(l2) as usize;
                            let mut nbuf = vec![0u8; nlen];
                            AsyncReadExt::read_exact(&mut recv, &mut nbuf).await?;
                            let name = String::from_utf8_lossy(&nbuf).to_string();
                            // Code permanent — best-effort (compat avec un ancien pair sans code).
                            let mut code = String::new();
                            let mut c2 = [0u8; 2];
                            if AsyncReadExt::read_exact(&mut recv, &mut c2).await.is_ok() {
                                let clen = u16::from_be_bytes(c2) as usize;
                                let mut cbuf = vec![0u8; clen];
                                if AsyncReadExt::read_exact(&mut recv, &mut cbuf).await.is_ok() {
                                    code = String::from_utf8_lossy(&cbuf).to_string();
                                }
                            }
                            anyhow::Ok::<(String, String)>((name, code))
                        }
                        .await
                        .unwrap_or_default();
                        let ev = if kind[0] == KIND_FREQ { "ghost-freq" } else { "ghost-faccept" };
                        let _ = a.emit(ev, serde_json::json!({ "name": name, "code": code }));
                        return;
                    }
                    if kind[0] == KIND_CALL_START || kind[0] == KIND_CALL_STOP {
                        let ev = if kind[0] == KIND_CALL_START { "ghost-call-start" } else { "ghost-call-stop" };
                        let _ = a.emit(ev, serde_json::json!({}));
                        return;
                    }
                    if kind[0] == KIND_HELLO {
                        // Poignée de main : on a accepté la connexion → on confirme à l'initiateur.
                        let _ = AsyncWriteExt::write_all(&mut send, &[1u8]).await;
                        let _ = send.finish();
                        return;
                    }
                    if kind[0] == KIND_FDATA {
                        // Flux de données d'un transfert : [u64 id][u64 offset][u64 len] puis les octets.
                        let hdr: anyhow::Result<(u64, u64, u64)> = async {
                            let mut b = [0u8; 8];
                            AsyncReadExt::read_exact(&mut recv, &mut b).await?;
                            let id = u64::from_be_bytes(b);
                            AsyncReadExt::read_exact(&mut recv, &mut b).await?;
                            let offset = u64::from_be_bytes(b);
                            AsyncReadExt::read_exact(&mut recv, &mut b).await?;
                            let len = u64::from_be_bytes(b);
                            Ok((id, offset, len))
                        }
                        .await;
                        let (id, offset, len) = match hdr {
                            Ok(v) => v,
                            Err(_) => return,
                        };
                        // Attendre que le flux de contrôle ait enregistré ce transfert (course réseau).
                        let mut inb = None;
                        for _ in 0..200 {
                            if let Some(x) = inbounds.lock().unwrap_or_else(|e| e.into_inner()).get(&id) {
                                inb = Some(x.clone());
                                break;
                            }
                            tokio::time::sleep(std::time::Duration::from_millis(50)).await;
                        }
                        let inb = match inb {
                            Some(x) => x,
                            None => return,
                        };
                        // GL-1 : refuser tout flux dont la plage [offset, offset+len) sort de la
                        // taille acceptée — évite l'écriture non bornée (DoS disque) et l'offset arbitraire.
                        match offset.checked_add(len) {
                            Some(end) if end <= inb.size => {}
                            _ => return,
                        }
                        let mut buf = vec![0u8; CHUNK];
                        let mut pos = offset;
                        let mut remaining = len;
                        while remaining > 0 {
                            if inb.cancelled.load(Ordering::SeqCst) {
                                return;
                            }
                            let want = std::cmp::min(CHUNK as u64, remaining) as usize;
                            if AsyncReadExt::read_exact(&mut recv, &mut buf[..want]).await.is_err() {
                                return;
                            }
                            {
                                use tokio::io::AsyncSeekExt;
                                let mut f = inb.file.lock().await;
                                if f.seek(std::io::SeekFrom::Start(pos)).await.is_err()
                                    || AsyncWriteExt::write_all(&mut *f, &buf[..want]).await.is_err()
                                {
                                    return;
                                }
                            }
                            pos += want as u64;
                            remaining -= want as u64;
                            inb.received.fetch_add(want as u64, Ordering::SeqCst);
                        }
                        return;
                    }
                    if kind[0] != KIND_FILE {
                        return; // type de flux inconnu : on ignore (compat ascendante + sécurité)
                    }

                    // KIND_FILE (flux de contrôle d'un transfert multi-flux) :
                    // [u64 id][u16 nom_len][nom][u64 taille][32 hash][u8 nflux]
                    cancel.store(false, Ordering::SeqCst);
                    let header: anyhow::Result<(u64, String, u64, [u8; 32])> = async {
                        let mut b8 = [0u8; 8];
                        AsyncReadExt::read_exact(&mut recv, &mut b8).await?;
                        let id = u64::from_be_bytes(b8);
                        let mut l2 = [0u8; 2];
                        AsyncReadExt::read_exact(&mut recv, &mut l2).await?;
                        let nlen = u16::from_be_bytes(l2) as usize;
                        let mut nbuf = vec![0u8; nlen];
                        AsyncReadExt::read_exact(&mut recv, &mut nbuf).await?;
                        AsyncReadExt::read_exact(&mut recv, &mut b8).await?;
                        let size = u64::from_be_bytes(b8);
                        let mut hash = [0u8; 32];
                        AsyncReadExt::read_exact(&mut recv, &mut hash).await?;
                        let mut nflux = [0u8; 1];
                        AsyncReadExt::read_exact(&mut recv, &mut nflux).await?;
                        Ok((id, sanitize(&String::from_utf8_lossy(&nbuf)), size, hash))
                    }
                    .await;
                    let (id, name, size, hash) = match header {
                        Ok(v) => v,
                        Err(_) => return,
                    };

                    // SEC-2 : refuser d'emblee si l'espace disque libre est insuffisant (marge 64 Mo).
                    // On répond [2] (et non [0]) pour que l'émetteur affiche la VRAIE raison
                    // (« espace disque insuffisant ») au lieu d'un « refusé » générique. Rétro-compatible :
                    // un ancien émetteur traite [2] comme « != 1 » = refus.
                    if let Some(free) = free_space(&settings.recv_dir()) {
                        if size > free.saturating_sub(64 * 1024 * 1024) {
                            let _ = AsyncWriteExt::write_all(&mut send, &[2u8]).await;
                            let _ = send.finish();
                            let _ = a.emit("ghost-recv-nospace", serde_json::json!({ "name": name, "size": size, "free": free }));
                            return;
                        }
                    }

                    // Demander l'autorisation AVANT de recevoir.
                    let offer_id = settings.file_counter.fetch_add(1, Ordering::SeqCst);
                    let (otx, orx) = tokio::sync::oneshot::channel::<bool>();
                    settings.file_pending.lock().unwrap_or_else(|e| e.into_inner()).insert(offer_id, otx);
                    let _ = a.emit("ghost-recv-offer", serde_json::json!({ "id": offer_id, "name": name, "size": size }));
                    let accepted = matches!(
                        tokio::time::timeout(std::time::Duration::from_secs(120), orx).await,
                        Ok(Ok(true))
                    );
                    settings.file_pending.lock().unwrap_or_else(|e| e.into_inner()).remove(&offer_id);
                    if !accepted {
                        let _ = AsyncWriteExt::write_all(&mut send, &[0u8]).await;
                        let _ = send.finish();
                        let _ = a.emit("ghost-recv-rejected", serde_json::json!({ "id": offer_id, "name": name }));
                        return;
                    }
                    let _ = AsyncWriteExt::write_all(&mut send, &[1u8]).await;

                    let dir = settings.recv_dir();
                    let dest = unique_path(&dir, &name);
                    let created = async {
                        let f = tokio::fs::File::create(&dest).await?;
                        f.set_len(size).await?; // pré-allouer : les flux écrivent à leur offset
                        anyhow::Ok(f)
                    }
                    .await;
                    let file = match created {
                        Ok(f) => f,
                        Err(_) => return,
                    };
                    let inb = Arc::new(Inbound {
                        file: tokio::sync::Mutex::new(file),
                        received: AtomicU64::new(0),
                        cancelled: AtomicBool::new(false),
                        size,
                    });
                    inbounds.lock().unwrap_or_else(|e| e.into_inner()).insert(id, inb.clone());
                    let _ = a.emit("ghost-recv-start", serde_json::json!({ "name": name, "size": size }));
                    let _ = a.emit("ghost-recv-progress", serde_json::json!({ "name": name, "received": 0u64, "size": size }));

                    // Attendre le réassemblage complet (les flux KIND_FDATA remplissent `received`).
                    let mut last_emit = std::time::Instant::now();
                    let mut last_got = 0u64;
                    let mut last_progress = std::time::Instant::now();
                    let mut done = false;
                    loop {
                        if cancel.load(Ordering::SeqCst) {
                            inb.cancelled.store(true, Ordering::SeqCst);
                            break;
                        }
                        let got = inb.received.load(Ordering::SeqCst);
                        if got >= size {
                            done = true;
                            break;
                        }
                        if got != last_got {
                            last_got = got;
                            last_progress = std::time::Instant::now();
                        }
                        // GL-LF-1 : seuil large (5 min) mesuré en temps réel (Instant), pour ne pas
                        // abandonner un gros transfert (100 Go+) sur une pause réseau passagère.
                        if last_progress.elapsed().as_secs() >= 300 {
                            break; // 5 min sans le moindre octet → abandon
                        }
                        if last_emit.elapsed().as_millis() >= 100 {
                            let _ = a.emit("ghost-recv-progress", serde_json::json!({ "name": name, "received": got, "size": size }));
                            last_emit = std::time::Instant::now();
                        }
                        tokio::time::sleep(std::time::Duration::from_millis(60)).await;
                    }
                    inbounds.lock().unwrap_or_else(|e| e.into_inner()).remove(&id);
                    {
                        let mut f = inb.file.lock().await;
                        let _ = AsyncWriteExt::flush(&mut *f).await;
                    }
                    let ok_hash = done && sha256_file(&dest).await.map(|h| h == hash).unwrap_or(false);
                    if ok_hash {
                        let _ = AsyncWriteExt::write_all(&mut send, b"ok").await;
                        let _ = send.finish();
                        let _ = a.emit("ghost-recv-done", serde_json::json!({ "name": name, "size": size, "path": dest.to_string_lossy() }));
                    } else {
                        let _ = tokio::fs::remove_file(&dest).await;
                        let ev = if done { "ghost-recv-corrupt" } else { "ghost-recv-cancel" };
                        let _ = a.emit(ev, serde_json::json!({ "name": name }));
                    }
                });
            }
            Err(_) => break,
        }
    }

    let mut g = slot.lock().await;
    if g.generation == mygen {
        g.conn = None;
        drop(g);
        let _ = app.emit("ghost-disconnected", &peer);
    }
}

/// Envoie un fichier sur la connexion ouverte. Annulable via `send_cancel`.
pub async fn send_file(
    app: &AppHandle,
    slot: &Slot,
    send_cancel: &Arc<AtomicBool>,
    path: &str,
) -> anyhow::Result<String> {
    send_cancel.store(false, Ordering::SeqCst);
    let conn = current(slot)
        .await
        .ok_or_else(|| anyhow::anyhow!("pas connecté à un pair"))?;
    // Le NOM affiché vient du fichier original ; les octets lus (taille, hash,
    // tranches) viennent de la copie NETTOYÉE de ses métadonnées (meta.rs).
    let name = Path::new(path)
        .file_name()
        .and_then(|s| s.to_str())
        .unwrap_or("fichier")
        .to_string();
    let read_path = prepare_meta(app, path, &name).await;
    let p = Path::new(&read_path);
    let size = tokio::fs::metadata(p)
        .await
        .map_err(|e| anyhow::anyhow!("fichier introuvable: {e}"))?
        .len();

    // Empreinte SHA-256 du fichier : le pair vérifiera l'intégrité après réassemblage.
    let hash = sha256_file(p)
        .await
        .map_err(|e| anyhow::anyhow!("lecture (hash): {e}"))?;
    let id = FILE_SEQ.fetch_add(1, Ordering::SeqCst);
    let nstreams = STREAMS.load(Ordering::SeqCst).clamp(1, 8);

    // Flux de CONTRÔLE : entête + accord du pair. Les octets passent par les flux de données.
    let (mut send, mut recv) = conn
        .open_bi()
        .await
        .map_err(|e| anyhow::anyhow!("ouverture du flux: {e}"))?;

    let mut head = Vec::with_capacity(1 + 8 + 2 + name.len() + 8 + 32 + 1);
    head.push(KIND_FILE);
    head.extend_from_slice(&id.to_be_bytes());
    let nb = name.as_bytes();
    head.extend_from_slice(&(nb.len() as u16).to_be_bytes());
    head.extend_from_slice(nb);
    head.extend_from_slice(&size.to_be_bytes());
    head.extend_from_slice(&hash);
    head.push(nstreams as u8);
    AsyncWriteExt::write_all(&mut send, &head)
        .await
        .map_err(|e| anyhow::anyhow!("envoi: {e}"))?;

    // Attendre que le pair accepte (ou refuse) le fichier avant d'envoyer les octets.
    let _ = app.emit("ghost-send-await", serde_json::json!({ "name": name }));
    let mut decision = [0u8; 1];
    AsyncReadExt::read_exact(&mut recv, &mut decision)
        .await
        .map_err(|e| anyhow::anyhow!("le pair n'a pas répondu: {e}"))?;
    if decision[0] != 1 {
        // [2] = SEC-2 espace disque insuffisant chez le pair ; sinon refus simple.
        return Err(if decision[0] == 2 {
            anyhow::anyhow!("espace disque insuffisant chez le pair pour ce fichier")
        } else {
            anyhow::anyhow!("refusé par le pair")
        });
    }

    let _ = app.emit("ghost-send-progress", serde_json::json!({ "name": name, "sent": 0u64, "size": size }));

    // Découper le fichier en NSTREAMS tranches contiguës, une par flux parallèle.
    let part = if size == 0 { 0 } else { (size + nstreams - 1) / nstreams };
    let sent = Arc::new(AtomicU64::new(0));
    let mut tasks = Vec::new();
    for i in 0..nstreams {
        let offset = i * part;
        if offset >= size {
            break; // fichier plus petit que NSTREAMS tranches
        }
        let len = std::cmp::min(part, size - offset);
        let task = send_one_part(
            conn.clone(),
            id,
            offset,
            len,
            p.to_path_buf(),
            size,
            send_cancel.clone(),
            app.clone(),
            name.clone(),
            sent.clone(),
        );
        tasks.push(tokio::spawn(task));
    }
    // Attendre tous les flux ; la première erreur fait échouer l'envoi.
    let mut first_err: Option<anyhow::Error> = None;
    for t in tasks {
        match t.await {
            Ok(Ok(())) => {}
            Ok(Err(e)) => {
                if first_err.is_none() {
                    first_err = Some(e);
                }
            }
            Err(e) => {
                if first_err.is_none() {
                    first_err = Some(anyhow::anyhow!("tâche d'envoi: {e}"));
                }
            }
        }
    }
    if let Some(e) = first_err {
        let _ = send.reset(0u32.into());
        return Err(e);
    }
    let _ = send.finish();

    // Accusé final du pair : "ok" si l'intégrité est vérifiée.
    let mut ack = [0u8; 2];
    match AsyncReadExt::read_exact(&mut recv, &mut ack).await {
        Ok(_) if &ack == b"ok" => Ok(name),
        _ => Err(anyhow::anyhow!(
            "le pair a rejeté le fichier (intégrité non vérifiée ou transfert interrompu)"
        )),
    }
}

/// Envoie une tranche [offset, offset+len) du fichier sur son propre flux QUIC.
/// En-tête du flux : [KIND_FDATA][u64 id][u64 offset][u64 len] puis les octets.
#[allow(clippy::too_many_arguments)]
async fn send_one_part(
    conn: Connection,
    id: u64,
    offset: u64,
    len: u64,
    path: PathBuf,
    total: u64,
    send_cancel: Arc<AtomicBool>,
    app: AppHandle,
    name: String,
    sent: Arc<AtomicU64>,
) -> anyhow::Result<()> {
    use tokio::io::AsyncSeekExt;
    let (mut send, _recv) = conn
        .open_bi()
        .await
        .map_err(|e| anyhow::anyhow!("ouverture du flux: {e}"))?;
    let mut hdr = Vec::with_capacity(25);
    hdr.push(KIND_FDATA);
    hdr.extend_from_slice(&id.to_be_bytes());
    hdr.extend_from_slice(&offset.to_be_bytes());
    hdr.extend_from_slice(&len.to_be_bytes());
    AsyncWriteExt::write_all(&mut send, &hdr)
        .await
        .map_err(|e| anyhow::anyhow!("envoi entête: {e}"))?;

    let mut file = tokio::fs::File::open(&path)
        .await
        .map_err(|e| anyhow::anyhow!("ouverture: {e}"))?;
    file.seek(std::io::SeekFrom::Start(offset))
        .await
        .map_err(|e| anyhow::anyhow!("seek: {e}"))?;

    let mut buf = vec![0u8; CHUNK];
    let mut remaining = len;
    let mut last_emit = std::time::Instant::now();
    while remaining > 0 {
        if send_cancel.load(Ordering::SeqCst) {
            let _ = send.reset(0u32.into());
            return Err(anyhow::anyhow!("annulé"));
        }
        let want = std::cmp::min(CHUNK as u64, remaining) as usize;
        AsyncReadExt::read_exact(&mut file, &mut buf[..want])
            .await
            .map_err(|e| anyhow::anyhow!("lecture: {e}"))?;
        AsyncWriteExt::write_all(&mut send, &buf[..want])
            .await
            .map_err(|e| anyhow::anyhow!("envoi: {e}"))?;
        remaining -= want as u64;
        let s = sent.fetch_add(want as u64, Ordering::SeqCst) + want as u64;
        if last_emit.elapsed().as_millis() >= 100 {
            let _ = app.emit("ghost-send-progress", serde_json::json!({ "name": name, "sent": s, "size": total }));
            last_emit = std::time::Instant::now();
        }
    }
    send.finish().map_err(|e| anyhow::anyhow!("finish: {e}"))?;
    Ok(())
}

/// Envoie un message de chat (avec le nom d'affichage) sur la connexion ouverte.
pub async fn send_chat(slot: &Slot, name: &str, text: &str) -> anyhow::Result<()> {
    let conn = current(slot)
        .await
        .ok_or_else(|| anyhow::anyhow!("pas connecté à un pair"))?;
    let (mut send, _recv) = conn
        .open_bi()
        .await
        .map_err(|e| anyhow::anyhow!("ouverture du flux: {e}"))?;
    AsyncWriteExt::write_all(&mut send, &[KIND_CHAT])
        .await
        .map_err(|e| anyhow::anyhow!("envoi: {e}"))?;
    let nb = name.as_bytes();
    AsyncWriteExt::write_all(&mut send, &(nb.len() as u16).to_be_bytes())
        .await
        .map_err(|e| anyhow::anyhow!("envoi: {e}"))?;
    AsyncWriteExt::write_all(&mut send, nb)
        .await
        .map_err(|e| anyhow::anyhow!("envoi: {e}"))?;
    let tb = text.as_bytes();
    AsyncWriteExt::write_all(&mut send, &(tb.len() as u32).to_be_bytes())
        .await
        .map_err(|e| anyhow::anyhow!("envoi: {e}"))?;
    AsyncWriteExt::write_all(&mut send, tb)
        .await
        .map_err(|e| anyhow::anyhow!("envoi: {e}"))?;
    send.finish().map_err(|e| anyhow::anyhow!("finish: {e}"))?;
    Ok(())
}

/// Envoie une image inline (octets) sur la connexion 1-à-1 ouverte.
pub async fn send_img(slot: &Slot, author: &str, name: &str, mime: &str, data: &[u8]) -> anyhow::Result<()> {
    let conn = current(slot).await.ok_or_else(|| anyhow::anyhow!("pas connecté"))?;
    let (mut send, _r) = conn.open_bi().await.map_err(|e| anyhow::anyhow!("flux: {e}"))?;
    send.write_all(&[KIND_IMG]).await?;
    write_lp16(&mut send, author).await?;
    write_lp16(&mut send, name).await?;
    write_lp16(&mut send, mime).await?;
    write_lp32_bytes(&mut send, data).await?;
    send.finish().map_err(|e| anyhow::anyhow!("finish: {e}"))?;
    Ok(())
}

/// Envoie un message « type + nom d'affichage » (demandes d'ami) sur la connexion ouverte.
async fn send_named(slot: &Slot, kind: u8, name: &str, code: &str) -> anyhow::Result<()> {
    let conn = current(slot)
        .await
        .ok_or_else(|| anyhow::anyhow!("pas connecté à un pair"))?;
    let (mut send, _recv) = conn
        .open_bi()
        .await
        .map_err(|e| anyhow::anyhow!("ouverture du flux: {e}"))?;
    AsyncWriteExt::write_all(&mut send, &[kind])
        .await
        .map_err(|e| anyhow::anyhow!("envoi: {e}"))?;
    let nb = name.as_bytes();
    AsyncWriteExt::write_all(&mut send, &(nb.len() as u16).to_be_bytes())
        .await
        .map_err(|e| anyhow::anyhow!("envoi: {e}"))?;
    AsyncWriteExt::write_all(&mut send, nb)
        .await
        .map_err(|e| anyhow::anyhow!("envoi: {e}"))?;
    // Code PERMANENT de l'expéditeur : l'ami enregistre ce code (stable), pas l'éphémère.
    let cb = code.as_bytes();
    AsyncWriteExt::write_all(&mut send, &(cb.len() as u16).to_be_bytes())
        .await
        .map_err(|e| anyhow::anyhow!("envoi: {e}"))?;
    AsyncWriteExt::write_all(&mut send, cb)
        .await
        .map_err(|e| anyhow::anyhow!("envoi: {e}"))?;
    send.finish().map_err(|e| anyhow::anyhow!("finish: {e}"))?;
    Ok(())
}

/// Demande d'ami (au pair connecté) : nom d'affichage + code permanent.
pub async fn send_freq(slot: &Slot, name: &str, code: &str) -> anyhow::Result<()> {
    send_named(slot, KIND_FREQ, name, code).await
}

/// Acceptation d'une demande d'ami : nom d'affichage + code permanent.
pub async fn send_faccept(slot: &Slot, name: &str, code: &str) -> anyhow::Result<()> {
    send_named(slot, KIND_FACCEPT, name, code).await
}

/// Envoie un simple signal (un octet) — utilisé pour la signalisation d'appel vocal.
async fn send_kind_only(slot: &Slot, kind: u8) -> anyhow::Result<()> {
    let conn = current(slot)
        .await
        .ok_or_else(|| anyhow::anyhow!("pas connecté à un pair"))?;
    let (mut send, _recv) = conn
        .open_bi()
        .await
        .map_err(|e| anyhow::anyhow!("ouverture du flux: {e}"))?;
    AsyncWriteExt::write_all(&mut send, &[kind])
        .await
        .map_err(|e| anyhow::anyhow!("envoi: {e}"))?;
    send.finish().map_err(|e| anyhow::anyhow!("finish: {e}"))?;
    Ok(())
}

/// Signale au pair le début d'un appel vocal.
pub async fn send_call_start(slot: &Slot) -> anyhow::Result<()> {
    send_kind_only(slot, KIND_CALL_START).await
}

/// Signale au pair la fin d'un appel vocal.
pub async fn send_call_stop(slot: &Slot) -> anyhow::Result<()> {
    send_kind_only(slot, KIND_CALL_STOP).await
}

/// Empreinte lisible d'un code (8 premiers octets de SHA-256, en 4 groupes hex).
pub fn fingerprint(code: &str) -> String {
    use sha2::{Digest, Sha256};
    let mut h = Sha256::new();
    h.update(code.trim().to_lowercase().as_bytes());
    let out = h.finalize();
    let hex: String = out.iter().take(8).map(|b| format!("{:02X}", b)).collect();
    hex.as_bytes()
        .chunks(4)
        .filter_map(|c| std::str::from_utf8(c).ok())
        .collect::<Vec<_>>()
        .join("-")
}

fn sanitize(name: &str) -> String {
    let base = Path::new(name)
        .file_name()
        .and_then(|s| s.to_str())
        .unwrap_or("fichier");
    let cleaned: String = base
        .chars()
        .filter(|c| !c.is_control() && !matches!(c, '/' | '\\' | ':' | '*' | '?' | '"' | '<' | '>' | '|'))
        .collect();
    // Windows ignore les points/espaces de fin → on les retire (collisions / noms pièges).
    let cleaned = cleaned.trim().trim_end_matches(['.', ' ']).to_string();
    if cleaned.is_empty() {
        "fichier".to_string()
    } else {
        cleaned
    }
}

fn unique_path(dir: &Path, name: &str) -> PathBuf {
    let p = dir.join(name);
    if !p.exists() {
        return p;
    }
    let stem = Path::new(name)
        .file_stem()
        .and_then(|s| s.to_str())
        .unwrap_or("fichier")
        .to_string();
    let ext = Path::new(name)
        .extension()
        .and_then(|s| s.to_str())
        .map(|e| format!(".{e}"))
        .unwrap_or_default();
    for i in 1..10000 {
        let cand = dir.join(format!("{stem} ({i}){ext}"));
        if !cand.exists() {
            return cand;
        }
    }
    dir.join(name)
}
