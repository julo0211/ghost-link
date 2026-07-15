// État partagé + couche de données (localStorage). Ce module n'importe AUCUN
// module de domaine → il est la base du graphe (pas de cycle d'imports).
import { invoke } from "./tauri.js";
import { shortId } from "./dom.js";
export const FKEY = "ghostlink_friends";
export const GKEY = "ghostlink_groups";
export const PINV = "ghostlink_pinv"; // invitations à (ré)envoyer
export const GDECL = "ghostlink_declined"; // ids de groupes refusés
/** Config ICE pour la vidéo. Réglage utilisateur 'ghostlink_ice' : URL STUN/TURN,
 *  ou vide = LAN uniquement (aucun tiers contacté). Défaut = STUN Google. */
export function iceConfig() {
    const v = (localStorage.getItem("ghostlink_ice") ?? "stun:stun.l.google.com:19302").trim();
    return v ? { iceServers: [{ urls: v }] } : { iceServers: [] };
}
/** Partage d'écran NATIF (expérimental) : capture+H.264 côté Rust, flux iroh —
 *  pas de WebRTC/STUN pour l'écran. Réglages → case « Partage d'écran natif ». */
export function nativeVideoWanted() {
    return localStorage.getItem("ghostlink_native_video") === "1";
}
// Tout l'état mutable de l'app, regroupé (les modules font S.xxx).
export const S = {
    myCode: "",
    currentPeer: null,
    presence: {},
    fpCache: {},
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
    fileOfferId: null,
    // session / connexion entrante
    incomingId: null,
    // vocal 1-à-1
    voiceTesting: false,
    inCall: false,
    muted: false,
    callOfferTimer: null,
    // groupes
    groupMsgs: {},
    openGroupId: null,
    pendingInvite: null,
    meshOnline: new Set(),
    groupGains: {},
    // Son du partage d'écran coupé, PAR pair (le son natif est per-pair, pas per-flux) :
    // une vignette recréée relit cet état au lieu de repartir à « son activé ».
    screenMuted: {},
    // Volume du son d'écran d'un pair, en % (0..200) — le « stream qu'on regarde ».
    // Défaut 100 (non stocké = 100). Relu par une vignette recréée.
    screenGains: {},
    inGroupCall: false,
    groupMuted: false,
    groupCallId: null,
    pendingGCall: null,
    // Votes d'exclusion en cours : clé "gid|cible" → { codeVotant: horodatage }.
    kickVotes: {},
    // Qui est en vocal / parle en ce moment (event ghost-voice-activity), par code.
    voiceAct: {},
    gfileOfferId: null,
    // vidéo (WebRTC)
    localCam: null,
    localScreen: null,
    pcs: {},
    // vidéo NATIVE (video.rs) : partage d'écran en cours, sans MediaStream local.
    localScreenNative: false,
};
export function loadFriends() {
    try {
        return JSON.parse(localStorage.getItem(FKEY) || "") || [];
    }
    catch {
        return [];
    }
}
export function saveFriends(a) {
    localStorage.setItem(FKEY, JSON.stringify(a));
}
export function loadGroups() {
    try {
        return JSON.parse(localStorage.getItem(GKEY) || "") || [];
    }
    catch {
        return [];
    }
}
export function saveGroups(a) {
    localStorage.setItem(GKEY, JSON.stringify(a));
}
export function myName() {
    return (localStorage.getItem("ghostlink_name") || "").trim();
}
// SEC-5 : ne composer (dial) automatiquement que des membres qui sont MES amis (et pas moi).
export function friendsOnly(members) {
    const fr = loadFriends();
    return (members || []).filter((c) => c && c !== S.myCode && fr.some((f) => f.code === c));
}
export function memberName(code) {
    const f = loadFriends().find((x) => x.code === code);
    return f && f.name ? f.name : shortId(code);
}
export function pushFriendsToBackend() {
    const codes = loadFriends().map((f) => f.code);
    invoke("set_friends", { codes }).catch(() => { });
}
