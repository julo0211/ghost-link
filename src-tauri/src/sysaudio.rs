// Capture du SON SYSTÈME via WASAPI « process loopback » en EXCLUANT notre propre
// arbre de processus (Windows 10 2004+). C'est l'anti-écho parfait pour le partage
// d'écran : le son des autres applis (jeu, vidéo, musique) est capté, mais les voix de
// l'appel de groupe que ghost link joue lui-même sont exclues à la SOURCE — plus besoin
// d'atténuer (duck) le stream quand quelqu'un parle. Tout le code COM/WASAPI `unsafe`
// est isolé ici ; le reste de l'app ne voit qu'un flux d'échantillons mono 48 kHz.
//
// Validé empiriquement (mode EXCLUDE avec un ton joué par notre process → RMS ≈ 0).

use std::collections::VecDeque;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::mpsc::Sender;
use std::sync::{Arc, Mutex};
use std::time::Duration;

use windows::core::{implement, Interface, PROPVARIANT};
use windows::Win32::Media::Audio::{
    ActivateAudioInterfaceAsync, IActivateAudioInterfaceAsyncOperation,
    IActivateAudioInterfaceCompletionHandler, IActivateAudioInterfaceCompletionHandler_Impl,
    IAudioCaptureClient, IAudioClient, AUDCLNT_SHAREMODE_SHARED, AUDCLNT_STREAMFLAGS_LOOPBACK,
    AUDIOCLIENT_ACTIVATION_PARAMS, AUDIOCLIENT_ACTIVATION_PARAMS_0,
    AUDIOCLIENT_ACTIVATION_TYPE_PROCESS_LOOPBACK, AUDIOCLIENT_PROCESS_LOOPBACK_PARAMS,
    PROCESS_LOOPBACK_MODE_EXCLUDE_TARGET_PROCESS_TREE,
    PROCESS_LOOPBACK_MODE_INCLUDE_TARGET_PROCESS_TREE, VIRTUAL_AUDIO_DEVICE_PROCESS_LOOPBACK,
    WAVEFORMATEX,
};
use windows::Win32::System::Com::{CoInitializeEx, CoUninitialize, COINIT_MULTITHREADED};

/// Le format d'un flux « process loopback » est IMPOSÉ (pas de GetMixFormat) : on demande
/// explicitement 48 kHz, stéréo, f32 — l'app tourne déjà tout en 48 kHz.
const CAPTURE_RATE: u32 = 48_000;
const CAPTURE_CH: u16 = 2;
const WAVE_FORMAT_IEEE_FLOAT: u16 = 3;
/// AUDCLNT_S_NO_SINGLE_PROCESS (0x08890008) : succès « soft » possible de l'activation.
const AUDCLNT_S_NO_SINGLE_PROCESS: i32 = 0x0889_0008u32 as i32;

/// PROPVARIANT au layout C exact (VT_BLOB + BLOB) : la PROPVARIANT « intelligente » de
/// windows-core n'expose pas de constructeur BLOB — on la fabrique nous-mêmes et on
/// caste le pointeur (même disposition mémoire).
#[repr(C)]
struct RawBlobPropVariant {
    vt: u16,
    r1: u16,
    r2: u16,
    r3: u16,
    cb_size: u32,
    p_blob: *mut u8,
}

/// Handler de complétion de l'activation asynchrone : réveille le thread appelant.
#[implement(IActivateAudioInterfaceCompletionHandler)]
struct Completion(Sender<()>);

impl IActivateAudioInterfaceCompletionHandler_Impl for Completion_Impl {
    fn ActivateCompleted(
        &self,
        _op: Option<&IActivateAudioInterfaceAsyncOperation>,
    ) -> windows::core::Result<()> {
        let _ = self.0.send(());
        Ok(())
    }
}

/// Cible de capture « process loopback » : soit TOUT le système sauf nous (partage
/// d'écran plein / anti-écho), soit le son d'UN process précis (partage d'une fenêtre
/// → seul le son de cette appli). Le PID et le mode INCLUDE/EXCLUDE en découlent.
#[derive(Clone, Copy)]
pub enum LoopbackTarget {
    /// Tout le son système EN EXCLUANT notre propre arbre de processus (anti-écho).
    ExcludeSelf,
    /// Uniquement le son de l'arbre de processus donné (une fenêtre partagée).
    IncludeProcess(u32),
}

/// Active le client audio « process loopback » pour la cible et renvoie l'IAudioClient.
unsafe fn activate_loopback(target: LoopbackTarget) -> anyhow::Result<IAudioClient> {
    let (pid, mode) = match target {
        LoopbackTarget::ExcludeSelf => {
            (std::process::id(), PROCESS_LOOPBACK_MODE_EXCLUDE_TARGET_PROCESS_TREE)
        }
        LoopbackTarget::IncludeProcess(pid) => {
            (pid, PROCESS_LOOPBACK_MODE_INCLUDE_TARGET_PROCESS_TREE)
        }
    };
    let params = AUDIOCLIENT_ACTIVATION_PARAMS {
        ActivationType: AUDIOCLIENT_ACTIVATION_TYPE_PROCESS_LOOPBACK,
        Anonymous: AUDIOCLIENT_ACTIVATION_PARAMS_0 {
            ProcessLoopbackParams: AUDIOCLIENT_PROCESS_LOOPBACK_PARAMS {
                TargetProcessId: pid,
                ProcessLoopbackMode: mode,
            },
        },
    };
    let prop = RawBlobPropVariant {
        vt: 65, // VT_BLOB
        r1: 0,
        r2: 0,
        r3: 0,
        cb_size: std::mem::size_of::<AUDIOCLIENT_ACTIVATION_PARAMS>() as u32,
        p_blob: &params as *const _ as *mut u8,
    };
    let (tx, rx) = std::sync::mpsc::channel();
    let handler: IActivateAudioInterfaceCompletionHandler = Completion(tx).into();
    let op = ActivateAudioInterfaceAsync(
        VIRTUAL_AUDIO_DEVICE_PROCESS_LOOPBACK,
        &IAudioClient::IID,
        Some(&prop as *const _ as *const PROPVARIANT),
        &handler,
    )?;
    // `params` doit rester vivant tant que l'activation n'a pas complété.
    rx.recv_timeout(Duration::from_secs(3))
        .map_err(|_| anyhow::anyhow!("activation loopback : pas de réponse"))?;
    let mut hr = windows::core::HRESULT(0);
    let mut iface: Option<windows::core::IUnknown> = None;
    op.GetActivateResult(&mut hr, &mut iface)?;
    if hr.0 != 0 && hr.0 != AUDCLNT_S_NO_SINGLE_PROCESS {
        return Err(anyhow::anyhow!("activation loopback refusée (hr=0x{:08x})", hr.0));
    }
    // `params` est resté vivant durant toute l'activation (bloquée sur rx ci-dessus) :
    // son adresse était référencée par le BLOB du PROPVARIANT.
    iface
        .ok_or_else(|| anyhow::anyhow!("activation loopback : interface absente"))?
        .cast::<IAudioClient>()
        .map_err(|e| anyhow::anyhow!("cast IAudioClient: {e}"))
}

/// Capture le son système (hors notre process) et pousse des échantillons MONO 48 kHz
/// dans `sink` jusqu'à ce que `stop` passe à true. Envoie `ready` UNE fois l'init OK
/// (ou l'erreur d'init) — l'appelant sait alors si la capture a démarré. Doit tourner
/// sur son propre thread (COM STA/MTA + boucle bloquante).
pub fn capture_process_loopback(
    target: LoopbackTarget,
    stop: Arc<AtomicBool>,
    ready: Sender<Result<(), String>>,
    sink: Arc<Mutex<VecDeque<f32>>>,
    sink_cap: usize,
) {
    unsafe {
        // MTA : le handler d'activation est appelé sur un thread du pool COM.
        // Chaque partage crée un thread neuf → il faut équilibrer CoInitializeEx par un
        // CoUninitialize à la sortie, sinon l'apartment COM fuit à chaque cycle on/off.
        let co = CoInitializeEx(None, COINIT_MULTITHREADED);
        let co_ok = co.is_ok(); // S_OK/S_FALSE : à désinitialiser ; RPC_E_CHANGED_MODE : non
        let setup = (|| -> anyhow::Result<(IAudioClient, IAudioCaptureClient)> {
            let client = activate_loopback(target)?;
            let format = WAVEFORMATEX {
                wFormatTag: WAVE_FORMAT_IEEE_FLOAT,
                nChannels: CAPTURE_CH,
                nSamplesPerSec: CAPTURE_RATE,
                nAvgBytesPerSec: CAPTURE_RATE * CAPTURE_CH as u32 * 4,
                nBlockAlign: CAPTURE_CH * 4,
                wBitsPerSample: 32,
                cbSize: 0,
            };
            client.Initialize(
                AUDCLNT_SHAREMODE_SHARED,
                AUDCLNT_STREAMFLAGS_LOOPBACK,
                2_000_000, // buffer 200 ms (unités de 100 ns)
                0,
                &format,
                None,
            )?;
            let cap: IAudioCaptureClient = client.GetService()?;
            client.Start()?;
            Ok((client, cap))
        })();

        let (client, cap) = match setup {
            Ok(v) => {
                let _ = ready.send(Ok(()));
                v
            }
            Err(e) => {
                let _ = ready.send(Err(e.to_string()));
                if co_ok {
                    CoUninitialize();
                }
                return;
            }
        };

        while !stop.load(Ordering::SeqCst) {
            // Polling léger : le buffer de 200 ms tolère largement 10 ms de latence.
            std::thread::sleep(Duration::from_millis(10));
            loop {
                let pkt = match cap.GetNextPacketSize() {
                    Ok(n) => n,
                    Err(_) => break,
                };
                if pkt == 0 {
                    break;
                }
                let mut data: *mut u8 = std::ptr::null_mut();
                let mut frames = 0u32;
                let mut flags = 0u32;
                if cap
                    .GetBuffer(&mut data, &mut frames, &mut flags, None, None)
                    .is_err()
                {
                    break;
                }
                if frames > 0 {
                    if let Ok(mut q) = sink.lock() {
                        // AUDCLNT_BUFFERFLAGS_SILENT (0x2) : contenu à ignorer → silence.
                        if flags & 0x2 != 0 || data.is_null() {
                            for _ in 0..frames {
                                q.push_back(0.0);
                            }
                        } else {
                            let inter =
                                std::slice::from_raw_parts(data as *const f32, (frames * 2) as usize);
                            for pair in inter.chunks_exact(2) {
                                q.push_back((pair[0] + pair[1]) * 0.5); // downmix stéréo → mono
                            }
                        }
                        while q.len() > sink_cap {
                            q.pop_front();
                        }
                    }
                }
                let _ = cap.ReleaseBuffer(frames);
            }
        }
        let _ = client.Stop();
        // client/cap (IAudioClient, IAudioCaptureClient) sont Release-és à leur drop ici.
        drop(cap);
        drop(client);
        if co_ok {
            CoUninitialize();
        }
    }
}
