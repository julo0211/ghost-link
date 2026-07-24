// Carnet d'amis : rendu, présence, empreintes, demandes d'ami mutuelles.
import { invoke, listen } from "./tauri.js";
import { $, log, shortId, playPing } from "./dom.js";
import { S, loadFriends, saveFriends, pushFriendsToBackend, myName, memberName, ensureFpLabel, loadAliases, setAlias, relabelPeer, } from "./state.js";
import { showTab } from "./session.js";
/** Édition EN LIGNE d'un surnom local : le libellé devient un <input>. Entrée valide,
 *  Échap annule, la perte de focus valide.
 *  S.editingAlias porte le code en cours d'édition : la puce de membre de groupe vit dans
 *  #groupMembers, que refreshGroupCounts re-rend à CHAQUE changement de présence — sans cet
 *  état hors du DOM, un ami qui se connecte détruirait la saisie en cours. */
export function startAliasEdit(host, code, after) {
    if (host.querySelector("input"))
        return;
    S.editingAlias = code;
    const inp = document.createElement("input");
    inp.className = "aliasedit";
    inp.value = loadAliases()[code] || "";
    // Indice = le libellé RÉSOLU, et non host.textContent : ce dernier embarque le contenu
    // des enfants (la pastille « ✓ » d'ami mutuel, par exemple).
    inp.placeholder = memberName(code);
    host.textContent = "";
    host.appendChild(inp);
    inp.focus();
    inp.select();
    let done = false;
    const finish = (commit) => {
        if (done)
            return;
        done = true;
        S.editingAlias = null;
        if (commit) {
            setAlias(code, inp.value);
            // Ré-étiquetage EN PLACE de tout ce qui est déjà à l'écran (messages, bulles image,
            // vignettes vidéo). Surtout PAS un re-rendu des journaux de chat : cela détruirait
            // les images affichées.
            relabelPeer(code);
        }
        after();
    };
    inp.onclick = (e) => e.stopPropagation();
    inp.onblur = () => finish(true);
    inp.onkeydown = (e) => {
        e.stopPropagation(); // ne pas laisser Échap fermer la visionneuse d'image
        if (e.key === "Enter") {
            e.preventDefault();
            finish(true);
        }
        else if (e.key === "Escape") {
            e.preventDefault();
            finish(false);
        }
    };
}
export function renderFriends() {
    const a = loadFriends();
    const box = $("#friendsList");
    box.innerHTML = "";
    if (!a.length) {
        box.innerHTML = '<div class="empty">Aucun ami enregistré.</div>';
        return;
    }
    a.forEach((f) => {
        // Item compact façon Discord : pastille de présence + nom, clic = connexion, ✕ au survol = retirer.
        const d = document.createElement("div");
        d.className = "item";
        const st = S.presence[f.code];
        const pcls = st ? "pdot " + st : "pdot";
        const ptitle = st === "online" ? "en ligne" : st === "checking" ? "vérification…" : "hors ligne";
        d.innerHTML =
            '<span class="' +
                pcls +
                '" title="' +
                ptitle +
                '"></span><span class="grow"></span><button class="iconx" data-act="ren" title="Renommer (surnom local)">✏️</button><button class="iconx" data-act="del" title="Retirer">✕</button>';
        const nm = d.querySelector(".grow");
        // memberName : surnom local d'abord, puis le nom enregistré. Ne PAS lire f.name
        // directement — le surnom serait invisible ici.
        nm.textContent = memberName(f.code);
        nm.title = S.fpCache[f.code] || shortId(f.code);
        if (f.mutual) {
            const bg = document.createElement("span");
            bg.className = "badge";
            bg.textContent = "✓";
            bg.title = "ami mutuel";
            nm.appendChild(bg);
        }
        // Clic = ouvrir l'écran de connexion AVEC le code pré-rempli (pas de connexion
        // instantanée) : l'utilisateur lance ensuite la connexion explicitement.
        d.onclick = () => {
            showTab("connect");
            const inp = $("#peerAddr");
            inp.value = f.code;
            inp.focus();
            log("Prêt à te connecter à « " + memberName(f.code) + " » — clique sur « 🔌 Se connecter ».");
        };
        d.querySelector('[data-act="del"]').onclick = (e) => {
            e.stopPropagation();
            removeFriend(f.code);
        };
        // Le surnom vit dans une carte SÉPARÉE (ghostlink_aliases), pas dans Friend.name :
        // il doit survivre à removeFriend, à un kick, et au fait que saveMutual laisse un pair
        // écraser le nom stocké localement.
        d.querySelector('[data-act="ren"]').onclick = (e) => {
            e.stopPropagation();
            startAliasEdit(nm, f.code, () => renderFriends());
        };
        box.appendChild(d);
    });
}
function addFriend(name, code) {
    name = (name || "").trim();
    code = (code || "").trim();
    if (!name || !code) {
        log("Donne un nom et un code.");
        return false;
    }
    const a = loadFriends();
    if (a.some((f) => f.code === code)) {
        log("Cet ami est déjà enregistré.");
        return false;
    }
    a.push({ name, code });
    saveFriends(a);
    renderFriends();
    pushFriendsToBackend();
    return true;
}
function removeFriend(code) {
    // Par CODE (pas par index) : un index capté au rendu peut être périmé si la liste
    // a été re-rendue entre-temps → on supprimait le mauvais ami et la cible restait.
    saveFriends(loadFriends().filter((f) => f.code !== code));
    renderFriends();
    pushFriendsToBackend();
}
// Présence : sonder les amis pour savoir qui est en ligne (P2P, sans serveur).
async function probeFriend(code) {
    S.presence[code] = "checking";
    renderFriends();
    let online = false;
    try {
        online = await invoke("probe", { id: code });
    }
    catch {
        online = false;
    }
    S.presence[code] = online ? "online" : "offline";
    renderFriends();
}
// BUG-4 : sonder par petits lots (et non tous d'un coup) pour ne pas saturer l'endpoint au démarrage.
export async function refreshPresence() {
    const a = loadFriends();
    if (!a.length || S.presenceBusy)
        return;
    S.presenceBusy = true;
    try {
        const B = 3;
        for (let i = 0; i < a.length; i += B) {
            await Promise.all(a.slice(i, i + B).map((f) => probeFriend(f.code)));
        }
    }
    finally {
        S.presenceBusy = false;
    }
}
// Empreintes d'identité
export async function loadFingerprints() {
    const a = loadFriends();
    let changed = false;
    for (const f of a) {
        if (!S.fpCache[f.code]) {
            try {
                S.fpCache[f.code] = await invoke("fingerprint", { code: f.code });
                changed = true;
            }
            catch {
                /* ignore */
            }
        }
    }
    if (changed)
        renderFriends();
}
export async function showFp(code) {
    try {
        $("#myFp").textContent = await invoke("fingerprint", { code });
        $("#fpBox").classList.remove("hidden");
    }
    catch {
        /* ignore */
    }
}
// Demandes d'ami (mutuelles)
function saveMutual(code, name) {
    if (!code)
        return;
    const label = name && name.trim() ? name.trim() : "Ami " + String(code).slice(0, 8);
    const a = loadFriends();
    let f = a.find((x) => x.code === code);
    if (!f) {
        f = { name: label, code, mutual: true };
        a.push(f);
    }
    else {
        f.mutual = true;
        if (name && name.trim())
            f.name = name.trim();
    }
    saveFriends(a);
    renderFriends();
    loadFingerprints();
    pushFriendsToBackend();
}
// #48 : trace des demandes d'ami SORTANTES en attente (codes) — pour que ghost-faccept
// (déclenché par un pair qui accepte MA demande) ne puisse pas être détourné en FACCEPT
// non sollicité forçant un ajout/écrasement d'ami. Persisté (une réponse peut arriver
// après un redémarrage de l'app).
const PENDING_FREQ_OUT = "ghostlink_pending_freq_out";
function loadPendingFreqOut() {
    try {
        return new Set(JSON.parse(localStorage.getItem(PENDING_FREQ_OUT) || "[]"));
    }
    catch {
        return new Set();
    }
}
function savePendingFreqOut(s) {
    localStorage.setItem(PENDING_FREQ_OUT, JSON.stringify([...s]));
}
function markFreqSent(code) {
    if (!code)
        return;
    const s = loadPendingFreqOut();
    s.add(code);
    // Borne la croissance : ne garder que les ~64 demandes les plus récentes (un Set
    // JS conserve l'ordre d'insertion) — évite une accumulation illimitée de demandes
    // jamais acceptées à travers les sessions.
    const arr = [...s];
    savePendingFreqOut(new Set(arr.slice(-64)));
}
export function initFriends() {
    $("#btnAddFriend").onclick = () => {
        if (addFriend($("#friendName").value, $("#friendCode").value)) {
            $("#friendName").value = "";
            $("#friendCode").value = "";
            log("Ami ajouté.");
        }
    };
    $("#btnRefreshPresence").onclick = refreshPresence;
    $("#btnFreq").onclick = async () => {
        try {
            await invoke("send_freq", { name: myName() });
            if (S.currentPeer)
                markFreqSent(S.currentPeer);
            log("Demande d'ami envoyée.");
        }
        catch (e) {
            log("Demande : " + e);
        }
    };
    listen("ghost-freq", (e) => {
        if (!S.currentPeer)
            return;
        S.pendingFreqName = e.payload && e.payload.name ? e.payload.name : "";
        // Enregistrer le code PERMANENT de l'autre (pas l'éphémère de la connexion).
        S.pendingFreqCode = e.payload && e.payload.code ? e.payload.code : S.currentPeer;
        // Premier barreau PROPRE à ce site : le nom DÉCLARÉ porté par la demande. Les barreaux
        // bas (empreinte → code court) sont factorisés dans memberName/ensureFpLabel — les deux
        // échelles inline (ici et session.ts) ne partageaient QUE ces barreaux-là.
        const paint = () => {
            $("#freqText").textContent = memberName(S.pendingFreqCode, S.pendingFreqName) + " veut t'ajouter en ami.";
        };
        paint();
        void ensureFpLabel(S.pendingFreqCode, paint);
        // Une demande d'ami n'arrive que sur une session déjà établie et acceptée : pas besoin
        // du filtre « ami » appliqué à ghost-incoming.
        playPing("req");
        $("#freqBanner").classList.remove("hidden");
    });
    $("#btnFreqAccept").onclick = async () => {
        $("#freqBanner").classList.add("hidden");
        if (!S.currentPeer)
            return;
        saveMutual(S.pendingFreqCode || S.currentPeer, S.pendingFreqName);
        try {
            await invoke("send_faccept", { name: myName() });
        }
        catch {
            /* ignore */
        }
        log("Demande d'ami acceptée.");
    };
    $("#btnFreqRefuse").onclick = () => {
        $("#freqBanner").classList.add("hidden");
        log("Demande d'ami refusée.");
    };
    listen("ghost-faccept", (e) => {
        if (!S.currentPeer)
            return;
        const nm = e.payload && e.payload.name ? e.payload.name : "";
        const code = e.payload && e.payload.code ? e.payload.code : S.currentPeer;
        // #48 (défensif) : un FACCEPT n'est légitime que si J'avais une demande d'ami
        // sortante en attente sur CETTE connexion. On valide par S.currentPeer (le remote_id
        // AUTHENTIFIÉ de la connexion, = la clé posée par markFreqSent), PAS par `code` : le
        // code permanent auto-déclaré du payload diffère du remote_id éphémère d'un pas-encore-
        // ami, donc tester pending.has(code) rejetterait une acceptation légitime. On stocke
        // ensuite l'ami par son code permanent (`code`).
        const pending = loadPendingFreqOut();
        if (!pending.has(S.currentPeer)) {
            log("⚠️ Acceptation d'ami reçue sans demande en attente — ignorée.");
            return;
        }
        pending.delete(S.currentPeer);
        savePendingFreqOut(pending);
        saveMutual(code, nm);
        log("Ami ajouté (mutuel) ✓" + (nm ? " — " + nm : ""));
    });
}
