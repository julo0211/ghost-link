// Cœur réseau P2P de ghost link — iroh (QUIC, hole-punching + relais chiffré).
// Modèle « session » : on se connecte une fois, puis on s'envoie autant de fichiers
// qu'on veut (dans les deux sens). Avec débit, annulation et déconnexion propagée.

use std::collections::HashSet;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};
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
const CHUNK: usize = 64 * 1024;
// Premier octet de chaque flux bi-directionnel : type de message.
const KIND_FILE: u8 = 1;
const KIND_CHAT: u8 = 2;
const KIND_FREQ: u8 = 3; // demande d'ami
const KIND_FACCEPT: u8 = 4; // acceptation d'ami

#[derive(Default)]
pub struct ConnState {
    generation: u64,
    conn: Option<Connection>,
}
pub type Slot = Arc<Mutex<ConnState>>;

async fn current(slot: &Slot) -> Option<Connection> {
    slot.lock().await.conn.clone()
}

/// Réglages partagés, modifiables à chaud depuis l'UI.
#[derive(Clone, Default)]
pub struct Settings {
    pub download_dir: Arc<StdMutex<Option<PathBuf>>>,
    pub only_friends: Arc<AtomicBool>,
    pub friends: Arc<StdMutex<HashSet<String>>>,
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

#[derive(Clone)]
pub struct Ghost {
    pub app: AppHandle,
    pub slot: Slot,
    pub recv_cancel: Arc<AtomicBool>,
    pub settings: Settings,
}

impl std::fmt::Debug for Ghost {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str("Ghost")
    }
}

impl ProtocolHandler for Ghost {
    async fn accept(&self, connection: Connection) -> Result<(), AcceptError> {
        // Filtre « amis uniquement » : on refuse les pairs inconnus avant tout.
        let peer = connection.remote_id().to_string();
        if !self.settings.allows(&peer) {
            connection.close(0u32.into(), b"not-a-friend");
            let _ = self.app.emit("ghost-refused", &peer);
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
    pub endpoint: Endpoint,
    pub slot: Slot,
    pub send_cancel: Arc<AtomicBool>,
    pub recv_cancel: Arc<AtomicBool>,
    pub settings: Settings,
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
        if bytes.len() == 32 {
            let mut arr = [0u8; 32];
            arr.copy_from_slice(&bytes);
            return SecretKey::from_bytes(&arr);
        }
    }
    let sk = SecretKey::generate();
    if let Some(parent) = path.parent() {
        let _ = std::fs::create_dir_all(parent);
    }
    if std::fs::write(&path, sk.to_bytes()).is_ok() {
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            let _ = std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o600));
        }
    }
    sk
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

pub async fn start(app: AppHandle) -> anyhow::Result<Net> {
    // Identité persistante : même clé (donc même code ami) à chaque lancement.
    let secret = load_or_create_secret();
    let endpoint = Endpoint::builder(presets::N0)
        .secret_key(secret)
        .bind()
        .await
        .map_err(|e| anyhow::anyhow!("bind iroh: {e}"))?;
    let slot: Slot = Arc::new(Mutex::new(ConnState::default()));
    let send_cancel = Arc::new(AtomicBool::new(false));
    let recv_cancel = Arc::new(AtomicBool::new(false));
    let settings = Settings::default();
    if let Some(dir) = load_download_dir() {
        *settings.download_dir.lock().unwrap() = Some(dir);
    }
    let router = Router::builder(endpoint.clone())
        .accept(
            ALPN,
            Ghost {
                app,
                slot: slot.clone(),
                recv_cancel: recv_cancel.clone(),
                settings: settings.clone(),
            },
        )
        .accept(PRESENCE_ALPN, Presence)
        .spawn();
    Ok(Net {
        endpoint,
        slot,
        send_cancel,
        recv_cancel,
        settings,
        _router: router,
    })
}

pub async fn my_addr(ep: &Endpoint) -> anyhow::Result<String> {
    ep.online().await;
    serde_json::to_string(&ep.addr()).map_err(|e| anyhow::anyhow!("sérialisation adresse: {e}"))
}

/// « Code ami » = l'identité publique du nœud (EndpointId). Court, stable, partageable.
/// Un ami qui l'a peut nous retrouver via la découverte, sans coller d'adresse.
pub async fn my_id(ep: &Endpoint) -> anyhow::Result<String> {
    Ok(ep.addr().id.to_string())
}

/// Sonde un ami par son code : tente une connexion légère (ALPN présence) avec un délai borné.
/// Renvoie true s'il est joignable (donc en ligne), false sinon.
pub async fn probe(ep: &Endpoint, id_str: &str) -> bool {
    let id: EndpointId = match id_str.trim().parse() {
        Ok(i) => i,
        Err(_) => return false,
    };
    let addr = EndpointAddr::from(id);
    match tokio::time::timeout(
        std::time::Duration::from_secs(5),
        ep.connect(addr, PRESENCE_ALPN),
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

pub async fn connect(
    ep: &Endpoint,
    app: &AppHandle,
    slot: &Slot,
    recv_cancel: &Arc<AtomicBool>,
    settings: &Settings,
    input: &str,
) -> anyhow::Result<String> {
    let input = input.trim();
    // Accepte soit une adresse complète (JSON), soit un simple « code ami » (EndpointId).
    // Avec un code seul, la découverte (presets::N0) résout l'adresse courante du pair.
    let addr: EndpointAddr = match serde_json::from_str::<EndpointAddr>(input) {
        Ok(a) => a,
        Err(_) => {
            let id: EndpointId = input
                .parse()
                .map_err(|_| anyhow::anyhow!("code ami ou adresse invalide"))?;
            EndpointAddr::from(id)
        }
    };
    let conn = ep
        .connect(addr, ALPN)
        .await
        .map_err(|e| anyhow::anyhow!("connexion: {e}"))?;
    let peer = conn.remote_id().to_string();
    let app2 = app.clone();
    let slot2 = slot.clone();
    let rc = recv_cancel.clone();
    let st = settings.clone();
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
                        // [u16 nom_len][nom]
                        let name = async {
                            let mut l2 = [0u8; 2];
                            AsyncReadExt::read_exact(&mut recv, &mut l2).await?;
                            let nlen = u16::from_be_bytes(l2) as usize;
                            let mut nbuf = vec![0u8; nlen];
                            AsyncReadExt::read_exact(&mut recv, &mut nbuf).await?;
                            anyhow::Ok(String::from_utf8_lossy(&nbuf).to_string())
                        }
                        .await
                        .unwrap_or_default();
                        let ev = if kind[0] == KIND_FREQ { "ghost-freq" } else { "ghost-faccept" };
                        let _ = a.emit(ev, serde_json::json!({ "name": name }));
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

                    let dir = settings.recv_dir();
                    let dest = unique_path(&dir, &name);
                    let _ = a.emit("ghost-recv-start", serde_json::json!({ "name": name, "size": size }));

                    // Corps (Ok(true) = terminé, Ok(false) = annulé, Err = flux coupé)
                    let body: anyhow::Result<bool> = async {
                        let mut file = tokio::fs::File::create(&dest).await?;
                        let mut buf = vec![0u8; CHUNK];
                        let mut got: u64 = 0;
                        while got < size {
                            if cancel.load(Ordering::SeqCst) {
                                return Ok(false);
                            }
                            let want = std::cmp::min(CHUNK as u64, size - got) as usize;
                            AsyncReadExt::read_exact(&mut recv, &mut buf[..want]).await?;
                            AsyncWriteExt::write_all(&mut file, &buf[..want]).await?;
                            got += want as u64;
                            let _ = a.emit("ghost-recv-progress", serde_json::json!({ "name": name, "received": got, "size": size }));
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

    let mut file = tokio::fs::File::open(p)
        .await
        .map_err(|e| anyhow::anyhow!("ouverture: {e}"))?;
    let mut buf = vec![0u8; CHUNK];
    let mut sent: u64 = 0;
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
        let _ = app.emit("ghost-send-progress", serde_json::json!({ "name": name, "sent": sent, "size": size }));
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
async fn send_named(slot: &Slot, kind: u8, name: &str) -> anyhow::Result<()> {
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
    send.finish().map_err(|e| anyhow::anyhow!("finish: {e}"))?;
    Ok(())
}

/// Demande d'ami (au pair connecté), avec le nom d'affichage.
pub async fn send_freq(slot: &Slot, name: &str) -> anyhow::Result<()> {
    send_named(slot, KIND_FREQ, name).await
}

/// Acceptation d'une demande d'ami, avec le nom d'affichage.
pub async fn send_faccept(slot: &Slot, name: &str) -> anyhow::Result<()> {
    send_named(slot, KIND_FACCEPT, name).await
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
        .filter(|c| !matches!(c, '/' | '\\' | ':' | '*' | '?' | '"' | '<' | '>' | '|'))
        .collect();
    if cleaned.trim().is_empty() {
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
