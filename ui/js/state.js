// État partagé + couche de données (localStorage). Ce module n'importe AUCUN
// module de domaine → il est la base du graphe (pas de cycle d'imports).
import { invoke } from "./tauri.js";
import { shortId } from "./dom.js";
export const FKEY = "ghostlink_friends";
export const GKEY = "ghostlink_groups";
export const PINV = "ghostlink_pinv"; // invitations à (ré)envoyer
export const GDECL = "ghostlink_declined"; // ids de groupes refusés
export const RTC_CFG = { iceServers: [{ urls: "stun:stun.l.google.com:19302" }] };
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
    inGroupCall: false,
    groupMuted: false,
    groupCallId: null,
    pendingGCall: null,
    gfileOfferId: null,
    // vidéo (WebRTC)
    localCam: null,
    localScreen: null,
    pcs: {},
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
