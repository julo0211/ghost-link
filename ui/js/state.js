// État partagé + couche de données (localStorage). Ce module n'importe AUCUN
// module de domaine → il est la base du graphe (pas de cycle d'imports).
import { invoke } from "./tauri.js";
import { shortId, clampLabel } from "./dom.js";
export const FKEY = "ghostlink_friends";
export const GKEY = "ghostlink_groups";
export const PINV = "ghostlink_pinv"; // invitations à (ré)envoyer
export const GDECL = "ghostlink_declined"; // ids de groupes refusés
export const ALIASKEY = "ghostlink_aliases"; // code → surnom LOCAL choisi par moi
export const GAINKEY = "ghostlink_gains"; // code → gain voix en % (0..200)
export const SGAINKEY = "ghostlink_sgains"; // code → gain du son d'écran en % (0..200)
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
    // Portée : uniquement l'appel où JE suis (pilote `.incall`/`.speaking`).
    voiceAct: {},
    // Présence vocale PAR GROUPE, diffusée à TOUT le groupe (event ghost-voice-presence,
    // beacon ~1 Hz) — même hors appel. gid → (code → lastSeenMs). Pilote `.inbooth`
    // (pastille statique), SÉPARÉE de `.incall`/`.speaking` ci-dessus.
    voicePresence: {},
    gfileOfferId: null,
    // vidéo (WebRTC)
    localCam: null,
    localScreen: null,
    pcs: {},
    // vidéo NATIVE (video.rs) : partage d'écran en cours, sans MediaStream local.
    localScreenNative: false,
    // Non-lu : compteurs PAR GROUPE + session 1-à-1, et focus de la fenêtre.
    // JAMAIS stockés dans le DOM : renderGroups vide #groupList en innerHTML et
    // refreshGroupCounts le re-rend à CHAQUE ghost-mesh-up/down — une pastille peinte
    // dans le DOM disparaîtrait au premier changement de présence.
    unread: {},
    unread1to1: 0,
    focused: true,
    // Code du pair dont le surnom est en cours d'édition. L'input vit dans une puce de
    // #groupMembers que refreshGroupCounts peut re-rendre à tout instant : l'état ne peut
    // donc PAS vivre dans le DOM, sinon un simple changement de présence détruit la saisie.
    editingAlias: null,
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
export function pushFriendsToBackend() {
    const codes = loadFriends().map((f) => f.code);
    invoke("set_friends", { codes }).catch(() => { });
}
// ===== Identité de conversation : UN seul résolveur de libellé de pair =====
/** Un code n'est persistable que s'il est PERMANENT. Les codes d'ami et de membre de
 *  groupe le sont par construction (le mesh dial toujours via net.perm) ; S.currentPeer
 *  ne l'est pas forcément — `connect` peut dialer depuis l'endpoint éphémère, et un code
 *  éphémère est invalidé à la rotation, donc un surnom/volume posé dessus pourrirait.
 *  Garde DÉFENSIVE : sur les surfaces livrées aujourd'hui elle ne retourne jamais null
 *  (on ne renomme que depuis l'item ami et la puce de membre). Elle devient porteuse le
 *  jour où une entrée de renommage apparaît sur la conversation 1-à-1. */
export function persistKey(code) {
    if (!code)
        return null;
    if (loadFriends().some((f) => f.code === code))
        return code;
    if (loadGroups().some((g) => g.members.includes(code)))
        return code;
    return null;
}
/** Plafond du carnet de surnoms. Les surnoms NE sont PAS élagués à la suppression d'un
 *  ami, à une sortie de groupe ni à un kick — c'est précisément leur intérêt de survivre
 *  à ces événements. Seul ce plafond borne la croissance (cf. les 64 de
 *  ghostlink_pending_freq_out). */
const ALIAS_MAX = 256;
export function loadAliases() {
    try {
        return JSON.parse(localStorage.getItem(ALIASKEY) || "{}") || {};
    }
    catch {
        return {};
    }
}
/** Pose (ou retire, si `name` est vide) un surnom local. Refuse un code non permanent. */
export function setAlias(code, name) {
    const key = persistKey(code);
    if (!key)
        return;
    const m = loadAliases();
    const v = clampLabel(name);
    if (v)
        m[key] = v;
    else
        delete m[key];
    const keys = Object.keys(m);
    if (keys.length > ALIAS_MAX)
        for (const k of keys.slice(0, keys.length - ALIAS_MAX))
            delete m[k];
    localStorage.setItem(ALIASKEY, JSON.stringify(m));
}
/** LE résolveur. Échelle, sémantique « vide ⇒ on descend d'un cran » (donc `||`, pas
 *  `??` : le nom déclaré est piloté par le pair, une chaîne vide doit tomber au cran
 *  suivant plutôt que produire une étiquette vide) :
 *
 *    surnom local → nom d'ami → nom DÉCLARÉ (borné) → empreinte → code court
 *
 *  Le nom déclaré est un ÉCHELON et n'est jamais remplacé par le code : les membres
 *  arrivés par convergence de roster sont ajoutés en codes bruts sans nom local, les
 *  afficher en hexadécimal serait une régression par rapport à aujourd'hui. L'appelant
 *  marque ce cas avec la classe .unverified (cf. isDeclaredLabel). */
export function memberName(code, declared) {
    const al = loadAliases()[code];
    if (al)
        return al;
    const f = loadFriends().find((x) => x.code === code);
    if (f && f.name)
        return f.name;
    const d = clampLabel(declared);
    if (d)
        return d;
    const fp = S.fpCache[code];
    if (fp)
        return fp;
    return shortId(code);
}
/** Vrai si le libellé affiché vient du nom AUTO-DÉCLARÉ du pair (donc non vérifiable).
 *  Pilote la classe .unverified. Sans ce marquage, l'utilisateur réapprend à faire
 *  confiance aux noms et le surnom perd sa valeur anti-usurpation : rien ne garantit
 *  l'unicité d'un nom déclaré, et deux pairs peuvent afficher le même. */
export function isDeclaredLabel(code, declared) {
    if (loadAliases()[code])
        return false;
    const f = loadFriends().find((x) => x.code === code);
    if (f && f.name)
        return false;
    return !!clampLabel(declared);
}
// Empreintes en vol / déjà échouées : ensureFpLabel est appelée DEPUIS un rendu et
// redéclenche ce rendu — sans ces deux gardes, un invoke qui échoue rebouclerait à
// l'infini (tempête d'invoke, pas un one-shot).
const fpInFlight = new Set();
const fpFailed = new Set();
/** Remplit S.fpCache[code] puis rappelle `after` pour ré-afficher. Ne résout rien
 *  elle-même : elle factorise les trois barreaux BAS de l'échelle, chaque site d'appel
 *  gardant son premier barreau (nom d'ami pour la connexion entrante, nom déclaré pour
 *  la demande d'ami). No-op si un surnom/nom d'ami existe déjà. */
/** Forme COURTE d'une empreinte, pour un usage d'ÉTIQUETTE (chat, listes, bannières).
 *
 *  `fingerprint` fait désormais 8 groupes (128 bits) parce qu'elle sert de vérification
 *  hors bande. Ce n'est pas une bonne étiquette : elle casserait la mise en page des
 *  bulles et des vignettes. On n'affiche donc que les 4 premiers groupes — exactement ce
 *  qui était affiché avant, donc zéro changement visuel. La vérification, elle, se fait
 *  sur l'empreinte ENTIÈRE (écran Réglages / fiche d'ami), jamais sur cette étiquette. */
export function fpLabel(full) {
    return full.split("-").slice(0, 4).join("-");
}
export async function ensureFpLabel(code, after) {
    if (!code || S.fpCache[code] || fpInFlight.has(code) || fpFailed.has(code))
        return;
    if (loadAliases()[code])
        return;
    if (loadFriends().some((x) => x.code === code && x.name))
        return;
    fpInFlight.add(code);
    try {
        S.fpCache[code] = fpLabel(await invoke("fingerprint", { code }));
        if (after)
            after();
    }
    catch {
        fpFailed.add(code); // cache négatif : ne pas re-tenter en boucle
    }
    finally {
        fpInFlight.delete(code);
    }
}
/** Applique un renommage à TOUT ce qui est déjà à l'écran, EN PLACE.
 *  On ne peut PAS re-rendre les journaux de chat pour ça : renderGroupMsgs fait
 *  clearImgBlobs + innerHTML="" et ne rejoue que S.groupMsgs, qui ne contient AUCUNE
 *  image — toutes les images affichées seraient détruites et leurs blob: révoqués. Et le
 *  chat 1-à-1 n'a même aucun tampon (append-only, vidé à la déconnexion). D'où
 *  l'estampille data-from posée à l'insertion de chaque étiquette d'auteur.
 *  L'événement DOM sert à ré-étiqueter les vignettes vidéo : state.ts ne peut pas
 *  importer groups.ts (cycle d'imports). */
export function relabelPeer(code) {
    const label = memberName(code);
    document.querySelectorAll('[data-from="' + code + '"]').forEach((el) => {
        el.textContent = label;
        el.classList.remove("unverified"); // un surnom vient d'être posé : plus rien de déclaré
    });
    document.dispatchEvent(new CustomEvent("gl-relabel", { detail: code }));
}
// ===== Volumes par pair, persistés =====
/** Lit une carte code → pourcentage en bornant à 0..200. `set_gain` côté Rust n'a AUCUN
 *  clamp (contrairement à set_screen_gain qui borne au moins le bas) et localStorage est
 *  éditable : une valeur aberrante irait droit dans la somme du mixeur. */
function loadPctMap(key) {
    const out = {};
    try {
        const m = JSON.parse(localStorage.getItem(key) || "{}") || {};
        for (const c of Object.keys(m)) {
            const n = Number(m[c]);
            if (Number.isFinite(n))
                out[c] = Math.max(0, Math.min(200, Math.round(n)));
        }
    }
    catch {
        /* carte illisible : on repart à vide plutôt que d'échouer */
    }
    return out;
}
/** Écrit une carte de gains, ÉLAGUÉE aux pairs qu'on peut encore appeler (amis ∪ membres
 *  de groupes) — contrairement aux surnoms, un gain n'a de sens que pour ces gens-là,
 *  sinon la carte grossit d'une entrée par pair jamais croisé, à vie. Les valeurs à
 *  100 % ne sont pas stockées (défaut). */
function savePctMap(key, m) {
    const keep = new Set(loadFriends().map((f) => f.code));
    for (const g of loadGroups())
        for (const c of g.members)
            keep.add(c);
    const out = {};
    for (const c of Object.keys(m))
        if (keep.has(c) && m[c] !== 100)
            out[c] = m[c];
    localStorage.setItem(key, JSON.stringify(out));
}
export function saveGains() {
    savePctMap(GAINKEY, S.groupGains);
}
export function saveSGains() {
    savePctMap(SGAINKEY, S.screenGains);
}
/** Soupape : sans issue de secours visible, un réglage invisible et durable est un piège.
 *  Vide le disque ET la mémoire ; l'appelant re-pousse 1.0 au backend et rafraîchit les
 *  curseurs (sinon on recréerait le désync curseur/backend que ce lot corrige). */
export function resetGains() {
    S.groupGains = {};
    S.screenGains = {};
    localStorage.removeItem(GAINKEY);
    localStorage.removeItem(SGAINKEY);
}
// Hydratation des gains persistés. Placée en fin de module : loadPctMap et S doivent
// tous deux être définis, et les valeurs à 100 % ne sont pas stockées (défaut implicite).
S.groupGains = loadPctMap(GAINKEY);
S.screenGains = loadPctMap(SGAINKEY);
