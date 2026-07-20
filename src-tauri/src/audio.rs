// Audio temps réel (cpal + Opus).
// Opus tourne TOUJOURS en 48 kHz mono. On convertit n'importe quel format de
// périphérique (f32/i16/u16) et on ré-échantillonne automatiquement vers/depuis 48 kHz,
// pour que l'appel fonctionne quelle que soit la carte son (ex. micro en 44,1 kHz).

use std::collections::{HashMap, VecDeque};
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::{Arc, Mutex};
use std::time::Instant;

use cpal::traits::{DeviceTrait, HostTrait, StreamTrait};
use cpal::SampleFormat;
use iroh::endpoint::Connection;
use tauri::{AppHandle, Emitter};

/// Opus fonctionne à 48 kHz mono, trame de 20 ms = 960 échantillons.
const OPUS_FRAME: usize = 960;
const VOICE_TAG: u8 = 1; // premier octet d'un datagramme = type « voix »
const SCREEN_TAG: u8 = 2; // premier octet d'un datagramme = « son d'écran » (process-loopback)
const CALL_PING: u8 = 3; // balise « je suis dans l'appel » (indépendante de la parole)
/// Seuil de crête au-dessus duquel une trame voix compte comme « parle ».
const SPEAK_PEAK: f32 = 0.02;

/// Activité vocale d'un participant : dernier instant où il a PARLÉ (voix > seuil) et
/// dernière balise CALL_PING reçue. Sert à l'indicateur « en appel / parle » de l'UI.
#[derive(Clone, Copy, Default)]
struct VoiceMark {
    last_voice: Option<Instant>,
    last_ping: Option<Instant>,
}
/// Activité par code de pair (+ clé « me » pour mon propre micro).
type VoiceAct = Arc<Mutex<HashMap<String, VoiceMark>>>;

/// Clé du tampon de mixage pour le SON D'ÉCRAN d'un pair — distincte de sa voix pour
/// que chaque flux garde SON décodeur Opus (les codes de pair ne contiennent pas `#`).
fn screen_mix_key(peer: &str) -> String {
    format!("{peer}#écran")
}

// ---- Conversions de format d'échantillon ----
fn f32_in(s: f32) -> f32 { s }
fn i16_in(s: i16) -> f32 { s as f32 / 32768.0 }
fn u16_in(s: u16) -> f32 { (s as f32 - 32768.0) / 32768.0 }
fn f32_out(s: f32) -> f32 { s }
fn i16_out(s: f32) -> i16 { (s.clamp(-1.0, 1.0) * 32767.0) as i16 }
fn u16_out(s: f32) -> u16 { ((s.clamp(-1.0, 1.0) * 32767.0) + 32768.0) as u16 }

/// Ré-échantillonne un bloc mono `input` vers exactement `out_len` échantillons
/// (interpolation linéaire). Suffisant pour de la voix.
fn resample_block(input: &[f32], out_len: usize) -> Vec<f32> {
    let n = input.len();
    if out_len == 0 {
        return Vec::new();
    }
    if n == 0 {
        return vec![0.0; out_len];
    }
    if n == out_len {
        return input.to_vec();
    }
    if n == 1 {
        return vec![input[0]; out_len];
    }
    let mut out = Vec::with_capacity(out_len);
    let step = (n - 1) as f32 / (out_len - 1) as f32;
    for i in 0..out_len {
        let x = i as f32 * step;
        let j = x.floor() as usize;
        let frac = x - j as f32;
        let a = input[j];
        let b = if j + 1 < n { input[j + 1] } else { a };
        out.push(a + (b - a) * frac);
    }
    out
}

/// Périphériques audio choisis (None = périphérique par défaut du système).
#[derive(Clone, Default)]
pub struct AudioCfg {
    input: Arc<Mutex<Option<String>>>,
    output: Arc<Mutex<Option<String>>>,
}
impl AudioCfg {
    pub fn set_input(&self, name: Option<String>) {
        *self.input.lock().unwrap_or_else(|e| e.into_inner()) = name.filter(|s| !s.trim().is_empty());
    }
    pub fn set_output(&self, name: Option<String>) {
        *self.output.lock().unwrap_or_else(|e| e.into_inner()) = name.filter(|s| !s.trim().is_empty());
    }
    fn input_name(&self) -> Option<String> {
        self.input.lock().unwrap_or_else(|e| e.into_inner()).clone()
    }
    fn output_name(&self) -> Option<String> {
        self.output.lock().unwrap_or_else(|e| e.into_inner()).clone()
    }
}

/// Liste les périphériques disponibles : (entrées, sorties), par nom.
pub fn list_devices() -> (Vec<String>, Vec<String>) {
    let host = cpal::default_host();
    let inputs = host
        .input_devices()
        .map(|it| it.filter_map(|d| d.name().ok()).collect())
        .unwrap_or_default();
    let outputs = host
        .output_devices()
        .map(|it| it.filter_map(|d| d.name().ok()).collect())
        .unwrap_or_default();
    (inputs, outputs)
}

fn pick_input(host: &cpal::Host, name: &Option<String>) -> Option<cpal::Device> {
    if let Some(n) = name {
        if let Ok(devs) = host.input_devices() {
            for d in devs {
                if d.name().map(|x| &x == n).unwrap_or(false) {
                    return Some(d);
                }
            }
        }
    }
    host.default_input_device()
}

fn pick_output(host: &cpal::Host, name: &Option<String>) -> Option<cpal::Device> {
    if let Some(n) = name {
        if let Ok(devs) = host.output_devices() {
            for d in devs {
                if d.name().map(|x| &x == n).unwrap_or(false) {
                    return Some(d);
                }
            }
        }
    }
    host.default_output_device()
}

/// Construit le flux d'entrée (micro) : convertit le format natif → f32, downmix mono → `in_buf`.
fn build_input(
    device: &cpal::Device,
    config: &cpal::StreamConfig,
    fmt: SampleFormat,
    in_buf: Arc<Mutex<VecDeque<f32>>>,
    ch: usize,
    in_cap: usize,
    dev_lost: Arc<AtomicBool>,
) -> anyhow::Result<cpal::Stream> {
    macro_rules! build {
        ($t:ty, $conv:path) => {{
            let dev_lost = dev_lost.clone();
            device.build_input_stream(
                config,
                move |data: &[$t], _: &cpal::InputCallbackInfo| {
                    if let Ok(mut q) = in_buf.lock() {
                        for chunk in data.chunks(ch) {
                            let m: f32 = chunk.iter().map(|&s| $conv(s)).sum::<f32>() / ch as f32;
                            q.push_back(m);
                        }
                        while q.len() > in_cap {
                            q.pop_front();
                        }
                    }
                },
                // Erreur cpal (périphérique débranché, défaut pilote) : lever un drapeau
                // que la boucle de traitement observe (sinon l'appel devient un zombie
                // silencieux — flux mort mais boucle qui tourne).
                move |e| {
                    eprintln!("erreur flux micro: {e}");
                    dev_lost.store(true, Ordering::SeqCst);
                },
                None,
            )
        }};
    }
    let stream = match fmt {
        SampleFormat::F32 => build!(f32, f32_in),
        SampleFormat::I16 => build!(i16, i16_in),
        SampleFormat::U16 => build!(u16, u16_in),
        other => return Err(anyhow::anyhow!("format micro non géré: {other:?}")),
    }
    .map_err(|e| anyhow::anyhow!("ouverture du flux micro: {e}"))?;
    Ok(stream)
}

/// Construit le flux de sortie (haut-parleur) : lit `out_buf` (mono f32), upmix → format natif.
fn build_output(
    device: &cpal::Device,
    config: &cpal::StreamConfig,
    fmt: SampleFormat,
    out_buf: Arc<Mutex<VecDeque<f32>>>,
    ch: usize,
    dev_lost: Arc<AtomicBool>,
) -> anyhow::Result<cpal::Stream> {
    macro_rules! build {
        ($t:ty, $conv:path) => {{
            let dev_lost = dev_lost.clone();
            device.build_output_stream(
                config,
                move |data: &mut [$t], _: &cpal::OutputCallbackInfo| {
                    if let Ok(mut q) = out_buf.lock() {
                        for chunk in data.chunks_mut(ch) {
                            let v = $conv(q.pop_front().unwrap_or(0.0));
                            for x in chunk.iter_mut() {
                                *x = v;
                            }
                        }
                    } else {
                        let z = $conv(0.0);
                        for x in data.iter_mut() {
                            *x = z;
                        }
                    }
                },
                // Voir build_input : périphérique de sortie perdu → drapeau observé par
                // la boucle de traitement.
                move |e| {
                    eprintln!("erreur flux sortie: {e}");
                    dev_lost.store(true, Ordering::SeqCst);
                },
                None,
            )
        }};
    }
    let stream = match fmt {
        SampleFormat::F32 => build!(f32, f32_out),
        SampleFormat::I16 => build!(i16, i16_out),
        SampleFormat::U16 => build!(u16, u16_out),
        other => return Err(anyhow::anyhow!("format sortie non géré: {other:?}")),
    }
    .map_err(|e| anyhow::anyhow!("ouverture du flux sortie: {e}"))?;
    Ok(stream)
}

/// Ouvre le flux micro en essayant la config par défaut PUIS toutes les configs
/// supportées (en préférant 48 kHz), jusqu'à ce qu'une s'ouvre vraiment. Renvoie (flux, taux).
/// Indispensable pour les périphériques (casques gaming Chat/Game, etc.) dont la config
/// « par défaut » annoncée n'est pas réellement ouvrable.
fn open_input_stream(
    device: &cpal::Device,
    in_buf: Arc<Mutex<VecDeque<f32>>>,
    dev_lost: Arc<AtomicBool>,
) -> anyhow::Result<(cpal::Stream, u32)> {
    let mut candidates: Vec<cpal::SupportedStreamConfig> = Vec::new();
    if let Ok(def) = device.default_input_config() {
        candidates.push(def);
    }
    if let Ok(list) = device.supported_input_configs() {
        for range in list {
            let sr = if range.min_sample_rate().0 <= 48000 && 48000 <= range.max_sample_rate().0 {
                cpal::SampleRate(48000)
            } else {
                range.max_sample_rate()
            };
            candidates.push(range.with_sample_rate(sr));
        }
    }
    let mut last = anyhow::anyhow!("aucune configuration micro disponible");
    for sup in candidates {
        let fmt = sup.sample_format();
        let config: cpal::StreamConfig = sup.into();
        let ch = (config.channels as usize).max(1);
        let rate = config.sample_rate.0;
        let in_cap = (rate as usize / 50) * 50;
        match build_input(device, &config, fmt, in_buf.clone(), ch, in_cap, dev_lost.clone()) {
            Ok(s) => return Ok((s, rate)),
            Err(e) => last = e,
        }
    }
    Err(last)
}

/// Idem pour la sortie (haut-parleur). Renvoie (flux, taux).
fn open_output_stream(
    device: &cpal::Device,
    out_buf: Arc<Mutex<VecDeque<f32>>>,
    dev_lost: Arc<AtomicBool>,
) -> anyhow::Result<(cpal::Stream, u32)> {
    let mut candidates: Vec<cpal::SupportedStreamConfig> = Vec::new();
    if let Ok(def) = device.default_output_config() {
        candidates.push(def);
    }
    if let Ok(list) = device.supported_output_configs() {
        for range in list {
            let sr = if range.min_sample_rate().0 <= 48000 && 48000 <= range.max_sample_rate().0 {
                cpal::SampleRate(48000)
            } else {
                range.max_sample_rate()
            };
            candidates.push(range.with_sample_rate(sr));
        }
    }
    let mut last = anyhow::anyhow!("aucune configuration sortie disponible");
    for sup in candidates {
        let fmt = sup.sample_format();
        let config: cpal::StreamConfig = sup.into();
        let ch = (config.channels as usize).max(1);
        let rate = config.sample_rate.0;
        match build_output(device, &config, fmt, out_buf.clone(), ch, dev_lost.clone()) {
            Ok(s) => return Ok((s, rate)),
            Err(e) => last = e,
        }
    }
    Err(last)
}

fn new_encoder() -> anyhow::Result<audiopus::coder::Encoder> {
    audiopus::coder::Encoder::new(
        audiopus::SampleRate::Hz48000,
        audiopus::Channels::Mono,
        audiopus::Application::Voip,
    )
    .map_err(|e| anyhow::anyhow!("init encodeur Opus: {e:?}"))
}

fn new_decoder() -> anyhow::Result<audiopus::coder::Decoder> {
    audiopus::coder::Decoder::new(audiopus::SampleRate::Hz48000, audiopus::Channels::Mono)
        .map_err(|e| anyhow::anyhow!("init décodeur Opus: {e:?}"))
}

/// Encodeur pour le SON D'ÉCRAN : profil « Audio » (musique/vidéo), pas « Voip ».
fn new_screen_encoder() -> anyhow::Result<audiopus::coder::Encoder> {
    audiopus::coder::Encoder::new(
        audiopus::SampleRate::Hz48000,
        audiopus::Channels::Mono,
        audiopus::Application::Audio,
    )
    .map_err(|e| anyhow::anyhow!("init encodeur Opus (écran): {e:?}"))
}

/// État partagé du test vocal : contient le drapeau d'arrêt de la boucle en cours (s'il y en a une).
#[derive(Clone, Default)]
pub struct Voice {
    flag: Arc<Mutex<Option<Arc<AtomicBool>>>>,
}

impl Voice {
    /// Démarre (ou redémarre) la boucle locale micro→Opus→haut-parleur.
    pub fn start(&self, cfg: AudioCfg) -> anyhow::Result<()> {
        self.stop();
        let f = start_loopback(cfg)?;
        *self.flag.lock().unwrap_or_else(|e| e.into_inner()) = Some(f);
        Ok(())
    }
    /// Coupe la boucle en cours (s'il y en a une).
    pub fn stop(&self) {
        if let Some(f) = self.flag.lock().unwrap_or_else(|e| e.into_inner()).take() {
            f.store(true, Ordering::SeqCst);
        }
    }
}

/// Lance la boucle audio locale (à travers Opus). Renvoie un drapeau : le passer à `true` coupe la boucle.
fn start_loopback(cfg: AudioCfg) -> anyhow::Result<Arc<AtomicBool>> {
    let stop = Arc::new(AtomicBool::new(false));
    let stop_thread = stop.clone();
    let (tx, rx) = std::sync::mpsc::channel::<Result<(), String>>();

    // cpal::Stream n'est pas `Send` : on construit ET on garde les streams sur CE thread,
    // qui exécute aussi le traitement Opus (hors des callbacks audio temps réel).
    std::thread::spawn(move || {
        type Setup = (
            cpal::Stream,
            cpal::Stream,
            Arc<Mutex<VecDeque<f32>>>, // tampon d'entrée (mono @ in_rate)
            Arc<Mutex<VecDeque<f32>>>, // tampon de sortie (mono @ out_rate)
            usize,                     // trame d'entrée (échantillons @ in_rate)
            usize,                     // trame de sortie (échantillons @ out_rate)
            audiopus::coder::Encoder,
            audiopus::coder::Decoder,
        );
        // Levé par le callback d'erreur cpal (périphérique perdu) → coupe la boucle.
        let dev_lost = Arc::new(AtomicBool::new(false));
        let setup = (|| -> anyhow::Result<Setup> {
            let host = cpal::default_host();
            let input = pick_input(&host, &cfg.input_name())
                .ok_or_else(|| anyhow::anyhow!("aucun micro détecté"))?;
            let output = pick_output(&host, &cfg.output_name())
                .ok_or_else(|| anyhow::anyhow!("aucune sortie audio détectée"))?;

            let in_buf: Arc<Mutex<VecDeque<f32>>> = Arc::new(Mutex::new(VecDeque::new()));
            let out_buf: Arc<Mutex<VecDeque<f32>>> = Arc::new(Mutex::new(VecDeque::new()));

            let (input_stream, in_rate) = open_input_stream(&input, in_buf.clone(), dev_lost.clone())?;
            let (output_stream, out_rate) = open_output_stream(&output, out_buf.clone(), dev_lost.clone())?;

            let in_frame = (in_rate as usize) / 50; // 20 ms @ in_rate
            let out_frame = (out_rate as usize) / 50; // 20 ms @ out_rate
            // Petit coussin (~100 ms) en sortie pour lisser.
            out_buf
                .lock()
                .unwrap()
                .extend(std::iter::repeat_n(0.0f32, out_frame * 5));

            let encoder = new_encoder()?;
            let decoder = new_decoder()?;

            input_stream.play()?;
            output_stream.play()?;
            Ok((
                input_stream,
                output_stream,
                in_buf,
                out_buf,
                in_frame,
                out_frame,
                encoder,
                decoder,
            ))
        })();

        match setup {
            Ok((_in_s, _out_s, in_buf, out_buf, in_frame, out_frame, encoder, mut decoder)) => {
                let _ = tx.send(Ok(()));
                let out_cap = out_frame * 50;
                let mut packet = [0u8; 4000];
                let mut framebuf = vec![0f32; in_frame];
                let mut decoded = vec![0f32; OPUS_FRAME];
                while !stop_thread.load(Ordering::SeqCst) {
                    if dev_lost.load(Ordering::SeqCst) {
                        break; // périphérique perdu : inutile de tourner à vide
                    }
                    let got = if let Ok(mut q) = in_buf.lock() {
                        if q.len() >= in_frame {
                            for x in framebuf.iter_mut() {
                                *x = q.pop_front().unwrap_or(0.0);
                            }
                            true
                        } else {
                            false
                        }
                    } else {
                        false
                    };
                    if got {
                        // in_rate → 48 kHz → Opus encode → decode → 48 kHz → out_rate.
                        let frame48 = resample_block(&framebuf, OPUS_FRAME);
                        if let Ok(n) = encoder.encode_float(&frame48, &mut packet) {
                            if let Ok(samples) =
                                decoder.decode_float(Some(&packet[..n]), &mut decoded[..], false)
                            {
                                let play = resample_block(&decoded[..samples], out_frame);
                                if let Ok(mut q) = out_buf.lock() {
                                    for &s in play.iter() {
                                        q.push_back(s);
                                    }
                                    while q.len() > out_cap {
                                        q.pop_front();
                                    }
                                }
                            }
                        }
                    } else {
                        std::thread::sleep(std::time::Duration::from_millis(3));
                    }
                }
                // streams (_in_s, _out_s) droppés ici → capture/lecture arrêtées.
            }
            Err(e) => {
                let _ = tx.send(Err(e.to_string()));
            }
        }
    });

    match rx.recv() {
        Ok(Ok(())) => Ok(stop),
        Ok(Err(e)) => Err(anyhow::anyhow!(e)),
        Err(_) => Err(anyhow::anyhow!("démarrage audio interrompu")),
    }
}

// ===== Appel vocal réel entre deux pairs (datagrammes iroh) =====

/// Appel vocal en cours (ou non). Détient le drapeau d'arrêt et l'état « muet ».
#[derive(Clone, Default)]
pub struct Call {
    flag: Arc<Mutex<Option<Arc<AtomicBool>>>>,
    muted: Arc<AtomicBool>,
}

impl Call {
    /// Démarre un appel duplex sur la connexion `conn`, avec les périphériques `cfg`.
    /// `rt` (handle Tokio) sert à lancer la tâche asynchrone de réception des datagrammes.
    pub fn start(
        &self,
        conn: Connection,
        rt: tokio::runtime::Handle,
        cfg: AudioCfg,
    ) -> anyhow::Result<()> {
        self.stop();
        self.muted.store(false, Ordering::SeqCst);
        let stop = Arc::new(AtomicBool::new(false));
        let out_buf: Arc<Mutex<VecDeque<f32>>> = Arc::new(Mutex::new(VecDeque::new()));
        // 1) Capture micro → 48 kHz → Opus → datagrammes (envoi). Renvoie le taux de SORTIE.
        let out_rate = start_capture_send(
            conn.clone(),
            stop.clone(),
            out_buf.clone(),
            cfg,
            self.muted.clone(),
        )?;
        // 2) Réception des datagrammes → Opus (48 kHz) → ré-échantillonnage vers la sortie → lecture.
        rt.spawn(receive_voice(conn, stop.clone(), out_buf, out_rate));
        *self.flag.lock().unwrap_or_else(|e| e.into_inner()) = Some(stop);
        Ok(())
    }
    /// Active/désactive le micro (muet) pendant l'appel.
    pub fn set_mute(&self, on: bool) {
        self.muted.store(on, Ordering::SeqCst);
    }
    /// Termine l'appel en cours (s'il y en a un).
    pub fn stop(&self) {
        if let Some(f) = self.flag.lock().unwrap_or_else(|e| e.into_inner()).take() {
            f.store(true, Ordering::SeqCst);
        }
    }
}

/// Capture le micro, ré-échantillonne vers 48 kHz, encode en Opus, envoie chaque trame en datagramme.
/// Joue aussi `out_buf` (rempli par la réception). Renvoie le taux d'échantillonnage de SORTIE.
fn start_capture_send(
    conn: Connection,
    stop: Arc<AtomicBool>,
    out_buf: Arc<Mutex<VecDeque<f32>>>,
    cfg: AudioCfg,
    muted: Arc<AtomicBool>,
) -> anyhow::Result<u32> {
    let (tx, rx) = std::sync::mpsc::channel::<Result<u32, String>>();
    std::thread::spawn(move || {
        type S = (
            cpal::Stream,
            cpal::Stream,
            Arc<Mutex<VecDeque<f32>>>,
            usize, // trame d'entrée (échantillons @ in_rate)
            audiopus::coder::Encoder,
        );
        // Levé par le callback d'erreur cpal (micro/sortie débranché en plein appel) →
        // couper la capture au lieu de laisser un appel zombie qui tourne à vide.
        let dev_lost = Arc::new(AtomicBool::new(false));
        let setup = (|| -> anyhow::Result<(S, u32)> {
            let host = cpal::default_host();
            let input = pick_input(&host, &cfg.input_name())
                .ok_or_else(|| anyhow::anyhow!("aucun micro détecté"))?;
            let output = pick_output(&host, &cfg.output_name())
                .ok_or_else(|| anyhow::anyhow!("aucune sortie audio détectée"))?;

            let in_buf: Arc<Mutex<VecDeque<f32>>> = Arc::new(Mutex::new(VecDeque::new()));
            let (input_stream, in_rate) = open_input_stream(&input, in_buf.clone(), dev_lost.clone())?;
            let (output_stream, out_rate) = open_output_stream(&output, out_buf.clone(), dev_lost.clone())?;

            let in_frame = (in_rate as usize) / 50;

            let encoder = new_encoder()?;

            input_stream.play()?;
            output_stream.play()?;
            Ok(((input_stream, output_stream, in_buf, in_frame, encoder), out_rate))
        })();

        match setup {
            Ok(((_in_s, _out_s, in_buf, in_frame, encoder), out_rate)) => {
                let _ = tx.send(Ok(out_rate));
                let mut packet = vec![0u8; 4000];
                let mut framebuf = vec![0f32; in_frame];
                while !stop.load(Ordering::SeqCst) {
                    if dev_lost.load(Ordering::SeqCst) {
                        break; // périphérique perdu : couper la capture (plus de zombie)
                    }
                    let got = if let Ok(mut q) = in_buf.lock() {
                        if q.len() >= in_frame {
                            for x in framebuf.iter_mut() {
                                *x = q.pop_front().unwrap_or(0.0);
                            }
                            true
                        } else {
                            false
                        }
                    } else {
                        false
                    };
                    if got {
                        if !muted.load(Ordering::SeqCst) {
                            let frame48 = resample_block(&framebuf, OPUS_FRAME);
                            if let Ok(n) = encoder.encode_float(&frame48, &mut packet[1..]) {
                                packet[0] = VOICE_TAG;
                                let _ = conn
                                    .send_datagram(bytes::Bytes::copy_from_slice(&packet[..1 + n]));
                            }
                        }
                    } else {
                        std::thread::sleep(std::time::Duration::from_millis(3));
                    }
                }
                // streams (_in_s, _out_s) droppés ici → capture/lecture arrêtées.
            }
            Err(e) => {
                let _ = tx.send(Err(e.to_string()));
            }
        }
    });

    match rx.recv() {
        Ok(Ok(out_rate)) => Ok(out_rate),
        Ok(Err(e)) => Err(anyhow::anyhow!(e)),
        Err(_) => Err(anyhow::anyhow!("démarrage audio interrompu")),
    }
}

/// Lit les datagrammes voix, les décode (Opus 48 kHz), ré-échantillonne vers `out_rate`,
/// et pousse dans `out_buf` pour lecture.
async fn receive_voice(
    conn: Connection,
    stop: Arc<AtomicBool>,
    out_buf: Arc<Mutex<VecDeque<f32>>>,
    out_rate: u32,
) {
    let mut decoder = match new_decoder() {
        Ok(d) => d,
        Err(_) => return,
    };
    let out_frame = (out_rate as usize) / 50;
    let out_cap = out_frame * 50;
    // Coussin anti-gigue (~60 ms) : le tampon de sortie démarre plein de silence pour
    // absorber la première gigue réseau au lieu de sous-alimenter la sortie (crépitements).
    if let Ok(mut q) = out_buf.lock() {
        q.extend(std::iter::repeat_n(0.0f32, out_frame * 3));
    }
    let mut decoded = vec![0f32; OPUS_FRAME];
    while !stop.load(Ordering::SeqCst) {
        tokio::select! {
            res = conn.read_datagram() => {
                match res {
                    Ok(dg) => {
                        if dg.len() > 1 && dg[0] == VOICE_TAG {
                            if let Ok(samples) = decoder.decode_float(Some(&dg[1..]), &mut decoded[..], false) {
                                let play = resample_block(&decoded[..samples], out_frame);
                                if let Ok(mut q) = out_buf.lock() {
                                    for &s in play.iter() {
                                        q.push_back(s);
                                    }
                                    // Résorption active de la latence : au-delà de ~240 ms
                                    // (dérive d'horloge / rafale après gigue), jeter une trame
                                    // de 20 ms au lieu d'attendre le plafond de 1 s.
                                    if q.len() > out_frame * 12 {
                                        for _ in 0..out_frame {
                                            q.pop_front();
                                        }
                                    }
                                    while q.len() > out_cap {
                                        q.pop_front();
                                    }
                                }
                            }
                        }
                    }
                    Err(_) => break, // connexion fermée
                }
            }
            _ = tokio::time::sleep(std::time::Duration::from_millis(200)) => {
                // re-vérifie le drapeau d'arrêt même sans datagramme entrant
            }
        }
    }
}

// ===== Appel vocal de GROUPE (maillage : diffusion + mixage multi-flux) =====

/// Tampons de mixage par pair : code → (gain, échantillons en attente).
type Peers = Arc<Mutex<HashMap<String, (f32, VecDeque<f32>)>>>;

/// Appel de groupe en cours : diffuse le micro à tous les pairs et mixe leurs flux.
#[derive(Clone, Default)]
pub struct GroupCall {
    flag: Arc<Mutex<Option<Arc<AtomicBool>>>>,
    muted: Arc<AtomicBool>,
    peers: Peers,
    act: VoiceAct,
    /// Génération de l'appel : incrémentée à chaque start(). L'émetteur d'activité ne
    /// publie son événement de fin (« plus personne en vocal ») que s'il est ENCORE la
    /// génération courante — sinon, basculer d'un appel à l'autre effacerait les
    /// indicateurs tout juste allumés par le nouvel appel.
    gen: Arc<AtomicU64>,
}

impl GroupCall {
    /// Démarre l'appel de groupe avec la liste des pairs (code, connexion du maillage).
    /// `app` sert à pousser l'activité vocale (« qui est en appel / parle ») à l'UI.
    pub fn start(
        &self,
        app: AppHandle,
        conns: Vec<(String, Connection)>,
        rt: tokio::runtime::Handle,
        cfg: AudioCfg,
    ) -> anyhow::Result<()> {
        self.stop();
        self.muted.store(false, Ordering::SeqCst);
        if let Ok(mut p) = self.peers.lock() {
            p.clear();
        }
        if let Ok(mut a) = self.act.lock() {
            a.clear();
        }
        let stop = Arc::new(AtomicBool::new(false));
        let peers = self.peers.clone();
        let act = self.act.clone();
        let conn_list: Vec<Connection> = conns.iter().map(|(_, c)| c.clone()).collect();
        // Capture micro → diffusion à tous + sortie mixée. Renvoie le taux de sortie.
        let out_rate = start_group_capture_mix(
            app.clone(),
            conn_list,
            stop.clone(),
            peers.clone(),
            cfg,
            self.muted.clone(),
            act.clone(),
        )?;
        // Génération de CET appel : calculée AVANT de lancer les tâches de réception,
        // pour que chacune ne nettoie l'état d'un pair en sortant QUE si l'appel courant
        // est toujours le sien (sinon une tâche d'un appel précédent effacerait l'entrée
        // tout juste recréée par le nouvel appel — gain perdu, tampon jeté).
        let my_gen = self.gen.fetch_add(1, Ordering::SeqCst) + 1;
        // Une tâche de réception par pair → décodage → tampon du pair (mixé à la lecture).
        for (peer, conn) in conns {
            rt.spawn(receive_group_voice(
                conn,
                stop.clone(),
                peers.clone(),
                peer,
                out_rate,
                act.clone(),
                self.gen.clone(),
                my_gen,
            ));
        }
        // Émetteur d'activité : pousse ~10 Hz à l'UI qui est en appel et qui parle.
        rt.spawn(emit_voice_activity(app, stop.clone(), act, self.gen.clone(), my_gen));
        *self.flag.lock().unwrap_or_else(|e| e.into_inner()) = Some(stop);
        Ok(())
    }
    /// Règle le volume (gain) de la VOIX d'un pair. 1.0 = normal, 0 = muet, 2.0 = ×2.
    /// Le son d'écran de ce pair a son propre contrôle (`set_screen_gain`) : le curseur
    /// de volume ne coupe donc pas le partage, et couper le partage ne coupe pas la voix.
    pub fn set_gain(&self, code: &str, gain: f32) {
        if let Ok(mut p) = self.peers.lock() {
            p.entry(code.to_string()).or_insert((1.0, VecDeque::new())).0 = gain;
        }
    }
    /// Règle le VOLUME du son d'écran partagé par un pair (le « stream qu'on regarde »).
    /// 1.0 = normal, 0 = muet (raccourci 🔇 = gain 0), 2.0 = ×2. Clé distincte de la voix
    /// (`screen_mix_key`) : régler ce volume ne touche pas la voix du pair.
    pub fn set_screen_gain(&self, code: &str, gain: f32) {
        if let Ok(mut p) = self.peers.lock() {
            p.entry(screen_mix_key(code)).or_insert((1.0, VecDeque::new())).0 = gain.max(0.0);
        }
    }
    pub fn set_mute(&self, on: bool) {
        self.muted.store(on, Ordering::SeqCst);
    }
    pub fn stop(&self) {
        if let Some(f) = self.flag.lock().unwrap_or_else(|e| e.into_inner()).take() {
            f.store(true, Ordering::SeqCst);
        }
    }
}

/// Sortie audio qui MIXE plusieurs flux (somme des pairs + écrêtage), un échantillon par pair.
fn build_group_output(
    device: &cpal::Device,
    config: &cpal::StreamConfig,
    fmt: SampleFormat,
    peers: Peers,
    ch: usize,
    dev_lost: Arc<AtomicBool>,
) -> anyhow::Result<cpal::Stream> {
    macro_rules! build {
        ($t:ty, $conv:path) => {{
            let dev_lost = dev_lost.clone();
            device.build_output_stream(
                config,
                move |data: &mut [$t], _: &cpal::OutputCallbackInfo| {
                    if let Ok(mut m) = peers.lock() {
                        for chunk in data.chunks_mut(ch) {
                            let mut s = 0.0f32;
                            for (gain, buf) in m.values_mut() {
                                s += *gain * buf.pop_front().unwrap_or(0.0);
                            }
                            let v = $conv(s.clamp(-1.0, 1.0));
                            for x in chunk.iter_mut() {
                                *x = v;
                            }
                        }
                    } else {
                        let z = $conv(0.0);
                        for x in data.iter_mut() {
                            *x = z;
                        }
                    }
                },
                // Voir build_input : sortie du mixeur perdue → drapeau observé par la
                // boucle de capture/diffusion de l'appel de groupe.
                move |e| {
                    eprintln!("erreur flux sortie groupe: {e}");
                    dev_lost.store(true, Ordering::SeqCst);
                },
                None,
            )
        }};
    }
    let stream = match fmt {
        SampleFormat::F32 => build!(f32, f32_out),
        SampleFormat::I16 => build!(i16, i16_out),
        SampleFormat::U16 => build!(u16, u16_out),
        other => return Err(anyhow::anyhow!("format sortie non géré: {other:?}")),
    }
    .map_err(|e| anyhow::anyhow!("ouverture du flux sortie groupe: {e}"))?;
    Ok(stream)
}

fn open_group_output(
    device: &cpal::Device,
    peers: Peers,
    dev_lost: Arc<AtomicBool>,
) -> anyhow::Result<(cpal::Stream, u32)> {
    let mut candidates: Vec<cpal::SupportedStreamConfig> = Vec::new();
    if let Ok(def) = device.default_output_config() {
        candidates.push(def);
    }
    if let Ok(list) = device.supported_output_configs() {
        for range in list {
            let sr = if range.min_sample_rate().0 <= 48000 && 48000 <= range.max_sample_rate().0 {
                cpal::SampleRate(48000)
            } else {
                range.max_sample_rate()
            };
            candidates.push(range.with_sample_rate(sr));
        }
    }
    let mut last = anyhow::anyhow!("aucune configuration sortie disponible");
    for sup in candidates {
        let fmt = sup.sample_format();
        let config: cpal::StreamConfig = sup.into();
        let ch = (config.channels as usize).max(1);
        let rate = config.sample_rate.0;
        match build_group_output(device, &config, fmt, peers.clone(), ch, dev_lost.clone()) {
            Ok(s) => return Ok((s, rate)),
            Err(e) => last = e,
        }
    }
    Err(last)
}

/// Capture micro → 48 kHz → Opus → diffusion en datagramme à TOUS les pairs ; sortie mixée.
/// `app` sert à prévenir l'UI (ghost-audio-error) si un périphérique est perdu en appel.
fn start_group_capture_mix(
    app: AppHandle,
    conns: Vec<Connection>,
    stop: Arc<AtomicBool>,
    peers: Peers,
    cfg: AudioCfg,
    muted: Arc<AtomicBool>,
    act: VoiceAct,
) -> anyhow::Result<u32> {
    let (tx, rx) = std::sync::mpsc::channel::<Result<u32, String>>();
    std::thread::spawn(move || {
        type S = (
            cpal::Stream,
            cpal::Stream,
            Arc<Mutex<VecDeque<f32>>>,
            usize,
            audiopus::coder::Encoder,
        );
        // Levé par le callback d'erreur cpal (micro/sortie débranché) → on prévient l'UI.
        let dev_lost = Arc::new(AtomicBool::new(false));
        let setup = (|| -> anyhow::Result<(S, u32)> {
            let host = cpal::default_host();
            let input = pick_input(&host, &cfg.input_name())
                .ok_or_else(|| anyhow::anyhow!("aucun micro détecté"))?;
            let output = pick_output(&host, &cfg.output_name())
                .ok_or_else(|| anyhow::anyhow!("aucune sortie audio détectée"))?;
            let in_buf: Arc<Mutex<VecDeque<f32>>> = Arc::new(Mutex::new(VecDeque::new()));
            let (input_stream, in_rate) = open_input_stream(&input, in_buf.clone(), dev_lost.clone())?;
            let (output_stream, out_rate) = open_group_output(&output, peers.clone(), dev_lost.clone())?;
            let in_frame = (in_rate as usize) / 50;
            let encoder = new_encoder()?;
            input_stream.play()?;
            output_stream.play()?;
            Ok(((input_stream, output_stream, in_buf, in_frame, encoder), out_rate))
        })();
        match setup {
            Ok(((_in_s, _out_s, in_buf, in_frame, encoder), out_rate)) => {
                let _ = tx.send(Ok(out_rate));
                let mut packet = vec![0u8; 4000];
                let mut framebuf = vec![0f32; in_frame];
                // Balise CALL_PING cadencée sur L'HORLOGE (pas sur l'arrivée des trames
                // micro) : émise même sans trame (micro en underrun) pour que les autres
                // ne voient pas le participant « sortir » de l'appel au bout de 3 s.
                let mut last_ping = Instant::now();
                while !stop.load(Ordering::SeqCst) {
                    // Périphérique perdu (débranché en plein appel) : prévenir l'UI et
                    // arrêter la capture au lieu de laisser un appel zombie tourner à vide.
                    if dev_lost.load(Ordering::SeqCst) {
                        let _ = app.emit("ghost-audio-error", "périphérique audio perdu");
                        break;
                    }
                    let got = if let Ok(mut q) = in_buf.lock() {
                        if q.len() >= in_frame {
                            for x in framebuf.iter_mut() {
                                *x = q.pop_front().unwrap_or(0.0);
                            }
                            true
                        } else {
                            false
                        }
                    } else {
                        false
                    };
                    if got {
                        if !muted.load(Ordering::SeqCst) {
                            // Indicateur « je parle » : crête du micro (mais pas si muet —
                            // je ne transmets rien, donc je ne « parle » pas pour les autres).
                            let peak = framebuf.iter().fold(0f32, |m, &s| m.max(s.abs()));
                            if peak > SPEAK_PEAK {
                                if let Ok(mut a) = act.lock() {
                                    a.entry("me".to_string()).or_default().last_voice = Some(Instant::now());
                                }
                            }
                            let frame48 = resample_block(&framebuf, OPUS_FRAME);
                            if let Ok(n) = encoder.encode_float(&frame48, &mut packet[1..]) {
                                packet[0] = VOICE_TAG;
                                let dg = bytes::Bytes::copy_from_slice(&packet[..1 + n]);
                                for c in &conns {
                                    let _ = c.send_datagram(dg.clone());
                                }
                            }
                        }
                    } else {
                        std::thread::sleep(std::time::Duration::from_millis(3));
                    }
                    // Balise « je suis dans l'appel » ~1 Hz sur l'horloge, ÉMISE MÊME SI
                    // MUET et MÊME sans trame micro (les autres doivent voir un participant
                    // muet ou en underrun comme présent).
                    if last_ping.elapsed() >= std::time::Duration::from_secs(1) {
                        last_ping = Instant::now();
                        let dg = bytes::Bytes::copy_from_slice(&[CALL_PING]);
                        for c in &conns {
                            let _ = c.send_datagram(dg.clone());
                        }
                    }
                }
                // streams (_in_s, _out_s) droppés ici → capture/lecture arrêtées.
            }
            Err(e) => {
                let _ = tx.send(Err(e.to_string()));
            }
        }
    });
    match rx.recv() {
        Ok(Ok(out_rate)) => Ok(out_rate),
        Ok(Err(e)) => Err(anyhow::anyhow!(e)),
        Err(_) => Err(anyhow::anyhow!("démarrage audio interrompu")),
    }
}

/// Reçoit les datagrammes voix ET son d'écran d'UN pair, décode, ré-échantillonne,
/// remplit son tampon de mixage. Deux flux Opus indépendants arrivent sur la même
/// connexion (micro = VOICE_TAG, écran = SCREEN_TAG) : chacun a SON décodeur — les
/// mélanger dans un seul corromprait l'état interne d'Opus.
#[allow(clippy::too_many_arguments)]
async fn receive_group_voice(
    conn: Connection,
    stop: Arc<AtomicBool>,
    peers: Peers,
    peer: String,
    out_rate: u32,
    act: VoiceAct,
    gen: Arc<AtomicU64>,
    my_gen: u64,
) {
    let mut voice_dec = match new_decoder() {
        Ok(d) => d,
        Err(_) => return,
    };
    let mut screen_dec = match new_decoder() {
        Ok(d) => d,
        Err(_) => return,
    };
    let skey = screen_mix_key(&peer);
    let out_frame = (out_rate as usize) / 50;
    let out_cap = out_frame * 50;
    // Coussin anti-gigue (~60 ms) pour la VOIX de ce pair : le tampon démarre plein de
    // silence pour absorber la première gigue réseau au lieu de sous-alimenter le mixeur
    // (crépitements). Le son d'écran, moins sensible à la latence, démarre vide.
    if let Ok(mut m) = peers.lock() {
        let e = m.entry(peer.clone()).or_insert((1.0, VecDeque::new()));
        e.1.extend(std::iter::repeat_n(0.0f32, out_frame * 3));
    }
    let mut decoded = vec![0f32; OPUS_FRAME];
    while !stop.load(Ordering::SeqCst) {
        tokio::select! {
            res = conn.read_datagram() => {
                match res {
                    Ok(dg) => {
                        if dg.len() == 1 && dg[0] == CALL_PING {
                            // Balise de présence : ce pair est dans l'appel (même muet).
                            if let Ok(mut a) = act.lock() {
                                a.entry(peer.clone()).or_default().last_ping = Some(Instant::now());
                            }
                        } else if dg.len() > 1 && (dg[0] == VOICE_TAG || dg[0] == SCREEN_TAG) {
                            let dec = if dg[0] == SCREEN_TAG { &mut screen_dec } else { &mut voice_dec };
                            if let Ok(samples) = dec.decode_float(Some(&dg[1..]), &mut decoded[..], false) {
                                // Indicateur « parle » : seulement la VOIX du pair (pas
                                // son son d'écran), au-dessus du seuil de crête.
                                if dg[0] == VOICE_TAG {
                                    let peak = decoded[..samples].iter().fold(0f32, |m, &s| m.max(s.abs()));
                                    if peak > SPEAK_PEAK {
                                        if let Ok(mut a) = act.lock() {
                                            a.entry(peer.clone()).or_default().last_voice = Some(Instant::now());
                                        }
                                    }
                                }
                                let play = resample_block(&decoded[..samples], out_frame);
                                let key = if dg[0] == SCREEN_TAG { &skey } else { &peer };
                                if let Ok(mut m) = peers.lock() {
                                    let e = m.entry(key.clone()).or_insert((1.0, VecDeque::new()));
                                    for &s in play.iter() {
                                        e.1.push_back(s);
                                    }
                                    // Résorption active de la latence : au-delà de ~240 ms
                                    // (dérive d'horloge / rafale après un pic de gigue), on
                                    // jette une trame de 20 ms pour ne pas laisser la latence
                                    // monter jusqu'au plafond de 1 s et y rester.
                                    if e.1.len() > out_frame * 12 {
                                        for _ in 0..out_frame {
                                            e.1.pop_front();
                                        }
                                    }
                                    while e.1.len() > out_cap {
                                        e.1.pop_front();
                                    }
                                }
                            }
                        }
                    }
                    Err(_) => break,
                }
            }
            _ = tokio::time::sleep(std::time::Duration::from_millis(200)) => {}
        }
    }
    // Nettoyage final UNIQUEMENT si l'appel courant est toujours le nôtre : une tâche
    // d'un appel précédent (qui peut survivre ~200 ms) ne doit pas supprimer l'entrée
    // d'un pair commun tout juste recréée par le nouvel appel (gain remis à 1.0, tampon
    // jeté). Même garde que emit_voice_activity.
    if gen.load(Ordering::SeqCst) == my_gen {
        if let Ok(mut m) = peers.lock() {
            m.remove(&peer);
            m.remove(&skey);
        }
        if let Ok(mut a) = act.lock() {
            a.remove(&peer);
        }
    }
}

/// Pousse l'activité vocale à l'UI ~10 Hz : par code, `inCall` (balise ou voix < 3 s) et
/// `speaking` (voix < 400 ms). « me » est toujours en appel tant que la boucle tourne.
async fn emit_voice_activity(
    app: AppHandle,
    stop: Arc<AtomicBool>,
    act: VoiceAct,
    gen: Arc<AtomicU64>,
    my_gen: u64,
) {
    while !stop.load(Ordering::SeqCst) {
        tokio::time::sleep(std::time::Duration::from_millis(100)).await;
        let now = Instant::now();
        let mut map = serde_json::Map::new();
        if let Ok(a) = act.lock() {
            for (code, v) in a.iter() {
                let speaking = v.last_voice.map(|t| now.duration_since(t).as_millis() < 400).unwrap_or(false);
                let in_call = code == "me"
                    || v.last_ping.map(|t| now.duration_since(t).as_secs() < 3).unwrap_or(false)
                    || v.last_voice.map(|t| now.duration_since(t).as_secs() < 3).unwrap_or(false);
                map.insert(code.clone(), serde_json::json!({ "inCall": in_call, "speaking": speaking }));
            }
        }
        if !map.contains_key("me") {
            map.insert("me".to_string(), serde_json::json!({ "inCall": true, "speaking": false }));
        }
        let _ = app.emit("ghost-voice-activity", serde_json::Value::Object(map));
    }
    // Fin d'appel : signaler que plus personne n'est en vocal — MAIS seulement si un
    // nouvel appel n'a pas déjà démarré (sinon on éteindrait ses indicateurs).
    if gen.load(Ordering::SeqCst) == my_gen {
        let _ = app.emit("ghost-voice-activity", serde_json::json!({}));
    }
}

// ===== SON D'ÉCRAN (partage) : process-loopback système → Opus → datagrammes =====

/// Capture du SON SYSTÈME pour le partage d'écran — repli natif : WebView2 ne capture
/// JAMAIS l'audio d'une fenêtre partagée, seulement « Écran entier » + case cochée
/// (MicrosoftEdge/WebView2Feedback#4327). Les trames partent taguées SCREEN_TAG aux
/// mêmes connexions que la voix de groupe ; seuls les membres DANS l'appel de groupe
/// les décodent (receive_group_voice). Anti-écho À LA SOURCE : la capture exclut notre
/// propre processus (sysaudio, process-loopback EXCLUDE), donc les voix de l'appel que
/// ghost link joue ne sont jamais réinjectées — aucun duck nécessaire.
#[derive(Clone, Default)]
pub struct ScreenAudio {
    flag: Arc<Mutex<Option<Arc<AtomicBool>>>>,
}

impl ScreenAudio {
    /// Démarre (ou redémarre) la capture vers les connexions données. `pid = None` →
    /// TOUT le son système sauf nous (écran plein). `pid = Some(p)` → uniquement le son
    /// de ce process (partage d'UNE fenêtre : on ne diffuse que le son de cette appli).
    pub fn start(&self, conns: Vec<Connection>, pid: Option<u32>) -> anyhow::Result<()> {
        self.stop();
        let f = start_screen_capture(conns, pid)?;
        *self.flag.lock().unwrap_or_else(|e| e.into_inner()) = Some(f);
        Ok(())
    }
    /// Coupe la capture en cours (s'il y en a une).
    pub fn stop(&self) {
        if let Some(f) = self.flag.lock().unwrap_or_else(|e| e.into_inner()).take() {
            f.store(true, Ordering::SeqCst);
        }
    }
}

/// Seuil de silence (crête) sous lequel une trame n'est pas envoyée. Le process-loopback
/// livre un flux CONTINU (silence compris) tant qu'une sortie est active ; sans porte,
/// on inonderait le réseau de trames muettes. Un court maintien évite de hacher les fins.
#[cfg(windows)]
const SILENCE_PEAK: f32 = 1e-4;
#[cfg(windows)]
const SILENCE_HANG_FRAMES: u32 = 25; // 500 ms : couvre les silences courts dans un morceau

/// Thread de framing : lit le tampon rempli par la capture système (mono 48 kHz),
/// découpe en trames de 20 ms, encode en Opus et diffuse en SCREEN_TAG. La capture
/// WASAPI (COM) tourne sur SON propre thread dans `sysaudio`.
#[cfg(windows)]
fn start_screen_capture(conns: Vec<Connection>, pid: Option<u32>) -> anyhow::Result<Arc<AtomicBool>> {
    let stop = Arc::new(AtomicBool::new(false));
    let (ready_tx, ready_rx) = std::sync::mpsc::channel::<Result<(), String>>();

    // Thread A : capture (EXCLUDE self = système entier, ou INCLUDE le PID d'une
    // fenêtre = son de cette appli seulement), remplit `sink`.
    let sink: Arc<Mutex<VecDeque<f32>>> = Arc::new(Mutex::new(VecDeque::new()));
    let sink_cap = 48_000; // ~1 s de coussin
    let target = match pid {
        Some(p) => crate::sysaudio::LoopbackTarget::IncludeProcess(p),
        None => crate::sysaudio::LoopbackTarget::ExcludeSelf,
    };
    {
        let stop = stop.clone();
        let sink = sink.clone();
        std::thread::spawn(move || {
            crate::sysaudio::capture_process_loopback(target, stop, ready_tx, sink, sink_cap);
        });
    }

    // Thread B : framing + Opus + envoi. Démarré seulement si la capture s'est lancée.
    let sink_b = sink.clone();
    let stop_b = stop.clone();
    let start_framing = move || {
        std::thread::spawn(move || {
            let encoder = match new_screen_encoder() {
                Ok(e) => e,
                // Sans ça, le thread B meurt mais le thread A de capture continuerait
                // de tourner (CPU) sans que rien ne soit émis — échec silencieux.
                Err(_) => {
                    stop_b.store(true, Ordering::SeqCst);
                    return;
                }
            };
            let in_frame = OPUS_FRAME; // 48 kHz déjà : 960 échantillons = 20 ms
            let mut packet = vec![0u8; 4000];
            let mut framebuf = vec![0f32; in_frame];
            let mut hang = 0u32;
            while !stop_b.load(Ordering::SeqCst) {
                let got = if let Ok(mut q) = sink_b.lock() {
                    if q.len() >= in_frame {
                        for x in framebuf.iter_mut() {
                            *x = q.pop_front().unwrap_or(0.0);
                        }
                        true
                    } else {
                        false
                    }
                } else {
                    false
                };
                if !got {
                    std::thread::sleep(std::time::Duration::from_millis(3));
                    continue;
                }
                // Porte de silence : ne transmettre que si la trame porte du signal
                // (ou juste après, pour ne pas couper les fins de sons).
                let peak = framebuf.iter().fold(0.0f32, |m, &s| m.max(s.abs()));
                if peak >= SILENCE_PEAK {
                    hang = SILENCE_HANG_FRAMES;
                } else if hang > 0 {
                    hang -= 1;
                } else {
                    continue; // silence prolongé : rien à envoyer
                }
                if let Ok(n) = encoder.encode_float(&framebuf, &mut packet[1..]) {
                    packet[0] = SCREEN_TAG;
                    let dg = bytes::Bytes::copy_from_slice(&packet[..1 + n]);
                    for c in &conns {
                        let _ = c.send_datagram(dg.clone());
                    }
                }
            }
        });
    };

    // recv_timeout : si l'activation WASAPI gèle, ne pas bloquer la commande Tauri.
    match ready_rx.recv_timeout(std::time::Duration::from_secs(5)) {
        Ok(Ok(())) => {
            start_framing();
            Ok(stop)
        }
        Ok(Err(e)) => {
            stop.store(true, Ordering::SeqCst);
            Err(anyhow::anyhow!(e))
        }
        Err(_) => {
            stop.store(true, Ordering::SeqCst);
            Err(anyhow::anyhow!(
                "démarrage de la capture du son système impossible (pilote audio bloqué ?)"
            ))
        }
    }
}

/// Hors Windows, la capture « process loopback » n'existe pas : échec propre (l'UI le
/// signale, l'app ne compile pas moins).
#[cfg(not(windows))]
fn start_screen_capture(_conns: Vec<Connection>, _pid: Option<u32>) -> anyhow::Result<Arc<AtomicBool>> {
    Err(anyhow::anyhow!(
        "capture du son système non disponible sur cette plateforme"
    ))
}
