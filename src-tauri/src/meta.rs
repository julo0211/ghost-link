// Nettoyage des MÉTADONNÉES avant envoi — la confidentialité est le produit.
// Le fichier envoyé est une COPIE nettoyée écrite dans un dossier temporaire ; le
// fichier original de l'utilisateur n'est JAMAIS modifié. Le hash d'intégrité et la
// taille annoncés au pair sont calculés sur la copie nettoyée (cohérence garantie).
//
// Couverture v1 (tout le reste passe tel quel, les formats à métadonnées connus mais
// non nettoyables déclenchent un avertissement dans l'UI via l'événement ghost-meta) :
// - JPEG  : EXIF/XMP (APP1), IPTC (APP13), APPn divers, commentaires — et TRONCATURE
//           après EOI : les « motion photos » Samsung/Google cachent une vidéo MP4
//           géolocalisée APRÈS la fin de l'image.
// - PNG   : tEXt/zTXt/iTXt/tIME/eXIf (+ données après IEND).
// - WebP  : chunks EXIF/XMP + bits correspondants du VP8X.
// - WAV   : LIST INFO, bext (broadcast), iXML/axml, id3.
// - MP3   : ID3v2 (en-tête + pied), ID3v1, APEv2.
// - MP4/MOV/M4A : atomes udta/meta (GPS ©xyz, tags iTunes) renommés « free » ET
//           contenu mis à ZÉRO pendant la copie — aucun offset ne bouge, donc aucune
//           table (stco/co64) à réécrire. PAS appliqué aux HEIC/HEIF (leur « meta »
//           contient les données de décodage).
// - PDF   : dictionnaire /Info, /Metadata XMP du catalogue, /ID (via lopdf).
// - OOXML (docx/xlsx/pptx…) : docProps/core.xml, app.xml, custom.xml blanchis.
// - ODF (odt/ods/odp…) : meta.xml blanchi.
//
// LIMITES ASSUMÉES (v1) :
// - Fail-open : si le nettoyage échoue ou n'est pas pris en charge, l'ORIGINAL part
//   quand même, avec un avertissement VISIBLE (ghost-meta) — on ne bloque jamais un
//   transfert, mais on ne se tait jamais non plus.
// - On nettoie les MÉTADONNÉES, pas le CONTENU : commentaires/révisions/auteurs dans
//   le corps d'un docx (word/comments.xml, people.xml), annotations et XMP par page
//   d'un PDF, texte incrusté — c'est du contenu que l'utilisateur peut vouloir
//   transmettre ; le modifier serait de l'altération silencieuse.
// - Le NOM du fichier part tel quel (comportement historique de l'app) : le nom est
//   souvent lui-même une métadonnée — à l'utilisateur de renommer avant envoi.

use std::fs;
use std::io::{Seek, SeekFrom, Write};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};

/// Taille max. d'un fichier chargé EN MÉMOIRE pour nettoyage (images, audio, docs).
const MAX_IN_MEMORY: u64 = 256 * 1024 * 1024;
/// Plafond SPÉCIFIQUE au PDF, bien plus bas que MAX_IN_MEMORY (deflate-bomb) :
/// lopdf::Document::load DÉCOMPRESSE les object/xref streams pour parser. Un PDF
/// hostile de quelques centaines de Ko (streams FlateDecode à très fort taux, ou
/// imbriqués) peut allouer plusieurs Go → en Rust l'échec d'allocation ABORTE le
/// process entier (un catch_unwind ne rattrape pas un OOM). On préfère un Skipped
/// VISIBLE au-delà du seuil plutôt qu'un crash silencieux du transfert.
const MAX_PDF: u64 = 64 * 1024 * 1024;
/// Taille max. d'une vidéo recopiée pour patch (copie disque, pas de chargement RAM).
const MAX_MP4_COPY: u64 = 2 * 1024 * 1024 * 1024;
/// Âge au-delà duquel une copie nettoyée du dossier temporaire est purgée. 24 h : un
/// envoi de GROUPE attend l'acceptation du pair sans limite et les tranches rouvrent
/// le fichier par chemin — purger trop tôt casserait un transfert accepté tardivement.
/// NB : les copies sont en CLAIR dans %TEMP% (dossier par-utilisateur sous Windows),
/// sur le disque de l'émetteur — au même titre que l'original juste à côté.
const TEMP_MAX_AGE: std::time::Duration = std::time::Duration::from_secs(24 * 3600);
/// Garde-fou contre les structures pathologiques (nombre max d'atomes MP4 parcourus).
const MAX_BOXES: usize = 65_536;

static TEMP_SEQ: AtomicU64 = AtomicU64::new(1);

/// Résultat de la préparation d'un fichier avant envoi.
pub enum Prep {
    /// Copie nettoyée écrite dans le dossier temporaire : c'est ELLE qu'il faut lire.
    Cleaned(PathBuf),
    /// Rien à changer : pas de métadonnées trouvées, ou format qui n'en porte pas.
    Untouched,
    /// Format à métadonnées CONNU mais non nettoyable (raison) : original + avertir.
    Skipped(&'static str),
    /// Nettoyage tenté mais échoué (raison) : original + avertir.
    Failed(String),
}

/// Prépare un fichier pour l'envoi : purge les vieilles copies, route selon
/// l'extension, écrit la copie nettoyée. BLOQUANT (à appeler via spawn_blocking).
pub fn prepare(path: &Path) -> Prep {
    gc_temp();
    let ext = path
        .extension()
        .and_then(|e| e.to_str())
        .map(|s| s.to_ascii_lowercase())
        .unwrap_or_default();
    let size = match fs::metadata(path) {
        Ok(m) => m.len(),
        Err(e) => return Prep::Failed(format!("lecture: {e}")),
    };
    match ext.as_str() {
        "jpg" | "jpeg" | "jfif" | "jpe" | "jfi" | "jif" => {
            clean_in_memory(path, size, clean_jpeg)
        }
        "png" => clean_in_memory(path, size, clean_png),
        "webp" => clean_in_memory(path, size, clean_webp),
        "wav" => clean_in_memory(path, size, clean_wav),
        "mp3" => clean_in_memory(path, size, clean_mp3),
        "mp4" | "m4a" | "m4v" | "mov" | "3gp" => clean_mp4_file(path, size),
        "pdf" => clean_pdf_file(path, size),
        "docx" | "xlsx" | "pptx" | "docm" | "xlsm" | "pptm" | "dotx" | "xltx" | "potx"
        | "ppsx" => clean_zip_doc(path, size, ZipDoc::Ooxml),
        "odt" | "ods" | "odp" | "odg" => clean_zip_doc(path, size, ZipDoc::Odf),
        // Métadonnées présentes mais nettoyage non implémenté : prévenir, ne pas se taire.
        // (heic/heif = photos iPhone : EXIF/GPS complet ; RAW constructeurs ; conteneurs
        // vidéo à tags ; formats bureautiques hérités ; svg = XML avec commentaires.)
        "heic" | "heif" | "tif" | "tiff" | "gif" | "avi" | "mkv" | "webm" | "flac" | "ogg"
        | "opus" | "aac" | "wma" | "wmv" | "doc" | "xls" | "ppt" | "rtf" | "dng" | "cr2"
        | "cr3" | "nef" | "arw" | "orf" | "rw2" | "raf" | "avif" | "jxl" | "svg" | "mts"
        | "m2ts" | "mpg" | "mpeg" | "flv" | "3g2" | "mka" | "m4b" => {
            Prep::Skipped("format à métadonnées non nettoyable pour l'instant")
        }
        // Extension inconnue/absente : ne pas se fier à l'extension SEULE (un JPEG en
        // .dat, en .jpg_large ou sans extension emporterait son GPS silencieusement —
        // exactement l'« échec silencieux » que le contrat interdit). Sniffer les magic
        // bytes des formats supportés ; router vers le bon nettoyeur, ou rester VISIBLE.
        _ => match sniff_magic(path) {
            Some(Magic::Jpeg) => clean_in_memory(path, size, clean_jpeg),
            Some(Magic::Png) => clean_in_memory(path, size, clean_png),
            Some(Magic::Webp) => clean_in_memory(path, size, clean_webp),
            Some(Magic::Wav) => clean_in_memory(path, size, clean_wav),
            Some(Magic::Mp3) => clean_in_memory(path, size, clean_mp3),
            Some(Magic::Mp4) => clean_mp4_file(path, size),
            Some(Magic::Pdf) => clean_pdf_file(path, size),
            // ZIP au contenu inconnu (peut être un docx/odt renommé) : on ne devine
            // pas OOXML vs ODF sans l'ouvrir — rester visible plutôt que se taire.
            Some(Magic::Zip) => Prep::Skipped("extension ne correspond pas au contenu"),
            None => Prep::Untouched,
        },
    }
}

/// Formats détectés par magic bytes quand l'extension ne dit rien de fiable.
enum Magic {
    Jpeg,
    Png,
    Webp,
    Wav,
    Mp3,
    Mp4,
    Pdf,
    Zip,
}

/// Renifle les premiers octets pour reconnaître un format porteur de métadonnées.
fn sniff_magic(path: &Path) -> Option<Magic> {
    use std::io::Read;
    let mut f = fs::File::open(path).ok()?;
    let mut buf = [0u8; 16];
    let n = f.read(&mut buf).ok()?;
    let h = &buf[..n];
    if h.len() >= 2 && h[0] == 0xFF && h[1] == 0xD8 {
        return Some(Magic::Jpeg); // FFD8 (JPEG SOI)
    }
    if h.len() >= 8 && h[..8] == PNG_SIG {
        return Some(Magic::Png);
    }
    if h.len() >= 12 && &h[..4] == b"RIFF" {
        let form = &h[8..12];
        if form == b"WEBP" {
            return Some(Magic::Webp);
        }
        if form == b"WAVE" {
            return Some(Magic::Wav);
        }
        return None;
    }
    if h.len() >= 5 && &h[..5] == b"%PDF-" {
        return Some(Magic::Pdf);
    }
    if h.len() >= 4 && &h[..4] == b"PK\x03\x04" {
        return Some(Magic::Zip);
    }
    if h.len() >= 8 && &h[4..8] == b"ftyp" {
        return Some(Magic::Mp4);
    }
    if h.len() >= 3 && &h[..3] == b"ID3" {
        return Some(Magic::Mp3);
    }
    // Trame MPEG audio nue (sync 11 bits) : FF Ex/Fx — MP3 sans tag ID3 en tête.
    if h.len() >= 2 && h[0] == 0xFF && (h[1] & 0xE0) == 0xE0 {
        return Some(Magic::Mp3);
    }
    None
}

// ---- Copies temporaires ----

fn temp_dir() -> PathBuf {
    std::env::temp_dir().join("ghostlink-clean")
}

fn temp_path(orig: &Path) -> PathBuf {
    let dir = temp_dir();
    let _ = fs::create_dir_all(&dir);
    let n = TEMP_SEQ.fetch_add(1, Ordering::Relaxed);
    let ts = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_millis())
        .unwrap_or(0);
    let ext = orig.extension().and_then(|e| e.to_str()).unwrap_or("bin");
    // pid dans le nom : %TEMP%/ghostlink-clean est partagé entre instances de l'app,
    // et TEMP_SEQ redémarre à 1 par processus — sans pid, collision possible.
    let pid = std::process::id();
    dir.join(format!("clean-{pid}-{ts}-{n}.{ext}"))
}

/// Purge les copies nettoyées assez vieilles pour qu'aucun envoi ne les lise encore.
/// (Les tranches parallèles rouvrent le fichier par chemin pendant tout le transfert :
/// on ne peut pas supprimer à la fin d'UNE tâche, un envoi de groupe peut durer.)
/// Appelée avant chaque préparation ET au démarrage de l'app (main.rs) — sinon les
/// copies du dernier envoi d'une session resteraient indéfiniment.
pub fn gc_temp() {
    if let Ok(rd) = fs::read_dir(temp_dir()) {
        for e in rd.flatten() {
            let old = e
                .metadata()
                .and_then(|m| m.modified())
                .ok()
                .and_then(|t| t.elapsed().ok())
                .map(|age| age > TEMP_MAX_AGE)
                .unwrap_or(false);
            if old {
                let _ = fs::remove_file(e.path());
            }
        }
    }
}

type Cleaner = fn(&[u8]) -> Result<Option<Vec<u8>>, String>;

fn clean_in_memory(path: &Path, size: u64, f: Cleaner) -> Prep {
    if size > MAX_IN_MEMORY {
        return Prep::Skipped("trop volumineux pour le nettoyage");
    }
    let data = match fs::read(path) {
        Ok(d) => d,
        Err(e) => return Prep::Failed(format!("lecture: {e}")),
    };
    match f(&data) {
        Ok(None) => Prep::Untouched,
        Ok(Some(bytes)) => {
            let tmp = temp_path(path);
            match fs::write(&tmp, &bytes) {
                Ok(()) => Prep::Cleaned(tmp),
                Err(e) => {
                    // Écriture partielle (disque plein) : ne pas laisser d'orphelin
                    // jusqu'à la purge 24 h — cohérent avec les chemins MP4/PDF/ZIP.
                    let _ = fs::remove_file(&tmp);
                    Prep::Failed(format!("écriture temporaire: {e}"))
                }
            }
        }
        Err(e) => Prep::Failed(e),
    }
}

// ---- JPEG ----

/// Filtre les segments : garde JFIF (APP0), ICC (APP2 « ICC_PROFILE »), Adobe (APP14)
/// et tout le nécessaire au décodage ; jette EXIF/XMP (APP1), IPTC (APP13), les autres
/// APPn et les commentaires — Y COMPRIS entre les scans d'un JPEG progressif. Tronque
/// après EOI (les « motion photos » Samsung/Google y cachent une vidéo géolocalisée).
/// NB : retirer l'EXIF retire aussi le drapeau d'orientation — certaines photos
/// s'afficheront « couchées » chez le destinataire, prix assumé de la suppression du GPS.
fn clean_jpeg(d: &[u8]) -> Result<Option<Vec<u8>>, String> {
    if d.len() < 4 || d[0] != 0xFF || d[1] != 0xD8 {
        return Err("pas un JPEG".into());
    }
    let mut out = Vec::with_capacity(d.len());
    out.extend_from_slice(&d[..2]);
    let mut i = 2usize;
    let mut changed = false;
    let mut in_scan = false;
    loop {
        if in_scan {
            // Flux entropique : copier jusqu'au prochain VRAI marqueur — 0xFF suivi
            // d'autre chose que 0x00 (stuffing), D0-D7 (RST) ou 0xFF (bourrage).
            let mut k = i;
            while k < d.len() {
                if d[k] == 0xFF && k + 1 < d.len() {
                    let m = d[k + 1];
                    if m != 0x00 && m != 0xFF && !(0xD0..=0xD7).contains(&m) {
                        break;
                    }
                }
                k += 1;
            }
            out.extend_from_slice(&d[i..k]);
            if k >= d.len() {
                break; // pas d'EOI : fichier copié tel quel
            }
            i = k;
            in_scan = false;
            continue;
        }
        if i + 1 >= d.len() {
            out.extend_from_slice(&d[i.min(d.len())..]);
            break;
        }
        if d[i] != 0xFF {
            return Err("structure JPEG invalide".into());
        }
        // Octets de bourrage 0xFF répétés avant le marqueur.
        let mut j = i;
        while j + 1 < d.len() && d[j + 1] == 0xFF {
            j += 1;
        }
        if j + 1 >= d.len() {
            // Fichier se terminant en plein bourrage : lire d[j+1] paniquerait.
            return Err("marqueur JPEG tronqué".into());
        }
        let marker = d[j + 1];
        if marker == 0xD9 {
            out.extend_from_slice(&d[i..j + 2]);
            if j + 2 < d.len() {
                changed = true; // données après EOI : coupées
            }
            break;
        }
        if marker == 0x01 || (0xD0..=0xD7).contains(&marker) {
            out.extend_from_slice(&d[i..j + 2]);
            i = j + 2;
            continue;
        }
        if j + 3 >= d.len() {
            return Err("segment JPEG tronqué".into());
        }
        let len = u16::from_be_bytes([d[j + 2], d[j + 3]]) as usize;
        if len < 2 || j + 2 + len > d.len() {
            return Err("longueur de segment JPEG invalide".into());
        }
        let seg_end = j + 2 + len;
        let keep = match marker {
            0xE0 => true,                                             // APP0 JFIF
            0xE2 => d[j + 4..seg_end].starts_with(b"ICC_PROFILE\0"),  // profil couleur
            0xEE => true,                                             // APP14 Adobe
            0xE1 | 0xE3..=0xED | 0xEF | 0xFE => false, // EXIF/XMP, APPn, IPTC, COM
            _ => true,                                 // DQT/DHT/SOF/DRI/DNL/…
        };
        if keep {
            out.extend_from_slice(&d[i..seg_end]);
        } else {
            changed = true;
        }
        i = seg_end;
        if marker == 0xDA {
            in_scan = true; // l'en-tête de scan est copié, le flux entropique suit
        }
    }
    Ok(if changed { Some(out) } else { None })
}

// ---- PNG ----

const PNG_SIG: [u8; 8] = [137, 80, 78, 71, 13, 10, 26, 10];

/// Liste BLANCHE : chunks critiques (IHDR/PLTE/IDAT/IEND) + auxiliaires utiles au
/// rendu (transparence, couleur, APNG). Tout le reste est jeté — textes
/// (tEXt/zTXt/iTXt), horodatage (tIME), eXIf, et les chunks PRIVÉS (iDOT Apple,
/// prVW Fireworks, inconnus…) qui sont autant de canaux de métadonnées. Les chunks
/// auxiliaires sont ignorables par les décodeurs : les jeter ne casse pas l'affichage.
fn clean_png(d: &[u8]) -> Result<Option<Vec<u8>>, String> {
    if d.len() < 8 || d[..8] != PNG_SIG {
        return Err("pas un PNG".into());
    }
    let mut out = Vec::with_capacity(d.len());
    out.extend_from_slice(&d[..8]);
    let mut i = 8usize;
    let mut changed = false;
    while i + 12 <= d.len() {
        let len = u32::from_be_bytes([d[i], d[i + 1], d[i + 2], d[i + 3]]) as usize;
        let typ: [u8; 4] = d[i + 4..i + 8].try_into().unwrap();
        let total = match len.checked_add(12) {
            Some(t) if i + t <= d.len() => t,
            _ => return Err("chunk PNG tronqué".into()),
        };
        let keep = matches!(
            &typ,
            b"IHDR" | b"PLTE" | b"IDAT" | b"IEND" | b"tRNS" | b"gAMA" | b"cHRM"
                | b"sRGB" | b"iCCP" | b"sBIT" | b"bKGD" | b"pHYs" | b"hIST" | b"sPLT"
                | b"acTL" | b"fcTL" | b"fdAT"
        );
        if keep {
            out.extend_from_slice(&d[i..i + total]);
        } else {
            changed = true;
        }
        i += total;
        if &typ == b"IEND" {
            if i < d.len() {
                changed = true; // données après IEND : coupées
            }
            break;
        }
    }
    Ok(if changed { Some(out) } else { None })
}

// ---- RIFF (WebP + WAV) ----

fn clean_webp(d: &[u8]) -> Result<Option<Vec<u8>>, String> {
    clean_riff(d, false)
}
fn clean_wav(d: &[u8]) -> Result<Option<Vec<u8>>, String> {
    clean_riff(d, true)
}

fn clean_riff(d: &[u8], wav: bool) -> Result<Option<Vec<u8>>, String> {
    if d.len() < 12 || &d[..4] != b"RIFF" {
        return Err("pas un conteneur RIFF".into());
    }
    let form = &d[8..12];
    if (wav && form != b"WAVE") || (!wav && form != b"WEBP") {
        return Err("type RIFF inattendu".into());
    }
    let mut kept: Vec<(usize, usize)> = Vec::new(); // (début, taille totale) des chunks gardés
    let mut i = 12usize;
    let mut changed = false;
    while i + 8 <= d.len() {
        let id: [u8; 4] = d[i..i + 4].try_into().unwrap();
        let sz = u32::from_le_bytes(d[i + 4..i + 8].try_into().unwrap()) as usize;
        if i + 8 + sz > d.len() {
            return Err("chunk RIFF tronqué".into());
        }
        // total avec octet de bourrage (sauf s'il manque en toute fin de fichier)
        let total = (8 + sz + (sz & 1)).min(d.len() - i);
        let drop = if wav {
            match &id {
                b"LIST" => sz >= 4 && &d[i + 8..i + 12] == b"INFO",
                b"id3 " | b"ID3 " | b"bext" | b"iXML" | b"axml" | b"aXML" | b"_PMX" => true,
                _ => false,
            }
        } else {
            matches!(&id, b"EXIF" | b"XMP ")
        };
        if drop {
            changed = true;
        } else {
            kept.push((i, total));
        }
        i += total;
    }
    if !changed {
        return Ok(None);
    }
    let mut out = Vec::with_capacity(d.len());
    out.extend_from_slice(b"RIFF\0\0\0\0");
    out.extend_from_slice(form);
    for (start, total) in kept {
        out.extend_from_slice(&d[start..start + total]);
    }
    let riff_size = (out.len() - 8) as u32;
    out[4..8].copy_from_slice(&riff_size.to_le_bytes());
    if !wav {
        patch_vp8x(&mut out);
    }
    Ok(Some(out))
}

/// Éteint les bits EXIF (0x08) et XMP (0x04) du VP8X — sinon un décodeur strict
/// chercherait des chunks qui n'existent plus.
fn patch_vp8x(buf: &mut [u8]) {
    let mut i = 12usize;
    while i + 8 <= buf.len() {
        let sz = u32::from_le_bytes(buf[i + 4..i + 8].try_into().unwrap()) as usize;
        if &buf[i..i + 4] == b"VP8X" && sz >= 1 && i + 8 < buf.len() {
            buf[i + 8] &= !(0x08 | 0x04);
            return;
        }
        i += 8 + sz + (sz & 1);
    }
}

// ---- MP3 ----

fn clean_mp3(d: &[u8]) -> Result<Option<Vec<u8>>, String> {
    let mut start = 0usize;
    let mut end = d.len();
    let mut changed = false;
    // ID3v2 en tête : 10 octets d'en-tête + taille « syncsafe » (+ pied éventuel).
    if d.len() >= 10 && &d[..3] == b"ID3" {
        if d[6] | d[7] | d[8] | d[9] >= 0x80 {
            return Err("taille ID3v2 invalide".into());
        }
        let sz = ((d[6] as usize) << 21)
            | ((d[7] as usize) << 14)
            | ((d[8] as usize) << 7)
            | (d[9] as usize);
        let mut total = 10 + sz;
        if d[5] & 0x10 != 0 {
            total += 10; // pied ID3v2.4
        }
        if total > d.len() {
            return Err("ID3v2 tronqué".into());
        }
        start = total;
        changed = true;
    }
    // ID3v1 en queue : 128 octets commençant par « TAG ».
    if end >= start + 128 && &d[end - 128..end - 125] == b"TAG" {
        end -= 128;
        changed = true;
    }
    // APEv2 en queue : pied « APETAGEX » de 32 octets.
    if end >= start + 32 && &d[end - 32..end - 24] == b"APETAGEX" {
        let tag_size =
            u32::from_le_bytes(d[end - 20..end - 16].try_into().unwrap()) as usize;
        let flags = u32::from_le_bytes(d[end - 12..end - 8].try_into().unwrap());
        let total = tag_size + if flags & 0x8000_0000 != 0 { 32 } else { 0 };
        if total <= end - start {
            end -= total;
            changed = true;
        }
    }
    Ok(if changed { Some(d[start..end].to_vec()) } else { None })
}

// ---- MP4 / MOV / M4A (ISO-BMFF) ----

/// Un atome à neutraliser : (offset du début de l'atome, taille totale, taille d'en-tête).
type BoxSpan = (u64, u64, u64);

/// UUID des boxes `uuid` contenant du XMP (Adobe Premiere/After Effects, caméras) :
/// BE7ACFCB-97A9-42E8-9C71-999491E3AFAC — souvent GPS, auteur, historique de montage.
const XMP_BOX_UUID: [u8; 16] = [
    0xBE, 0x7A, 0xCF, 0xCB, 0x97, 0xA9, 0x42, 0xE8, 0x9C, 0x71, 0x99, 0x94, 0x91, 0xE3, 0xAF, 0xAC,
];

/// Lit les 16 octets d'usertype d'une box `uuid` (à `pos`) et dit si c'est du XMP.
fn mp4_uuid_is_xmp(f: &mut fs::File, pos: u64) -> Result<bool, String> {
    use std::io::Read;
    let mut id = [0u8; 16];
    f.seek(SeekFrom::Start(pos)).map_err(|e| e.to_string())?;
    f.read_exact(&mut id).map_err(|e| e.to_string())?;
    Ok(id == XMP_BOX_UUID)
}

fn clean_mp4_file(path: &Path, size: u64) -> Prep {
    if size > MAX_MP4_COPY {
        return Prep::Skipped("trop volumineux pour le nettoyage");
    }
    match mp4_meta_offsets(path) {
        Err(e) => Prep::Failed(e),
        Ok(spans) if spans.is_empty() => Prep::Untouched,
        Ok(spans) => {
            let tmp = temp_path(path);
            match copy_with_free_patches(path, &tmp, &spans) {
                Ok(()) => Prep::Cleaned(tmp),
                Err(e) => {
                    let _ = fs::remove_file(&tmp);
                    Prep::Failed(e)
                }
            }
        }
    }
}

/// Lit l'en-tête d'atome à `pos` : (taille totale, type, taille d'en-tête).
fn read_box_header(
    f: &mut fs::File,
    pos: u64,
    flen: u64,
) -> Result<(u64, [u8; 4], u64), String> {
    use std::io::Read;
    let mut hdr = [0u8; 8];
    f.seek(SeekFrom::Start(pos)).map_err(|e| e.to_string())?;
    f.read_exact(&mut hdr).map_err(|e| e.to_string())?;
    let sz32 = u32::from_be_bytes(hdr[..4].try_into().unwrap()) as u64;
    let typ: [u8; 4] = hdr[4..8].try_into().unwrap();
    let (total, hlen) = if sz32 == 1 {
        let mut big = [0u8; 8];
        f.read_exact(&mut big).map_err(|e| e.to_string())?;
        (u64::from_be_bytes(big), 16u64)
    } else if sz32 == 0 {
        (flen - pos, 8u64) // jusqu'à la fin du fichier
    } else {
        (sz32, 8u64)
    };
    // Soustraction saturante et non `pos + total > flen` : un largesize forgé à
    // u64::MAX ferait déborder l'addition et contournerait le contrôle de bornes.
    if total < hlen || total > flen.saturating_sub(pos) {
        return Err("atome MP4 invalide".into());
    }
    Ok((total, typ, hlen))
}

/// Atomes à neutraliser (début, taille, en-tête) : moov/udta, moov/meta,
/// moov/trak/udta, et udta/meta de premier niveau (rares).
fn mp4_meta_offsets(path: &Path) -> Result<Vec<BoxSpan>, String> {
    let mut f = fs::File::open(path).map_err(|e| e.to_string())?;
    let flen = f.metadata().map_err(|e| e.to_string())?.len();
    let mut spans = Vec::new();
    let mut boxes = 0usize;
    let mut saw_known = false;
    let mut pos = 0u64;
    while flen.saturating_sub(pos) >= 8 {
        boxes += 1;
        if boxes > MAX_BOXES {
            return Err("structure MP4 anormale".into());
        }
        let (total, typ, hlen) = read_box_header(&mut f, pos, flen)?;
        if matches!(
            &typ,
            b"ftyp" | b"moov" | b"mdat" | b"free" | b"skip" | b"wide" | b"moof" | b"mfra"
                | b"styp" | b"sidx" | b"pdin" | b"uuid"
        ) {
            saw_known = true;
        }
        match &typ {
            b"udta" | b"meta" => spans.push((pos, total, hlen)),
            // XMP logé dans une box `uuid` top-level : même neutralisation (free +
            // zéro), offsets inchangés. On ne touche que l'UUID XMP connu, pas les
            // autres uuid (certains portent des données requises au décodage).
            b"uuid" => {
                if total >= hlen + 16 && mp4_uuid_is_xmp(&mut f, pos + hlen)? {
                    spans.push((pos, total, hlen));
                }
            }
            b"moov" => {
                collect_meta_children(&mut f, pos + hlen, pos + total, flen, 0, &mut boxes, &mut spans)?
            }
            _ => {}
        }
        pos += total; // sûr : read_box_header garantit total ≤ flen - pos
    }
    if !saw_known {
        return Err("structure MP4 non reconnue".into());
    }
    Ok(spans)
}

/// Parcourt les enfants d'un conteneur ; descend d'UN niveau dans les trak.
fn collect_meta_children(
    f: &mut fs::File,
    start: u64,
    end: u64,
    flen: u64,
    depth: u8,
    boxes: &mut usize,
    spans: &mut Vec<BoxSpan>,
) -> Result<(), String> {
    let mut pos = start;
    while end.saturating_sub(pos) >= 8 {
        *boxes += 1;
        if *boxes > MAX_BOXES {
            return Err("structure MP4 anormale".into());
        }
        let (total, typ, hlen) = read_box_header(f, pos, flen)?;
        if total > end.saturating_sub(pos) {
            return Err("atome MP4 hors de son conteneur".into());
        }
        match &typ {
            b"udta" | b"meta" => spans.push((pos, total, hlen)),
            b"trak" if depth == 0 => {
                collect_meta_children(f, pos + hlen, pos + total, flen, 1, boxes, spans)?
            }
            _ => {}
        }
        pos += total;
    }
    Ok(())
}

/// Copie le fichier puis neutralise chaque atome visé : TYPE remplacé par « free »
/// ET contenu mis à ZÉRO. Renommer seul ne suffirait pas : les lecteurs ignoreraient
/// l'atome mais les octets (GPS ©xyz, tags) resteraient physiquement dans le fichier.
/// Les tailles ne changent pas → les tables d'offsets stco/co64 restent valides.
fn copy_with_free_patches(src: &Path, dst: &Path, spans: &[BoxSpan]) -> Result<(), String> {
    fs::copy(src, dst).map_err(|e| format!("copie: {e}"))?;
    let mut f = fs::OpenOptions::new()
        .write(true)
        .open(dst)
        .map_err(|e| e.to_string())?;
    let zeros = vec![0u8; 64 * 1024];
    for &(pos, total, hlen) in spans {
        f.seek(SeekFrom::Start(pos + 4)).map_err(|e| e.to_string())?;
        f.write_all(b"free").map_err(|e| e.to_string())?;
        // Zéroter le contenu (après l'en-tête — le largesize éventuel doit rester).
        f.seek(SeekFrom::Start(pos + hlen)).map_err(|e| e.to_string())?;
        let mut left = total - hlen;
        while left > 0 {
            let n = left.min(zeros.len() as u64) as usize;
            f.write_all(&zeros[..n]).map_err(|e| e.to_string())?;
            left -= n as u64;
        }
    }
    Ok(())
}

// ---- PDF (lopdf) ----

/// Issue du nettoyage PDF (avant construction du Prep par l'appelant).
enum PdfOutcome {
    /// Métadonnées retirées : la copie a été écrite.
    Cleaned,
    /// Aucune métadonnée à retirer : NE PAS réécrire (voir clean_pdf_inner).
    Untouched,
    /// PDF signé numériquement : réécrire invaliderait la signature.
    Signed,
}

/// Point d'entrée du SOUS-PROCESSUS de nettoyage PDF (#12) : appelé par main.rs quand
/// l'app est relancée avec `--gl-clean-pdf <in> <out>`. Renvoie un code de sortie que
/// `clean_pdf_file` mappe vers un `Prep`. catch_unwind contient les paniques internes de
/// lopdf ; un OOM (deflate-bomb) abort ce process — le parent le traite comme Skipped.
pub fn clean_pdf_worker(in_path: &str, out_path: &str) -> i32 {
    match std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
        clean_pdf_inner(Path::new(in_path), Path::new(out_path))
    })) {
        Ok(Ok(PdfOutcome::Cleaned)) => 0,
        Ok(Ok(PdfOutcome::Untouched)) => 10,
        Ok(Ok(PdfOutcome::Signed)) => 11,
        Ok(Err(_)) => 12,
        Err(_) => 13, // panique lopdf contenue
    }
}

fn clean_pdf_file(path: &Path, size: u64) -> Prep {
    // Plafond PDF prudent (deflate-bomb, cf. MAX_PDF) : au-delà, Skipped VISIBLE.
    if size > MAX_PDF {
        return Prep::Skipped("PDF trop volumineux pour le nettoyage");
    }
    let tmp = temp_path(path);
    // #12 : le nettoyage lopdf tourne dans un SOUS-PROCESSUS ISOLÉ (re-exec de nous-mêmes
    // avec --gl-clean-pdf, cf. main.rs). Une deflate-bomb qui provoque un OOM (abort NON
    // rattrapable par catch_unwind) ne tue QUE ce sous-processus ; le parent renvoie alors
    // Skipped VISIBLE (métadonnées non retirées mais transfert préservé — contrat respecté).
    // Le code de sortie encode le résultat (voir clean_pdf_worker).
    let code = std::env::current_exe().ok().and_then(|exe| {
        std::process::Command::new(exe)
            .arg("--gl-clean-pdf")
            .arg(path.as_os_str())
            .arg(tmp.as_os_str())
            .status()
            .ok()
            .map(|s| s.code())
    });
    match code {
        Some(Some(0)) => Prep::Cleaned(tmp),
        Some(Some(10)) => {
            let _ = fs::remove_file(&tmp);
            Prep::Untouched
        }
        Some(Some(11)) => {
            let _ = fs::remove_file(&tmp);
            Prep::Skipped("PDF signé — nettoyage sauté pour préserver la signature")
        }
        Some(Some(12)) => {
            let _ = fs::remove_file(&tmp);
            Prep::Failed("PDF: illisible ou chiffré".to_string())
        }
        Some(Some(13)) => {
            let _ = fs::remove_file(&tmp);
            Prep::Failed("PDF: parseur en échec (fichier malformé)".to_string())
        }
        // Sous-processus tué (OOM/abort) ou code inattendu : repli VISIBLE, transfert préservé.
        Some(_) => {
            let _ = fs::remove_file(&tmp);
            Prep::Skipped("PDF trop coûteux à décompresser — nettoyage sauté")
        }
        // Impossible de lancer le sous-processus (current_exe/spawn en échec, rare) : repli
        // in-process protégé par catch_unwind (contient les paniques ; l'OOM ne reste possible
        // que dans ce cas résiduel).
        None => match std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| clean_pdf_inner(path, &tmp))) {
            Ok(Ok(PdfOutcome::Cleaned)) => Prep::Cleaned(tmp),
            Ok(Ok(PdfOutcome::Untouched)) => {
                let _ = fs::remove_file(&tmp);
                Prep::Untouched
            }
            Ok(Ok(PdfOutcome::Signed)) => {
                let _ = fs::remove_file(&tmp);
                Prep::Skipped("PDF signé — nettoyage sauté pour préserver la signature")
            }
            Ok(Err(e)) => {
                let _ = fs::remove_file(&tmp);
                Prep::Failed(format!("PDF: {e}"))
            }
            Err(_) => {
                let _ = fs::remove_file(&tmp);
                Prep::Failed("PDF: parseur en échec (fichier malformé)".to_string())
            }
        },
    }
}

/// Détecte une signature numérique : /AcroForm SigFlags (bit 1 = SignaturesExist)
/// dans le catalogue, ou un objet /Type /Sig. Re-sérialiser un PDF signé casse la
/// signature — mieux vaut prévenir (Skipped) que corrompre silencieusement.
fn pdf_is_signed(doc: &lopdf::Document) -> bool {
    if let Ok(root_id) = doc.trailer.get(b"Root").and_then(|o| o.as_reference()) {
        if let Some(acro) = doc
            .get_object(root_id)
            .ok()
            .and_then(|o| o.as_dict().ok())
            .and_then(|d| d.get(b"AcroForm").ok())
        {
            // /AcroForm peut être une référence indirecte ou un dictionnaire en ligne.
            let acro_dict = acro
                .as_reference()
                .ok()
                .and_then(|id| doc.get_object(id).ok())
                .and_then(|o| o.as_dict().ok())
                .or_else(|| acro.as_dict().ok());
            if let Some(dict) = acro_dict {
                if let Ok(flags) = dict.get(b"SigFlags").and_then(|o| o.as_i64()) {
                    if flags & 1 != 0 {
                        return true;
                    }
                }
            }
        }
    }
    doc.objects.values().any(|o| {
        o.as_dict()
            .ok()
            .and_then(|d| d.get(b"Type").ok())
            .and_then(|t| t.as_name().ok())
            .map(|n| n == b"Sig")
            .unwrap_or(false)
    })
}

fn clean_pdf_inner(path: &Path, out: &Path) -> Result<PdfOutcome, String> {
    let mut doc = lopdf::Document::load(path).map_err(|e| e.to_string())?;
    if doc.trailer.get(b"Encrypt").is_ok() {
        return Err("PDF chiffré".into());
    }
    if pdf_is_signed(&doc) {
        return Ok(PdfOutcome::Signed);
    }
    // /Info (auteur, logiciel, dates), /ID (empreinte du document), /Metadata (XMP).
    // ATTENTION : retirer la RÉFÉRENCE ne suffit pas — l'OBJET resterait dans le
    // fichier sauvegardé, octets confidentiels compris. Supprimer les objets, puis
    // élaguer les orphelins (flux XMP imbriqués, etc.). On mémorise si QUELQUE CHOSE
    // a réellement été retiré : sinon on ne réécrit pas (une re-sérialisation lopdf
    // d'un PDF exotique pourrait le corrompre et serait livrée comme « Cleaned »).
    let mut changed = false;
    if let Ok(info_id) = doc.trailer.get(b"Info").and_then(|o| o.as_reference()) {
        doc.objects.remove(&info_id);
    }
    if doc.trailer.remove(b"Info").is_some() {
        changed = true;
    }
    if doc.trailer.remove(b"ID").is_some() {
        changed = true;
    }
    if let Ok(root_id) = doc.trailer.get(b"Root").and_then(|o| o.as_reference()) {
        let meta_id = doc
            .get_object(root_id)
            .ok()
            .and_then(|o| o.as_dict().ok())
            .and_then(|d| d.get(b"Metadata").ok())
            .and_then(|o| o.as_reference().ok());
        if let Some(mid) = meta_id {
            doc.objects.remove(&mid);
        }
        if let Ok(obj) = doc.get_object_mut(root_id) {
            if let Ok(dict) = obj.as_dict_mut() {
                if dict.remove(b"Metadata").is_some() {
                    changed = true;
                }
                if dict.remove(b"PieceInfo").is_some() {
                    changed = true;
                }
            }
        }
    }
    if !changed {
        return Ok(PdfOutcome::Untouched);
    }
    doc.prune_objects();
    doc.save(out).map_err(|e| e.to_string())?;
    Ok(PdfOutcome::Cleaned)
}

// ---- Documents ZIP : OOXML (docx/xlsx/pptx) et OpenDocument (odt/ods/odp) ----

#[derive(Clone, Copy)]
enum ZipDoc {
    Ooxml,
    Odf,
}

const BLANK_CORE: &str = r#"<?xml version="1.0" encoding="UTF-8" standalone="yes"?><cp:coreProperties xmlns:cp="http://schemas.openxmlformats.org/package/2006/metadata/core-properties" xmlns:dc="http://purl.org/dc/elements/1.1/" xmlns:dcterms="http://purl.org/dc/terms/" xmlns:dcmitype="http://purl.org/dc/dcmitype/" xmlns:xsi="http://www.w3.org/2001/XMLSchema-instance"/>"#;
const BLANK_APP: &str = r#"<?xml version="1.0" encoding="UTF-8" standalone="yes"?><Properties xmlns="http://schemas.openxmlformats.org/officeDocument/2006/extended-properties" xmlns:vt="http://schemas.openxmlformats.org/officeDocument/2006/docPropsVTypes"/>"#;
const BLANK_CUSTOM: &str = r#"<?xml version="1.0" encoding="UTF-8" standalone="yes"?><Properties xmlns="http://schemas.openxmlformats.org/officeDocument/2006/custom-properties" xmlns:vt="http://schemas.openxmlformats.org/officeDocument/2006/docPropsVTypes"/>"#;
const BLANK_ODF_META: &str = r#"<?xml version="1.0" encoding="UTF-8"?><office:document-meta xmlns:office="urn:oasis:names:tc:opendocument:xmlns:office:1.0" office:version="1.2"><office:meta/></office:document-meta>"#;

fn clean_zip_doc(path: &Path, size: u64, kind: ZipDoc) -> Prep {
    if size > MAX_IN_MEMORY {
        return Prep::Skipped("trop volumineux pour le nettoyage");
    }
    let tmp = temp_path(path);
    match clean_zip_inner(path, &tmp, kind) {
        Ok(true) => Prep::Cleaned(tmp),
        Ok(false) => {
            let _ = fs::remove_file(&tmp);
            Prep::Untouched
        }
        Err(e) => {
            let _ = fs::remove_file(&tmp);
            Prep::Failed(format!("document: {e}"))
        }
    }
}

fn clean_zip_inner(path: &Path, out: &Path, kind: ZipDoc) -> Result<bool, String> {
    let f = fs::File::open(path).map_err(|e| e.to_string())?;
    let mut zin = zip::ZipArchive::new(f).map_err(|e| e.to_string())?;
    let fo = fs::File::create(out).map_err(|e| e.to_string())?;
    let mut zout = zip::ZipWriter::new(fo);
    let mut changed = false;
    for i in 0..zin.len() {
        let name = zin
            .by_index_raw(i)
            .map_err(|e| e.to_string())?
            .name()
            .to_string();
        let replacement = match kind {
            ZipDoc::Ooxml => match name.as_str() {
                "docProps/core.xml" => Some(BLANK_CORE),
                "docProps/app.xml" => Some(BLANK_APP),
                "docProps/custom.xml" => Some(BLANK_CUSTOM),
                _ => None,
            },
            ZipDoc::Odf => (name == "meta.xml").then_some(BLANK_ODF_META),
        };
        match replacement {
            Some(xml) => {
                // Remplacer (et non retirer) : l'entrée reste référencée par
                // [Content_Types].xml / manifest.xml, la retirer casserait le document.
                zout.start_file(name, zip::write::SimpleFileOptions::default())
                    .map_err(|e| e.to_string())?;
                zout.write_all(xml.as_bytes()).map_err(|e| e.to_string())?;
                changed = true;
            }
            None => {
                let entry = zin.by_index_raw(i).map_err(|e| e.to_string())?;
                zout.raw_copy_file(entry).map_err(|e| e.to_string())?;
            }
        }
    }
    zout.finish().map_err(|e| e.to_string())?;
    Ok(changed)
}

// ---- Tests (fichiers synthétiques : la logique octet-par-octet est sensible) ----

#[cfg(test)]
mod tests {
    use super::*;

    fn seg(marker: u8, payload: &[u8]) -> Vec<u8> {
        let mut v = vec![0xFF, marker];
        v.extend_from_slice(&((payload.len() + 2) as u16).to_be_bytes());
        v.extend_from_slice(payload);
        v
    }

    #[test]
    fn jpeg_strips_exif_and_trailer() {
        let mut d = vec![0xFF, 0xD8];
        d.extend(seg(0xE0, b"JFIF\0rest")); // APP0 gardé
        d.extend(seg(0xE1, b"Exif\0\0SECRETGPS")); // APP1 jeté
        d.extend(seg(0xDB, &[0u8; 4])); // DQT gardé
        d.extend(seg(0xDA, &[1, 2])); // SOS
        d.extend_from_slice(&[0x12, 0xFF, 0x00, 0x34, 0xFF, 0xD9]); // entropie + EOI
        d.extend_from_slice(b"MOTIONVIDEO"); // trailer après EOI (coupé)
        let out = clean_jpeg(&d).unwrap().expect("doit changer");
        let hay = |h: &[u8], n: &[u8]| h.windows(n.len()).any(|w| w == n);
        assert!(!hay(&out, b"SECRETGPS"));
        assert!(!hay(&out, b"MOTIONVIDEO"));
        assert!(hay(&out, b"JFIF"));
        assert!(out.ends_with(&[0xFF, 0xD9]));
    }

    #[test]
    fn jpeg_clean_is_untouched() {
        let mut d = vec![0xFF, 0xD8];
        d.extend(seg(0xDB, &[0u8; 4]));
        d.extend(seg(0xDA, &[1, 2]));
        d.extend_from_slice(&[0x12, 0xFF, 0xD9]);
        assert!(clean_jpeg(&d).unwrap().is_none());
    }

    #[test]
    fn jpeg_trailing_ff_padding_errors_not_panics() {
        // Fichier tronqué en plein bourrage 0xFF : doit renvoyer Err, pas paniquer.
        assert!(clean_jpeg(&[0xFF, 0xD8, 0xFF, 0xFF]).is_err());
        assert!(clean_jpeg(&[0xFF, 0xD8, 0xFF, 0xFF, 0xFF, 0xFF]).is_err());
    }

    #[test]
    fn jpeg_progressive_strips_app1_between_scans() {
        // JPEG progressif : un APP1/EXIF glissé ENTRE deux scans doit aussi sauter.
        let mut d = vec![0xFF, 0xD8];
        d.extend(seg(0xDB, &[0u8; 4]));
        d.extend(seg(0xDA, &[1])); // scan 1
        d.extend_from_slice(&[0x11, 0xFF, 0x00, 0x22]); // entropie (FF stuffé)
        d.extend(seg(0xE1, b"Exif\0\0LATEGPS")); // APP1 entre les scans
        d.extend(seg(0xDA, &[2])); // scan 2
        d.extend_from_slice(&[0x33, 0xFF, 0xD9]);
        let out = clean_jpeg(&d).unwrap().expect("doit changer");
        let hay = |h: &[u8], n: &[u8]| h.windows(n.len()).any(|w| w == n);
        assert!(!hay(&out, b"LATEGPS"));
        assert!(hay(&out, &[0x11, 0xFF, 0x00, 0x22])); // entropie du scan 1 intacte
        assert!(out.ends_with(&[0x33, 0xFF, 0xD9])); // scan 2 + EOI intacts
    }

    fn png_chunk(typ: &[u8; 4], data: &[u8]) -> Vec<u8> {
        let mut v = (data.len() as u32).to_be_bytes().to_vec();
        v.extend_from_slice(typ);
        v.extend_from_slice(data);
        v.extend_from_slice(&[0u8; 4]); // CRC non vérifié
        v
    }

    #[test]
    fn png_strips_text_and_private_chunks() {
        let mut d = PNG_SIG.to_vec();
        d.extend(png_chunk(b"IHDR", &[0u8; 13]));
        d.extend(png_chunk(b"tEXt", b"Author\0Someone"));
        d.extend(png_chunk(b"iDOT", &[9, 9, 9])); // chunk privé Apple : jeté aussi
        d.extend(png_chunk(b"tRNS", &[0])); // auxiliaire utile : gardé
        d.extend(png_chunk(b"IDAT", &[1, 2, 3]));
        d.extend(png_chunk(b"IEND", &[]));
        let out = clean_png(&d).unwrap().expect("doit changer");
        assert!(!out.windows(4).any(|w| w == b"tEXt"));
        assert!(!out.windows(4).any(|w| w == b"iDOT"));
        assert!(out.windows(4).any(|w| w == b"tRNS"));
        assert!(out.windows(4).any(|w| w == b"IDAT"));
        assert!(out.windows(4).any(|w| w == b"IEND"));
    }

    fn riff_chunk(id: &[u8; 4], data: &[u8]) -> Vec<u8> {
        let mut v = id.to_vec();
        v.extend_from_slice(&(data.len() as u32).to_le_bytes());
        v.extend_from_slice(data);
        if data.len() & 1 == 1 {
            v.push(0);
        }
        v
    }

    #[test]
    fn webp_strips_exif_and_patches_vp8x() {
        let mut chunks = riff_chunk(b"VP8X", &[0x0C | 0x10, 0, 0, 0, 9, 0, 0, 5, 0, 0]);
        chunks.extend(riff_chunk(b"VP8 ", &[9, 9, 9, 9]));
        chunks.extend(riff_chunk(b"EXIF", b"SECRETGPS"));
        let mut d = b"RIFF".to_vec();
        d.extend_from_slice(&((4 + chunks.len()) as u32).to_le_bytes());
        d.extend_from_slice(b"WEBP");
        d.extend_from_slice(&chunks);
        let out = clean_webp(&d).unwrap().expect("doit changer");
        assert!(!out.windows(9).any(|w| w == b"SECRETGPS"));
        // bits EXIF/XMP éteints, bit alpha (0x10) conservé
        let vp8x_flags = out[12 + 8];
        assert_eq!(vp8x_flags & 0x0C, 0);
        assert_eq!(vp8x_flags & 0x10, 0x10);
        // taille RIFF cohérente
        let sz = u32::from_le_bytes(out[4..8].try_into().unwrap()) as usize;
        assert_eq!(sz + 8, out.len());
    }

    #[test]
    fn wav_strips_info_list() {
        let mut chunks = riff_chunk(b"fmt ", &[0u8; 16]);
        chunks.extend(riff_chunk(b"LIST", b"INFOIART\x04\0\0\0Bob\0"));
        chunks.extend(riff_chunk(b"data", &[1, 2, 3, 4]));
        let mut d = b"RIFF".to_vec();
        d.extend_from_slice(&((4 + chunks.len()) as u32).to_le_bytes());
        d.extend_from_slice(b"WAVE");
        d.extend_from_slice(&chunks);
        let out = clean_wav(&d).unwrap().expect("doit changer");
        assert!(!out.windows(4).any(|w| w == b"INFO"));
        assert!(out.windows(4).any(|w| w == b"data"));
    }

    #[test]
    fn mp3_strips_id3_both_ends() {
        let mut d = b"ID3\x04\x00\x00\x00\x00\x00\x0A".to_vec(); // taille syncsafe = 10
        d.extend_from_slice(&[0u8; 10]); // corps du tag
        d.extend_from_slice(&[0xFF, 0xFB, 1, 2, 3, 4]); // « audio »
        let mut v1 = b"TAG".to_vec();
        v1.extend_from_slice(&[0u8; 125]);
        d.extend_from_slice(&v1);
        let out = clean_mp3(&d).unwrap().expect("doit changer");
        assert_eq!(out, vec![0xFF, 0xFB, 1, 2, 3, 4]);
    }

    #[test]
    fn png_oversized_chunk_len_errors_not_panics() {
        // Longueur de chunk annoncée = 0xFFFFFFFF : les bornes (checked_add + fin de
        // fichier) doivent renvoyer Err, sans déborder ni paniquer.
        let mut d = PNG_SIG.to_vec();
        d.extend_from_slice(&0xFFFF_FFFFu32.to_be_bytes()); // len énorme
        d.extend_from_slice(b"tEXt");
        d.extend_from_slice(&[0u8; 8]); // données bien plus courtes qu'annoncé
        assert!(clean_png(&d).is_err());
    }

    #[test]
    fn riff_oversized_chunk_errors_not_panics() {
        // Taille de chunk RIFF dépassant le fichier : Err propre, pas de panique.
        let mut d = b"RIFF".to_vec();
        d.extend_from_slice(&100u32.to_le_bytes()); // taille RIFF (indifférente ici)
        d.extend_from_slice(b"WAVE");
        d.extend_from_slice(b"data");
        d.extend_from_slice(&0xFFFF_FFFFu32.to_le_bytes()); // sz > fichier
        d.extend_from_slice(&[0u8; 4]);
        assert!(clean_wav(&d).is_err());
    }

    #[test]
    fn mp3_truncated_id3v2_errors_not_panics() {
        // En-tête ID3v2 annonçant une taille (syncsafe = 128) > fichier : Err, pas panic.
        let mut d = b"ID3\x04\x00\x00\x00\x00\x01\x00".to_vec(); // taille syncsafe = 128
        d.extend_from_slice(&[0u8; 4]); // corps bien plus court que 128
        assert!(clean_mp3(&d).is_err());
    }

    #[test]
    fn mp3_strips_apev2_footer() {
        // Audio suivi d'un tag APEv2 (pied seul, 32 o) : le tag doit être retiré.
        let mut d = vec![0xFF, 0xFB, 1, 2, 3, 4]; // « audio »
        let mut footer = b"APETAGEX".to_vec(); // préambule (8)
        footer.extend_from_slice(&2000u32.to_le_bytes()); // version (4)
        footer.extend_from_slice(&32u32.to_le_bytes()); // tag_size = pied seul (4)
        footer.extend_from_slice(&0u32.to_le_bytes()); // nombre d'items (4)
        footer.extend_from_slice(&0u32.to_le_bytes()); // flags : bit31=0 → pas d'en-tête (4)
        footer.extend_from_slice(&[0u8; 8]); // réservé (8)
        assert_eq!(footer.len(), 32);
        d.extend_from_slice(&footer);
        let out = clean_mp3(&d).unwrap().expect("doit changer");
        assert_eq!(out, vec![0xFF, 0xFB, 1, 2, 3, 4]);
    }

    fn mp4_box(typ: &[u8; 4], payload: &[u8]) -> Vec<u8> {
        let mut v = ((payload.len() + 8) as u32).to_be_bytes().to_vec();
        v.extend_from_slice(typ);
        v.extend_from_slice(payload);
        v
    }

    #[test]
    fn mp4_frees_udta_in_moov() {
        let udta = mp4_box(b"udta", b"\x00\x00\x00\x10meta GPS \xa9xyz!");
        let mvhd = mp4_box(b"mvhd", &[0u8; 20]);
        let moov = mp4_box(b"moov", &[mvhd, udta].concat());
        let mut d = mp4_box(b"ftyp", b"isom\0\0\0\0isom");
        d.extend_from_slice(&moov);
        d.extend_from_slice(&mp4_box(b"mdat", &[7u8; 16]));
        let dir = std::env::temp_dir().join("ghostlink-clean-test");
        let _ = fs::create_dir_all(&dir);
        let src = dir.join("in.mp4");
        fs::write(&src, &d).unwrap();
        match clean_mp4_file(&src, d.len() as u64) {
            Prep::Cleaned(tmp) => {
                let out = fs::read(&tmp).unwrap();
                assert_eq!(out.len(), d.len());
                assert!(!out.windows(4).any(|w| w == b"udta"));
                assert!(out.windows(4).any(|w| w == b"free"));
                assert!(out.windows(4).any(|w| w == b"mdat"));
                // Le CONTENU doit être zéroté, pas seulement le type renommé —
                // sinon les octets GPS resteraient lisibles dans le fichier envoyé.
                assert!(!out.windows(3).any(|w| w == b"GPS"));
                assert!(!out.windows(4).any(|w| w == b"\xa9xyz"));
                // La charge utile du mdat, elle, est intacte.
                assert!(out.windows(4).any(|w| w == [7u8, 7, 7, 7]));
                let _ = fs::remove_file(tmp);
            }
            _ => panic!("le mp4 aurait dû être nettoyé"),
        }
        let _ = fs::remove_file(src);
    }

    #[test]
    fn mp4_forged_largesize_errors_not_panics() {
        // Atome à taille étendue 64 bits forgée (u64::MAX) précédé d'un ftyp valide :
        // l'addition pos+total déborderait — doit renvoyer Failed, pas paniquer ni
        // contourner les bornes.
        let mut d = mp4_box(b"ftyp", b"isom\0\0\0\0isom");
        d.extend_from_slice(&1u32.to_be_bytes()); // size==1 → largesize
        d.extend_from_slice(b"moov");
        d.extend_from_slice(&u64::MAX.to_be_bytes());
        d.extend_from_slice(&[0u8; 16]);
        let dir = std::env::temp_dir().join("ghostlink-clean-test");
        let _ = fs::create_dir_all(&dir);
        let src = dir.join("forged.mp4");
        fs::write(&src, &d).unwrap();
        match clean_mp4_file(&src, d.len() as u64) {
            Prep::Failed(_) => {}
            _ => panic!("un largesize forgé doit échouer proprement"),
        }
        let _ = fs::remove_file(src);
    }

    #[test]
    fn ooxml_blanks_core_props() {
        let dir = std::env::temp_dir().join("ghostlink-clean-test");
        let _ = fs::create_dir_all(&dir);
        let src = dir.join("in.docx");
        {
            let f = fs::File::create(&src).unwrap();
            let mut w = zip::ZipWriter::new(f);
            let o = zip::write::SimpleFileOptions::default();
            w.start_file("[Content_Types].xml", o).unwrap();
            w.write_all(b"<Types/>").unwrap();
            w.start_file("docProps/core.xml", o).unwrap();
            w.write_all(b"<cp:coreProperties><dc:creator>Jules SECRET</dc:creator></cp:coreProperties>")
                .unwrap();
            w.start_file("word/document.xml", o).unwrap();
            w.write_all(b"<w:document>hello</w:document>").unwrap();
            w.finish().unwrap();
        }
        match clean_zip_doc(&src, fs::metadata(&src).unwrap().len(), ZipDoc::Ooxml) {
            Prep::Cleaned(tmp) => {
                let mut z = zip::ZipArchive::new(fs::File::open(&tmp).unwrap()).unwrap();
                let mut core = String::new();
                std::io::Read::read_to_string(&mut z.by_name("docProps/core.xml").unwrap(), &mut core).unwrap();
                assert!(!core.contains("SECRET"));
                let mut doc = String::new();
                std::io::Read::read_to_string(&mut z.by_name("word/document.xml").unwrap(), &mut doc).unwrap();
                assert!(doc.contains("hello"));
                let _ = fs::remove_file(tmp);
            }
            _ => panic!("le docx aurait dû être nettoyé"),
        }
        let _ = fs::remove_file(src);
    }

    #[test]
    fn pdf_drops_info() {
        use lopdf::{dictionary, Document, Object};
        let dir = std::env::temp_dir().join("ghostlink-clean-test");
        let _ = fs::create_dir_all(&dir);
        let src = dir.join("in.pdf");
        let mut doc = Document::with_version("1.5");
        let pages_id = doc.new_object_id();
        let page_id = doc.add_object(dictionary! {
            "Type" => "Page",
            "Parent" => pages_id,
        });
        doc.objects.insert(
            pages_id,
            Object::Dictionary(dictionary! {
                "Type" => "Pages",
                "Kids" => vec![page_id.into()],
                "Count" => 1,
            }),
        );
        let catalog_id = doc.add_object(dictionary! {
            "Type" => "Catalog",
            "Pages" => pages_id,
        });
        let info_id = doc.add_object(dictionary! {
            "Author" => Object::string_literal("Jules SECRET"),
        });
        doc.trailer.set("Root", catalog_id);
        doc.trailer.set("Info", info_id);
        doc.save(&src).unwrap();
        // #12 : le nettoyage réel tourne dans clean_pdf_worker (exécuté par le sous-
        // processus isolé en production). On le teste DIRECTEMENT — clean_pdf_file re-exec
        // le vrai binaire de l'app, indisponible sous `cargo test` (current_exe = binaire
        // de test). clean_pdf_worker écrit le fichier nettoyé et renvoie 0 = Cleaned.
        let tmp = temp_path(&src);
        let code = clean_pdf_worker(&src.to_string_lossy(), &tmp.to_string_lossy());
        assert_eq!(code, 0, "le pdf aurait dû être nettoyé (code {code})");
        let cleaned = Document::load(&tmp).unwrap();
        assert!(cleaned.trailer.get(b"Info").is_err());
        let raw = fs::read(&tmp).unwrap();
        assert!(!raw.windows(6).any(|w| w == b"SECRET"));
        let _ = fs::remove_file(tmp);
        let _ = fs::remove_file(src);
    }
}
