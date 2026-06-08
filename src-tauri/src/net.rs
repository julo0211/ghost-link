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

#[derive(Default)]
pub struct ConnState {
    generation: u64,
    conn: Option<Connection>,
}
pub type Slot = Arc<Mutex<ConnState>>;

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
        self.friends.lock().unwrap().contains(peer_id)
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
    if let Some(tx) = incoming.pending.lock().unwrap().remove(&id) {
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
        // Filtre « amis uniquement » : on refuse les pairs inconnus avant tout.
        if !self.settings.allows(&peer) {
            connection.close(0u32.into(), b"not-a-friend");
            let _ = self.app.emit("ghost-refused", &peer);
            return Ok(());
        }
        // Demander l'autorisation à l'utilisateur (toujours), avec délai de 45 s.
        let id = self.incoming.counter.fetch_add(1, Ordering::SeqCst);
        let (tx, rx) = tokio::sync::oneshot::channel::<bool>();
        self.incoming.pending.lock().unwrap().insert(id, tx);
        let _ = self
            .app
            .emit("ghost-incoming", serde_json::json!({ "id": id, "peer": peer }));
        let accepted = matches!(
            tokio::time::timeout(std::time::Duration::from_secs(45), rx).await,
            Ok(Ok(true))
        );
        self.incoming.pending.lock().unwrap().remove(&id);
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
}

/// Identité éphémère : clé aléatoire en mémoire, remplaçable à chaud (rotation).
pub struct Eph {
    endpoint: Endpoint,
    _router: Router,
}

/// Emplacement du fichier d'identité (clé secrète ed25519), dans le dossier de données de l'app.
fn identity_path() -> PathBuf {
    let base = dirs::data_dir().unwrap_or_else(std::env::temp_dir);
    base.join("ghost-link").join("identity.key")
}

/// Charge la clé secrète persistante, ou en crée une (et la sauvegarde) au premier lancement.
/// C'est elle qui fixe l'identité du nœud — donc le « code ami » — de façon stable dans le temps.
fn load_or_create_secret() -> SecretKey {
    let path = identity_path();
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
    *settings.download_dir.lock().unwrap() = if p.is_empty() { None } else { Some(PathBuf::from(p)) };
    save_download_dir(p);
}
pub fn get_download_dir(settings: &Settings) -> String {
    settings.recv_dir().to_string_lossy().to_string()
}
pub fn set_only_friends(settings: &Settings, on: bool) {
    settings.only_friends.store(on, Ordering::SeqCst);
}
pub fn set_friends(settings: &Settings, codes: Vec<String>) {
    let mut s = settings.friends.lock().unwrap();
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
    if let Some(tx) = settings.file_pending.lock().unwrap().remove(&id) {
        let _ = tx.send(accept);
    }
}

/// Construit un endpoint iroh (fenêtres QUIC élargies pour viser ~1 Gbps).
async fn build_endpoint(secret: SecretKey) -> anyhow::Result<Endpoint> {
    let transport = iroh::endpoint::QuicTransportConfig::builder()
        .stream_receive_window(iroh::endpoint::VarInt::from_u32(16 * 1024 * 1024))
        .receive_window(iroh::endpoint::VarInt::from_u32(64 * 1024 * 1024))
        .send_window(64 * 1024 * 1024)
        .build();
    Endpoint::builder(presets::N0)
        .secret_key(secret)
        .transport_config(transport)
        .bind()
        .await
        .map_err(|e| anyhow::anyhow!("bind iroh: {e}"))
}

/// Démarre le Router (protocoles fichier/chat/voix + présence) sur un endpoint.
fn build_router(
    endpoint: &Endpoint,
    app: &AppHandle,
    slot: &Slot,
    recv_cancel: &Arc<AtomicBool>,
    settings: &Settings,
    incoming: &Incoming,
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
        .accept(PRESENCE_ALPN, Presence)
        .spawn()
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
        *settings.download_dir.lock().unwrap() = Some(dir);
    }
    let incoming = Incoming::default();

    // Identité PERMANENTE : clé persistante = code ami stable.
    let perm = build_endpoint(load_or_create_secret()).await?;
    let _perm_router = build_router(&perm, &app, &slot, &recv_cancel, &settings, &incoming);

    // Identité ÉPHÉMÈRE : clé aléatoire en mémoire, régénérée à chaque lancement.
    let eph_ep = build_endpoint(SecretKey::generate()).await?;
    let eph_router = build_router(&eph_ep, &app, &slot, &recv_cancel, &settings, &incoming);
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
    })
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
        std::time::Duration::from_secs(5),
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
    let is_friend = net.settings.friends.lock().unwrap().contains(&target);
    let conn = if is_friend {
        net.perm.connect(addr, ALPN).await
    } else {
        let ep = net.eph.lock().await.endpoint.clone();
        ep.connect(addr, ALPN).await
    }
    .map_err(|e| anyhow::anyhow!("connexion: {e}"))?;
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

    loop {
        match connection.accept_bi().await {
            Ok((mut send, mut recv)) => {
                let a = app.clone();
                let cancel = recv_cancel.clone();
                let settings = settings.clone();
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

                    // KIND_FILE : réception de fichier.
                    cancel.store(false, Ordering::SeqCst);

                    // En-tête
                    let header: anyhow::Result<(String, u64)> = async {
                        let mut l2 = [0u8; 2];
                        AsyncReadExt::read_exact(&mut recv, &mut l2).await?;
                        let nlen = u16::from_be_bytes(l2) as usize;
                        let mut nbuf = vec![0u8; nlen];
                        AsyncReadExt::read_exact(&mut recv, &mut nbuf).await?;
                        let mut s8 = [0u8; 8];
                        AsyncReadExt::read_exact(&mut recv, &mut s8).await?;
                        Ok((sanitize(&String::from_utf8_lossy(&nbuf)), u64::from_be_bytes(s8)))
                    }
                    .await;
                    let (name, size) = match header {
                        Ok(v) => v,
                        Err(_) => return,
                    };

                    // Demander l'autorisation AVANT de recevoir le fichier.
                    let offer_id = settings.file_counter.fetch_add(1, Ordering::SeqCst);
                    let (otx, orx) = tokio::sync::oneshot::channel::<bool>();
                    settings.file_pending.lock().unwrap().insert(offer_id, otx);
                    let _ = a.emit("ghost-recv-offer", serde_json::json!({ "id": offer_id, "name": name, "size": size }));
                    let accepted = matches!(
                        tokio::time::timeout(std::time::Duration::from_secs(120), orx).await,
                        Ok(Ok(true))
                    );
                    settings.file_pending.lock().unwrap().remove(&offer_id);
                    if !accepted {
                        let _ = AsyncWriteExt::write_all(&mut send, &[0u8]).await; // refus
                        let _ = send.finish();
                        let _ = recv.stop(0u32.into());
                        let _ = a.emit("ghost-recv-rejected", serde_json::json!({ "id": offer_id, "name": name }));
                        return;
                    }
                    let _ = AsyncWriteExt::write_all(&mut send, &[1u8]).await; // accepté

                    let dir = settings.recv_dir();
                    let dest = unique_path(&dir, &name);
                    let _ = a.emit("ghost-recv-start", serde_json::json!({ "name": name, "size": size }));

                    // Corps (Ok(true) = terminé, Ok(false) = annulé, Err = flux coupé)
                    let body: anyhow::Result<bool> = async {
                        let mut file = tokio::io::BufWriter::with_capacity(
                            1 << 20,
                            tokio::fs::File::create(&dest).await?,
                        );
                        let mut buf = vec![0u8; CHUNK];
                        let mut got: u64 = 0;
                        let mut last_emit = std::time::Instant::now();
                        let _ = a.emit("ghost-recv-progress", serde_json::json!({ "name": name, "received": 0u64, "size": size }));
                        while got < size {
                            if cancel.load(Ordering::SeqCst) {
                                return Ok(false);
                            }
                            let want = std::cmp::min(CHUNK as u64, size - got) as usize;
                            AsyncReadExt::read_exact(&mut recv, &mut buf[..want]).await?;
                            AsyncWriteExt::write_all(&mut file, &buf[..want]).await?;
                            got += want as u64;
                            // Émettre la progression au plus 10×/s pour ne pas saturer l'IPC.
                            if last_emit.elapsed().as_millis() >= 100 {
                                let _ = a.emit("ghost-recv-progress", serde_json::json!({ "name": name, "received": got, "size": size }));
                                last_emit = std::time::Instant::now();
                            }
                        }
                        AsyncWriteExt::flush(&mut file).await?;
                        Ok(true)
                    }
                    .await;

                    match body {
                        Ok(true) => {
                            let _ = AsyncWriteExt::write_all(&mut send, b"ok").await;
                            let _ = send.finish();
                            let _ = a.emit("ghost-recv-done", serde_json::json!({ "name": name, "size": size, "path": dest.to_string_lossy() }));
                        }
                        _ => {
                            let _ = recv.stop(0u32.into());
                            let _ = tokio::fs::remove_file(&dest).await;
                            let _ = a.emit("ghost-recv-cancel", serde_json::json!({ "name": name }));
                        }
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
    let p = Path::new(path);
    let name = p
        .file_name()
        .and_then(|s| s.to_str())
        .unwrap_or("fichier")
        .to_string();
    let size = tokio::fs::metadata(p)
        .await
        .map_err(|e| anyhow::anyhow!("fichier introuvable: {e}"))?
        .len();

    let (mut send, mut recv) = conn
        .open_bi()
        .await
        .map_err(|e| anyhow::anyhow!("ouverture du flux: {e}"))?;

    AsyncWriteExt::write_all(&mut send, &[KIND_FILE])
        .await
        .map_err(|e| anyhow::anyhow!("envoi: {e}"))?;
    let nb = name.as_bytes();
    AsyncWriteExt::write_all(&mut send, &(nb.len() as u16).to_be_bytes())
        .await
        .map_err(|e| anyhow::anyhow!("envoi: {e}"))?;
    AsyncWriteExt::write_all(&mut send, nb).await.map_err(|e| anyhow::anyhow!("envoi: {e}"))?;
    AsyncWriteExt::write_all(&mut send, &size.to_be_bytes())
        .await
        .map_err(|e| anyhow::anyhow!("envoi: {e}"))?;

    // Attendre que le pair accepte (ou refuse) le fichier avant d'envoyer les octets.
    let mut decision = [0u8; 1];
    AsyncReadExt::read_exact(&mut recv, &mut decision)
        .await
        .map_err(|e| anyhow::anyhow!("le pair n'a pas répondu: {e}"))?;
    if decision[0] != 1 {
        return Err(anyhow::anyhow!("refusé par le pair"));
    }

    let mut file = tokio::fs::File::open(p)
        .await
        .map_err(|e| anyhow::anyhow!("ouverture: {e}"))?;
    let mut buf = vec![0u8; CHUNK];
    let mut sent: u64 = 0;
    let mut last_emit = std::time::Instant::now();
    let _ = app.emit("ghost-send-progress", serde_json::json!({ "name": name, "sent": 0u64, "size": size }));
    loop {
        if send_cancel.load(Ordering::SeqCst) {
            let _ = send.reset(0u32.into());
            return Err(anyhow::anyhow!("annulé"));
        }
        let n = AsyncReadExt::read(&mut file, &mut buf)
            .await
            .map_err(|e| anyhow::anyhow!("lecture: {e}"))?;
        if n == 0 {
            break;
        }
        AsyncWriteExt::write_all(&mut send, &buf[..n])
            .await
            .map_err(|e| anyhow::anyhow!("envoi: {e}"))?;
        sent += n as u64;
        // Émettre la progression au plus 10×/s pour ne pas saturer l'IPC.
        if last_emit.elapsed().as_millis() >= 100 {
            let _ = app.emit("ghost-send-progress", serde_json::json!({ "name": name, "sent": sent, "size": size }));
            last_emit = std::time::Instant::now();
        }
    }
    send.finish().map_err(|e| anyhow::anyhow!("finish: {e}"))?;

    let mut ack = [0u8; 2];
    let _ = AsyncReadExt::read_exact(&mut recv, &mut ack).await;
    Ok(name)
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
