// Frontière typée avec le backend Rust (Tauri, mode withGlobalTauri).
// Tout passe par `invoke` (commandes) et `listen` (événements) ci-dessous :
// les noms de commandes/événements ET la forme de leurs données sont vérifiés
// à la compilation. Une faute de frappe = erreur de build.

/** Canal binaire Tauri (Rust → WebView), créé côté JS et passé en argument d'invoke.
 *  Utilisé par la vidéo native : un message = une image encodée (voir groups.ts). */
export interface BinChannel {
  onmessage: ((data: unknown) => void) | null;
}

declare global {
  interface Window {
    __TAURI__: {
      core: {
        invoke(cmd: string, args?: Record<string, unknown>): Promise<unknown>;
        Channel: new () => BinChannel;
      };
      event: {
        listen(
          event: string,
          handler: (e: { payload: unknown }) => void,
        ): Promise<() => void>;
      };
    };
  }
}

// --- Commandes exposées par Rust : { args attendus ; type de retour } ---
export interface Commands {
  perm_code: { args: void; ret: string };
  eph_code: { args: void; ret: string };
  rotate_eph_code: { args: void; ret: string };
  fingerprint: { args: { code: string }; ret: string };

  connect: { args: { addr: string }; ret: void };
  disconnect: { args: void; ret: void };
  probe: { args: { id: string }; ret: boolean };

  send_file: { args: { path: string }; ret: string };
  cancel_send: { args: void; ret: void };
  cancel_recv: { args: void; ret: void };
  respond_file: { args: { id: number; accept: boolean }; ret: void };

  send_chat: { args: { text: string; name: string }; ret: void };
  send_freq: { args: { name: string }; ret: void };
  send_faccept: { args: { name: string }; ret: void };

  set_friends: { args: { codes: string[] }; ret: void };
  set_download_dir: { args: { path: string }; ret: void };
  get_download_dir: { args: void; ret: string };
  set_only_friends: { args: { on: boolean }; ret: void };
  respond_incoming: { args: { id: number; accept: boolean }; ret: void };

  check_update: { args: void; ret: string | null };
  install_update: { args: void; ret: void };
  app_version: { args: void; ret: string };

  voice_test_start: { args: void; ret: void };
  voice_test_stop: { args: void; ret: void };
  call_start: { args: { signal: boolean }; ret: void };
  call_stop: { args: { signal: boolean }; ret: void };
  call_set_mute: { args: { on: boolean }; ret: void };
  list_audio_devices: { args: void; ret: [string[], string[]] };
  set_audio_input: { args: { name: string | null }; ret: void };
  set_audio_output: { args: { name: string | null }; ret: void };

  open_group: { args: { members: string[] }; ret: void };
  send_ginvite: { args: { member: string; gid: string; name: string; members: string }; ret: void };
  send_gmembers: { args: { members: string[]; gid: string; name: string; roster: string }; ret: void };
  send_kick: { args: { members: string[]; gid: string; target: string; voter: string }; ret: void };
  send_gchat: { args: { members: string[]; gid: string; name: string; text: string }; ret: void };
  send_img: { args: { author: string; name: string; mime: string; data: number[] }; ret: void };
  send_gimg: { args: { members: string[]; gid: string; author: string; name: string; mime: string; data: number[] }; ret: void };
  read_image_bytes: { args: { path: string }; ret: number[] };
  group_call_start: { args: { members: string[]; gid: string; announce: boolean }; ret: void };
  group_call_stop: { args: void; ret: void };
  group_call_mute: { args: { on: boolean }; ret: void };
  group_call_volume: { args: { peer: string; vol: number }; ret: void };
  voice_presence: { args: { members: string[]; gid: string; inCall: boolean }; ret: void };
  screen_audio_start: { args: { members: string[]; pid: number | null }; ret: void };
  screen_audio_stop: { args: void; ret: void };
  screen_audio_gain: { args: { peer: string; vol: number }; ret: void };
  send_signal: { args: { peer: string; data: string }; ret: void };
  send_gfile: { args: { members: string[]; path: string }; ret: void };
  respond_gfile: { args: { id: number; accept: boolean }; ret: void };
  set_streams: { args: { n: number }; ret: void };

  // Partage d'écran NATIF (sans WebRTC/STUN) — video.rs.
  video_share_start: {
    args: { members: string[]; monitor: string | null; window: string | null; maxFps: number };
    ret: { w: number; h: number; fps: number; monitor: string; monitorFound: boolean };
  };
  video_share_stop: { args: void; ret: void };
  video_receive_attach: { args: { channel: BinChannel }; ret: void };
  video_list_monitors: {
    args: void;
    ret: { id: string; name: string; w: number; h: number; primary: boolean }[];
  };
  video_list_windows: { args: void; ret: { id: string; name: string; pid: number }[] };
}

// --- Événements émis par Rust : forme du payload reçu ---
export interface Events {
  "ghost-connected": string;
  "ghost-disconnected": null;
  "ghost-refused": string;

  "ghost-send-await": null;
  "ghost-meta": { name: string; status: "cleaned" | "skipped" | "failed"; info?: string };
  "ghost-send-progress": { sent: number; size: number };
  "ghost-recv-start": { name: string; size: number };
  "ghost-recv-progress": { received: number; size: number };
  "ghost-recv-done": { name: string; size: number; path: string };
  "ghost-recv-cancel": { name: string };
  "ghost-recv-offer": { id: number; name: string; size: number };
  "ghost-recv-rejected": { name: string };
  "ghost-recv-corrupt": { name: string };
  "ghost-recv-nospace": { name?: string; size?: number; free?: number; from?: string };

  "ghost-chat": { text: string; name: string };
  "ghost-chat-img": { author?: string; name?: string; mime: string; dataB64: string };
  "ghost-freq": { name?: string; code?: string };
  "ghost-faccept": { name?: string; code?: string };

  "update-progress": { chunk: number; total: number };

  "ghost-incoming": { id: number; peer: string };
  "ghost-incoming-cancel": null;

  "ghost-call-start": null;
  "ghost-call-stop": null;

  "ghost-mesh-up": string;
  "ghost-mesh-down": string;
  "ghost-gchat": { group: string; author?: string; text?: string };
  "ghost-gchat-img": { group: string; author?: string; name?: string; mime: string; dataB64: string; from?: string };
  "ghost-ginvite": { id: string; name?: string; members?: string };
  "ghost-gmembers": { group: string; name?: string; members?: string; from?: string };
  "ghost-kick": { group: string; target: string; voter: string; from?: string };
  "ghost-gcall": { group: string };
  "ghost-voice-presence": { group: string; code: string; inCall: boolean };
  "ghost-signal": { from?: string; data: string };
  "ghost-grecv-start": { name?: string; from?: string };
  "ghost-grecv-done": { name?: string };
  "ghost-grecv-offer": { id: number; name?: string; size?: number; from?: string };
  "ghost-grecv-rejected": { name?: string };
  "ghost-grecv-corrupt": { name?: string; from?: string };
  "ghost-video-ended": { reason?: string };
  "ghost-video-rx-end": string;
  "ghost-video-peer-dead": string;
  "ghost-voice-activity": Record<string, { inCall: boolean; speaking: boolean }>;
  "ghost-video-stats": {
    fps: number;
    kbps: number;
    peers: number;
    peersOk: number;
    level: number;
    pct: number;
    dyn: boolean;
    w: number;
    h: number;
  };

  "tauri://drag-enter": unknown;
  "tauri://drag-over": unknown;
  "tauri://drag-leave": unknown;
  "tauri://drag-drop": { paths?: string[] };
}

type HasArgs<K extends keyof Commands> = Commands[K]["args"] extends void ? false : true;

/** Appelle une commande Rust. Le nom et la forme des arguments sont typés. */
export function invoke<K extends keyof Commands>(
  ...[cmd, args]: HasArgs<K> extends true
    ? [cmd: K, args: Commands[K]["args"]]
    : [cmd: K, args?: undefined]
): Promise<Commands[K]["ret"]> {
  return window.__TAURI__.core.invoke(
    cmd as string,
    args as Record<string, unknown> | undefined,
  ) as Promise<Commands[K]["ret"]>;
}

/** Écoute un événement Rust. Le payload est typé selon l'événement. */
export function listen<K extends keyof Events>(
  event: K,
  handler: (e: { payload: Events[K] }) => void,
): void {
  void window.__TAURI__.event.listen(
    event,
    handler as (e: { payload: unknown }) => void,
  );
}
