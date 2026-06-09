// Audio temps réel (cpal + Opus).
// Opus tourne TOUJOURS en 48 kHz mono. On convertit n'importe quel format de
// périphérique (f32/i16/u16) et on ré-échantillonne automatiquement vers/depuis 48 kHz,
// pour que l'appel fonctionne quelle que soit la carte son (ex. micro en 44,1 kHz).

use std::collections::{HashMap, VecDeque};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};

use cpal::traits::{DeviceTrait, HostTrait, StreamTrait};
use cpal::SampleFormat;
use iroh::endpoint::Connection;

/// Opus fonctionne à 48 kHz mono, trame de 20 ms = 960 échantillons.
const OPUS_FRAME: usize = 960;
const VOICE_TAG: u8 = 1; // premier octet d'un datagramme = type « voix »

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
        *self.input.lock().unwrap() = name.filter(|s| !s.trim().is_empty());
    }
    pub fn set_output(&self, name: Option<String>) {
        *self.output.lock().unwrap() = name.filter(|s| !s.trim().is_empty());
    }
    fn input_name(&self) -> Option<String> {
        self.input.lock().unwrap().clone()
    }
    fn output_name(&self) -> Option<String> {
        self.output.lock().unwrap().clone()
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
) -> anyhow::Result<cpal::Stream> {
    macro_rules! build {
        ($t:ty, $conv:path) => {
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
                |e| eprintln!("erreur flux micro: {e}"),
                None,
            )
        };
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
) -> anyhow::Result<cpal::Stream> {
    macro_rules! build {
        ($t:ty, $conv:path) => {
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
                |e| eprintln!("erreur flux sortie: {e}"),
                None,
            )
        };
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
        match build_input(device, &config, fmt, in_buf.clone(), ch, in_cap) {
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
        match build_output(device, &config, fmt, out_buf.clone(), ch) {
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
        *self.flag.lock().unwrap() = Some(f);
        Ok(())
    }
    /// Coupe la boucle en cours (s'il y en a une).
    pub fn stop(&self) {
        if let Some(f) = self.flag.lock().unwrap().take() {
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
        let setup = (|| -> anyhow::Result<Setup> {
            let host = cpal::default_host();
            let input = pick_input(&host, &cfg.input_name())
                .ok_or_else(|| anyhow::anyhow!("aucun micro détecté"))?;
            let output = pick_output(&host, &cfg.output_name())
                .ok_or_else(|| anyhow::anyhow!("aucune sortie audio détectée"))?;

            let in_buf: Arc<Mutex<VecDeque<f32>>> = Arc::new(Mutex::new(VecDeque::new()));
            let out_buf: Arc<Mutex<VecDeque<f32>>> = Arc::new(Mutex::new(VecDeque::new()));

            let (input_stream, in_rate) = open_input_stream(&input, in_buf.clone())?;
            let (output_stream, out_rate) = open_output_stream(&output, out_buf.clone())?;

            let in_frame = (in_rate as usize) / 50; // 20 ms @ in_rate
            let out_frame = (out_rate as usize) / 50; // 20 ms @ out_rate
            // Petit coussin (~100 ms) en sortie pour lisser.
            out_buf
                .lock()
                .unwrap()
                .extend(std::iter::repeat(0.0f32).take(out_frame * 5));

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
                // streams (_in_s, _out_s) droppés ici → l'audio s'arrête.
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
        *self.flag.lock().unwrap() = Some(stop);
        Ok(())
    }
    /// Active/désactive le micro (muet) pendant l'appel.
    pub fn set_mute(&self, on: bool) {
        self.muted.store(on, Ordering::SeqCst);
    }
    /// Termine l'appel en cours (s'il y en a un).
    pub fn stop(&self) {
        if let Some(f) = self.flag.lock().unwrap().take() {
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
        let setup = (|| -> anyhow::Result<(S, u32)> {
            let host = cpal::default_host();
            let input = pick_input(&host, &cfg.input_name())
                .ok_or_else(|| anyhow::anyhow!("aucun micro détecté"))?;
            let output = pick_output(&host, &cfg.output_name())
                .ok_or_else(|| anyhow::anyhow!("aucune sortie audio détectée"))?;

            let in_buf: Arc<Mutex<VecDeque<f32>>> = Arc::new(Mutex::new(VecDeque::new()));
            let (input_stream, in_rate) = open_input_stream(&input, in_buf.clone())?;
            let (output_stream, out_rate) = open_output_stream(&output, out_buf.clone())?;

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
}

impl GroupCall {
    /// Démarre l'appel de groupe avec la liste des pairs (code, connexion du maillage).
    pub fn start(
        &self,
        conns: Vec<(String, Connection)>,
        rt: tokio::runtime::Handle,
        cfg: AudioCfg,
    ) -> anyhow::Result<()> {
        self.stop();
        self.muted.store(false, Ordering::SeqCst);
        if let Ok(mut p) = self.peers.lock() {
            p.clear();
        }
        let stop = Arc::new(AtomicBool::new(false));
        let peers = self.peers.clone();
        let conn_list: Vec<Connection> = conns.iter().map(|(_, c)| c.clone()).collect();
        // Capture micro → diffusion à tous + sortie mixée. Renvoie le taux de sortie.
        let out_rate = start_group_capture_mix(
            conn_list,
            stop.clone(),
            peers.clone(),
            cfg,
            self.muted.clone(),
        )?;
        // Une tâche de réception par pair → décodage → tampon du pair (mixé à la lecture).
        for (peer, conn) in conns {
            rt.spawn(receive_group_voice(conn, stop.clone(), peers.clone(), peer, out_rate));
        }
        *self.flag.lock().unwrap() = Some(stop);
        Ok(())
    }
    /// Règle le volume (gain) d'un pair dans le mixage. 1.0 = normal, 0 = muet, 2.0 = ×2.
    pub fn set_gain(&self, code: &str, gain: f32) {
        if let Ok(mut p) = self.peers.lock() {
            p.entry(code.to_string()).or_insert((1.0, VecDeque::new())).0 = gain;
        }
    }
    pub fn set_mute(&self, on: bool) {
        self.muted.store(on, Ordering::SeqCst);
    }
    pub fn stop(&self) {
        if let Some(f) = self.flag.lock().unwrap().take() {
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
) -> anyhow::Result<cpal::Stream> {
    macro_rules! build {
        ($t:ty, $conv:path) => {
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
                |e| eprintln!("erreur flux sortie groupe: {e}"),
                None,
            )
        };
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
        match build_group_output(device, &config, fmt, peers.clone(), ch) {
            Ok(s) => return Ok((s, rate)),
            Err(e) => last = e,
        }
    }
    Err(last)
}

/// Capture micro → 48 kHz → Opus → diffusion en datagramme à TOUS les pairs ; sortie mixée.
fn start_group_capture_mix(
    conns: Vec<Connection>,
    stop: Arc<AtomicBool>,
    peers: Peers,
    cfg: AudioCfg,
    muted: Arc<AtomicBool>,
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
        let setup = (|| -> anyhow::Result<(S, u32)> {
            let host = cpal::default_host();
            let input = pick_input(&host, &cfg.input_name())
                .ok_or_else(|| anyhow::anyhow!("aucun micro détecté"))?;
            let output = pick_output(&host, &cfg.output_name())
                .ok_or_else(|| anyhow::anyhow!("aucune sortie audio détectée"))?;
            let in_buf: Arc<Mutex<VecDeque<f32>>> = Arc::new(Mutex::new(VecDeque::new()));
            let (input_stream, in_rate) = open_input_stream(&input, in_buf.clone())?;
            let (output_stream, out_rate) = open_group_output(&output, peers.clone())?;
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
                                let dg = bytes::Bytes::copy_from_slice(&packet[..1 + n]);
                                for c in &conns {
                                    let _ = c.send_datagram(dg.clone());
                                }
                            }
                        }
                    } else {
                        std::thread::sleep(std::time::Duration::from_millis(3));
                    }
                }
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

/// Reçoit les datagrammes voix d'UN pair, décode, ré-échantillonne, remplit son tampon de mixage.
async fn receive_group_voice(
    conn: Connection,
    stop: Arc<AtomicBool>,
    peers: Peers,
    peer: String,
    out_rate: u32,
) {
    let mut decoder = match new_decoder() {
        Ok(d) => d,
        Err(_) => return,
    };
    let out_frame = (out_rate as usize) / 50;
    let out_cap = out_frame * 50;
    let mut decoded = vec![0f32; OPUS_FRAME];
    while !stop.load(Ordering::SeqCst) {
        tokio::select! {
            res = conn.read_datagram() => {
                match res {
                    Ok(dg) => {
                        if dg.len() > 1 && dg[0] == VOICE_TAG {
                            if let Ok(samples) = decoder.decode_float(Some(&dg[1..]), &mut decoded[..], false) {
                                let play = resample_block(&decoded[..samples], out_frame);
                                if let Ok(mut m) = peers.lock() {
                                    let e = m.entry(peer.clone()).or_insert((1.0, VecDeque::new()));
                                    for &s in play.iter() {
                                        e.1.push_back(s);
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
    if let Ok(mut m) = peers.lock() {
        m.remove(&peer);
    }
}
