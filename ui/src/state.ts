// État partagé + couche de données (localStorage). Ce module n'importe AUCUN
// module de domaine → il est la base du graphe (pas de cycle d'imports).

import { invoke } from "./tauri.js";
import { shortId } from "./dom.js";

export interface Friend {
  name: string;
  code: string;
  mutual?: boolean;
}
export interface Group {
  id: string;
  name: string;
  members: string[];
}
export interface PInvItem {
  member: string;
  gid: string;
  name: string;
  csv: string;
}
export interface GroupMsg {
  author: string;
  text: string;
  who: string;
}
export interface PcState {
  pc: RTCPeerConnection;
  makingOffer: boolean;
  polite: boolean;
}

export const FKEY = "ghostlink_friends";
export const GKEY = "ghostlink_groups";
export const PINV = "ghostlink_pinv"; // invitations à (ré)envoyer
export const GDECL = "ghostlink_declined"; // ids de groupes refusés
/** Config ICE pour la vidéo. Réglage utilisateur 'ghostlink_ice' : URL STUN/TURN,
 *  ou vide = LAN uniquement (aucun tiers contacté). Défaut = STUN Google. */
export function iceConfig(): RTCConfiguration {
  const v = (localStorage.getItem("ghostlink_ice") ?? "stun:stun.l.google.com:19302").trim();
  return v ? { iceServers: [{ urls: v }] } : { iceServers: [] };
}
/** Partage d'écran NATIF (expérimental) : capture+H.264 côté Rust, flux iroh —
 *  pas de WebRTC/STUN pour l'écran. Réglages → case « Partage d'écran natif ». */
export function nativeVideoWanted(): boolean {
  return localStorage.getItem("ghostlink_native_video") === "1";
}

// Tout l'état mutable de l'app, regroupé (les modules font S.xxx).
export const S = {
  myCode: "",
  currentPeer: null as string | null,
  presence: {} as Record<string, string>,
  fpCache: {} as Record<string, string>,
  presenceBusy: false,
  // demandes d'ami
  pendingFreqName: "",
  pendingFreqCode: "",
  // mises à jour
  dlBytes: 0,
  // transfert (stats débit)
  sT: 0,
  sB: 0,
  sSpd: 0,
  sLast: 0,
  rT: 0,
  rB: 0,
  rSpd: 0,
  rLast: 0,
  fileOfferId: null as number | null,
  // session / connexion entrante
  incomingId: null as number | null,
  // vocal 1-à-1
  voiceTesting: false,
  inCall: false,
  muted: false,
  callOfferTimer: null as number | null,
  // groupes
  groupMsgs: {} as Record<string, GroupMsg[]>,
  openGroupId: null as string | null,
  pendingInvite: null as { id: string; name: string; full: string[] } | null,
  meshOnline: new Set<string>(),
  groupGains: {} as Record<string, number>,
  // Son du partage d'écran coupé, PAR pair (le son natif est per-pair, pas per-flux) :
  // une vignette recréée relit cet état au lieu de repartir à « son activé ».
  screenMuted: {} as Record<string, boolean>,
  inGroupCall: false,
  groupMuted: false,
  groupCallId: null as string | null,
  pendingGCall: null as string | null,
  gfileOfferId: null as number | null,
  // vidéo (WebRTC)
  localCam: null as MediaStream | null,
  localScreen: null as MediaStream | null,
  pcs: {} as Record<string, PcState>,
  // vidéo NATIVE (video.rs) : partage d'écran en cours, sans MediaStream local.
  localScreenNative: false,
};

export function loadFriends(): Friend[] {
  try {
    return (JSON.parse(localStorage.getItem(FKEY) || "") as Friend[]) || [];
  } catch {
    return [];
  }
}
export function saveFriends(a: Friend[]): void {
  localStorage.setItem(FKEY, JSON.stringify(a));
}
export function loadGroups(): Group[] {
  try {
    return (JSON.parse(localStorage.getItem(GKEY) || "") as Group[]) || [];
  } catch {
    return [];
  }
}
export function saveGroups(a: Group[]): void {
  localStorage.setItem(GKEY, JSON.stringify(a));
}
export function myName(): string {
  return (localStorage.getItem("ghostlink_name") || "").trim();
}
// SEC-5 : ne composer (dial) automatiquement que des membres qui sont MES amis (et pas moi).
export function friendsOnly(members: string[]): string[] {
  const fr = loadFriends();
  return (members || []).filter((c) => c && c !== S.myCode && fr.some((f) => f.code === c));
}
export function memberName(code: string): string {
  const f = loadFriends().find((x) => x.code === code);
  return f && f.name ? f.name : shortId(code);
}
export function pushFriendsToBackend(): void {
  const codes = loadFriends().map((f) => f.code);
  invoke("set_friends", { codes }).catch(() => {});
}
