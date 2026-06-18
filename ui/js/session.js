// Connexion / session 1-à-1 + demande de connexion entrante + onglets.
import { invoke, listen } from "./tauri.js";
import { $, log, shortId } from "./dom.js";
import { S, loadFriends } from "./state.js";
import { setCallUI, hideCallOffer } from "./call.js";
export function showTab(name) {
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
    $("#connStatus").querySelector(".conn-text").textContent = "Connecté à " + shortId(peer);
    $("#peerLabel").textContent = "Connecté à " + shortId(peer);
    $("#connectForm").classList.add("hidden");
    $("#sessionBox").classList.remove("hidden");
    $("#btnConnect").disabled = false;
    $("#chatCard").classList.remove("hidden");
    showTab("session");
    const convo = document.getElementById("railConvo");
    if (convo) {
        const fr = loadFriends().find((x) => x.code === peer);
        const label = fr ? fr.name : shortId(peer);
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
    hideCallOffer();
    if (S.inCall) {
        invoke("call_stop", { signal: false }).catch(() => { });
        setCallUI(false);
    }
    $("#chatCard").classList.add("hidden");
    $("#chatLog").innerHTML = "";
    $("#chatInput").value = "";
    $("#filePath").value = "";
    $("#drop").innerHTML = '<span class="big">📄</span> Glisse un fichier ici pour l\'envoyer';
}
export function initSession() {
    $("#btnConnect").onclick = () => connectTo($("#peerAddr").value);
    $("#btnDisconnect").onclick = () => invoke("disconnect").catch((e) => log("Déconnexion : " + e));
    // Demande de connexion entrante (accepter / refuser)
    listen("ghost-incoming", async (e) => {
        const p = e.payload || {};
        S.incomingId = p.id ?? null;
        let label = "";
        const f = loadFriends().find((x) => x.code === p.peer);
        if (f) {
            label = f.name;
        }
        else {
            label = S.fpCache[p.peer || ""];
            if (!label) {
                try {
                    label = await invoke("fingerprint", { code: p.peer || "" });
                    S.fpCache[p.peer || ""] = label;
                }
                catch {
                    label = shortId(p.peer || "");
                }
            }
        }
        $("#incomingText").textContent = label + " veut se connecter à toi.";
        $("#incomingBanner").classList.remove("hidden");
    });
    $("#btnAccept").onclick = () => {
        if (S.incomingId != null)
            invoke("respond_incoming", { id: S.incomingId, accept: true }).catch(() => { });
        $("#incomingBanner").classList.add("hidden");
        S.incomingId = null;
        log("Connexion acceptée.");
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
        log("🔗 Connecté à : " + e.payload);
        setConnected(e.payload);
    });
    listen("ghost-disconnected", () => {
        log("Déconnecté.");
        setDisconnected();
    });
    listen("ghost-error", (e) => log("⚠️ " + e.payload));
    listen("ghost-refused", (e) => log("⛔ Connexion refusée (pair pas dans tes amis) : " + shortId(e.payload)));
}
