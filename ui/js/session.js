// Connexion / session 1-à-1 + demande de connexion entrante + onglets.
import { invoke, listen } from "./tauri.js";
import { $, log, clearImgBlobs, playPing } from "./dom.js";
import { S, loadFriends, memberName, ensureFpLabel } from "./state.js";
import { setCallUI, hideCallOffer } from "./call.js";
/** Peint (ou retire) la pastille de non-lu sur l'item de conversation du rail, EN PLACE :
 *  l'item est reconstruit par setConnected, on ne va pas le refaire à chaque message. */
export function paintConvoUnread() {
    const grow = document.querySelector("#railConvo .item .grow");
    if (!grow)
        return;
    grow.querySelector(".badge.unread")?.remove();
    const item = grow.closest(".item");
    if (!S.unread1to1) {
        item?.classList.remove("has-unread");
        return;
    }
    item?.classList.add("has-unread");
    const bg = document.createElement("span");
    bg.className = "badge unread";
    bg.textContent = S.unread1to1 > 99 ? "99+" : String(S.unread1to1);
    grow.appendChild(bg);
}
export function showTab(name) {
    // Portée EXPLICITE : entrer dans la vue « session » efface le compteur 1-à-1, et RIEN
    // d'autre. Les compteurs de groupe ne sont effacés que par openGroup — les vider à
    // l'entrée d'onglet effacerait les pastilles de groupes jamais ouverts, ce qui annulerait
    // la fonctionnalité.
    if (name === "session" && S.unread1to1) {
        S.unread1to1 = 0;
        paintConvoUnread();
    }
    document
        .querySelectorAll("[data-view]")
        .forEach((el) => el.classList.toggle("view-hidden", el.getAttribute("data-view") !== name));
    const grp = name === "group";
    const members = document.getElementById("membersCol");
    if (members)
        members.classList.toggle("view-hidden", !grp);
    const layout = document.getElementById("layout");
    if (layout)
        layout.classList.toggle("no-members", !grp);
}
export async function connectTo(addr) {
    addr = (addr || "").trim();
    if (!addr) {
        log("Entre un code ami ou une adresse.");
        return;
    }
    $("#btnConnect").disabled = true;
    log("Connexion… (en attente de l'acceptation du pair)");
    try {
        await invoke("connect", { addr });
    }
    catch (e) {
        log("Erreur connexion : " + e);
        $("#btnConnect").disabled = false;
    }
}
function setConnected(peer) {
    S.currentPeer = peer;
    $("#connStatus").className = "conn s-ok";
    $("#connStatus").querySelector(".conn-text").textContent = "Connecté à " + memberName(peer);
    $("#peerLabel").textContent = "Connecté à " + memberName(peer);
    $("#connectForm").classList.add("hidden");
    $("#sessionBox").classList.remove("hidden");
    $("#btnConnect").disabled = false;
    $("#chatCard").classList.remove("hidden");
    showTab("session");
    const convo = document.getElementById("railConvo");
    if (convo) {
        const fr = loadFriends().find((x) => x.code === peer); // encore utilisé pour le tag TEMP
        const label = memberName(peer);
        convo.innerHTML = "";
        const it = document.createElement("div");
        it.className = "item active";
        it.innerHTML = '<span class="dot on"></span><span class="grow"></span>';
        it.querySelector(".grow").textContent = label;
        if (!fr) {
            const tag = document.createElement("span");
            tag.className = "tag tmp";
            tag.textContent = "TEMP";
            it.appendChild(tag);
        }
        it.onclick = () => showTab("session");
        convo.appendChild(it);
        paintConvoUnread(); // un compteur peut déjà courir (message reçu avant l'affichage)
    }
}
function setDisconnected() {
    S.currentPeer = null;
    showTab("connect");
    const convo = document.getElementById("railConvo");
    if (convo)
        convo.innerHTML = '<div class="empty">Aucune session.</div>';
    $("#connStatus").className = "conn s-idle";
    $("#connStatus").querySelector(".conn-text").textContent = "Déconnecté";
    $("#sessionBox").classList.add("hidden");
    $("#connectForm").classList.remove("hidden");
    $("#btnConnect").disabled = false;
    $("#sendBox").classList.add("hidden");
    $("#recvBox").classList.add("hidden");
    $("#freqBanner").classList.add("hidden");
    $("#fileOfferBanner").classList.add("hidden");
    S.fileOfferId = null;
    hideCallOffer();
    if (S.inCall) {
        invoke("call_stop", { signal: false }).catch(() => { });
        setCallUI(false);
    }
    $("#chatCard").classList.add("hidden");
    // Libérer les blob: des images du chat avant de vider (sinon ils resteraient alloués
    // pour toute la vie du process, plus aucune référence DOM ne pouvant les libérer).
    clearImgBlobs($("#chatLog"));
    $("#chatLog").innerHTML = "";
    $("#chatInput").value = "";
    $("#filePath").value = "";
    $("#drop").innerHTML = '<span class="big">📄</span> Glisse un fichier ici pour l\'envoyer';
}
export function initSession() {
    $("#btnConnect").onclick = () => connectTo($("#peerAddr").value);
    $("#btnDisconnect").onclick = () => invoke("disconnect").catch((e) => log("Déconnexion : " + e));
    // Demande de connexion entrante (accepter / refuser)
    listen("ghost-incoming", (e) => {
        const p = e.payload || {};
        S.incomingId = p.id ?? null;
        // Premier barreau PROPRE à ce site : le nom d'ami. Cet événement ne porte AUCUN nom
        // déclaré (payload = { id, peer }). Les barreaux bas sont dans ensureFpLabel.
        const code = p.peer || "";
        const paint = () => {
            $("#incomingText").textContent = memberName(code) + " veut se connecter à toi.";
        };
        paint();
        void ensureFpLabel(code, paint);
        // DURCISSEMENT : ne SONNER que si le pair est un ami. ghost-incoming est émis avant
        // toute AUTORISATION applicative — le pair est bien authentifié cryptographiquement
        // (remote_id), mais le filtre « amis uniquement » est DÉSACTIVÉ par défaut et
        // l'anti-flood de 2 s est indexé sur l'identité, donc contournable en faisant tourner
        // ses clés (c'est gratuit). Sans ce filtre, un inconnu peut mitrailler le haut-parleur.
        // La BANNIÈRE, elle, ne change pas de comportement.
        if (loadFriends().some((x) => x.code === code))
            playPing("req");
        $("#incomingBanner").classList.remove("hidden");
    });
    $("#btnAccept").onclick = () => {
        // Si on est déjà connecté, accepter FERME la session en cours : on prévient au lieu
        // de basculer en silence. Refus → on garde la connexion actuelle.
        if (S.currentPeer && !confirm("Tu es déjà connecté à un pair.\nAccepter cette nouvelle connexion FERMERA la session actuelle.\n\nContinuer ?")) {
            if (S.incomingId != null)
                invoke("respond_incoming", { id: S.incomingId, accept: false }).catch(() => { });
            $("#incomingBanner").classList.add("hidden");
            S.incomingId = null;
            log("Nouvelle connexion refusée — session actuelle conservée.");
            return;
        }
        if (S.incomingId != null)
            invoke("respond_incoming", { id: S.incomingId, accept: true }).catch(() => { });
        $("#incomingBanner").classList.add("hidden");
        S.incomingId = null;
        log(S.currentPeer ? "Session précédente fermée — nouvelle connexion acceptée." : "Connexion acceptée.");
    };
    $("#btnRefuse").onclick = () => {
        if (S.incomingId != null)
            invoke("respond_incoming", { id: S.incomingId, accept: false }).catch(() => { });
        $("#incomingBanner").classList.add("hidden");
        S.incomingId = null;
        log("Connexion refusée.");
    };
    listen("ghost-incoming-cancel", () => {
        $("#incomingBanner").classList.add("hidden");
        S.incomingId = null;
    });
    listen("ghost-connected", (e) => {
        // Avant : imprimait le code brut COMPLET dans le journal.
        log("🔗 Connecté à : " + memberName(e.payload));
        setConnected(e.payload);
    });
    listen("ghost-disconnected", () => {
        log("Déconnecté.");
        setDisconnected();
    });
    listen("ghost-refused", (e) => log("⛔ Connexion refusée (pair pas dans tes amis) : " + memberName(e.payload)));
}
