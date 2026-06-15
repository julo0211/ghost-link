// Transfert de fichiers 1-à-1 (envoi/réception + accord) + chat texte + glisser-déposer.
import { invoke, listen } from "./tauri.js";
import { $, log, fmt, etaStr, baseName } from "./dom.js";
import { S, myName } from "./state.js";
function setFile(path) {
    $("#filePath").value = path;
    $("#drop").innerHTML = '<span class="big">📄</span> ' + baseName(path);
}
// Chat (texte chiffré par le canal iroh)
function addMsg(text, who, author) {
    const c = $("#chatLog");
    const m = document.createElement("div");
    m.className = "msg " + (who === "me" ? "me" : "them");
    if (who !== "me" && author && author.trim()) {
        const au = document.createElement("div");
        au.style.cssText = "font-size:11px;font-weight:700;opacity:.8;margin-bottom:2px";
        au.textContent = author.trim();
        m.appendChild(au);
    }
    const b = document.createElement("div");
    b.textContent = text;
    const t = document.createElement("span");
    t.className = "t";
    t.textContent = new Date().toLocaleTimeString("fr-FR", { hour: "2-digit", minute: "2-digit" });
    m.appendChild(b);
    m.appendChild(t);
    c.appendChild(m);
    c.scrollTop = c.scrollHeight;
}
async function sendChat() {
    const text = $("#chatInput").value.trim();
    if (!text)
        return;
    try {
        await invoke("send_chat", { text, name: myName() });
        addMsg(text, "me");
        $("#chatInput").value = "";
    }
    catch (e) {
        log("Chat : " + e);
    }
}
export function initTransfer() {
    // Envoi (avec débit + annulation)
    $("#btnSend").onclick = async () => {
        const path = $("#filePath").value.trim();
        if (!path) {
            log("Glisse un fichier (ou colle son chemin).");
            return;
        }
        S.sT = 0;
        S.sB = 0;
        S.sSpd = 0;
        S.sLast = 0;
        $("#sendBox").classList.remove("hidden");
        $("#btnCancelSend").classList.remove("hidden");
        $("#sendName").textContent = baseName(path);
        $("#sendBar").style.width = "0%";
        $("#sendPct").textContent = "0%";
        $("#btnSend").disabled = true;
        try {
            const name = await invoke("send_file", { path });
            $("#sendBar").style.width = "100%";
            $("#sendPct").textContent = "✅ envoyé";
            log("Fichier envoyé : " + name + " ✅");
        }
        catch (e) {
            $("#sendPct").textContent = "✗ " + (String(e) === "annulé" ? "annulé" : "erreur");
            log("Envoi : " + e);
        }
        finally {
            $("#btnSend").disabled = false;
            $("#btnCancelSend").classList.add("hidden");
        }
    };
    $("#btnCancelSend").onclick = () => {
        invoke("cancel_send");
        log("Annulation de l'envoi…");
    };
    // BUG-9 : tant que le pair n'a pas accepté le fichier, la barre reste à 0 % — on l'indique.
    listen("ghost-send-await", () => {
        $("#sendPct").textContent = "⏳ en attente d'acceptation…";
    });
    listen("ghost-send-progress", (e) => {
        const { sent, size } = e.payload;
        const now = performance.now();
        const p = size ? Math.round((sent / size) * 100) : 0;
        $("#sendBar").style.width = p + "%";
        if (S.sT === 0) {
            S.sT = now;
            S.sB = sent;
            S.sLast = now;
            $("#sendPct").textContent = p + "%";
            return;
        }
        if (now - S.sLast < 400)
            return; // rafraîchir vitesse/ETA au plus 1×/0,4 s
        const dt = (now - S.sT) / 1000;
        const inst = dt > 0 ? (sent - S.sB) / dt : 0;
        S.sSpd = S.sSpd > 0 ? S.sSpd * 0.6 + inst * 0.4 : inst; // lissage (moyenne mobile)
        const eta = S.sSpd > 0 ? (size - sent) / S.sSpd : 0;
        $("#sendPct").textContent = p + "% · " + fmt(S.sSpd) + "/s · ⏳ " + etaStr(eta);
        S.sT = now;
        S.sB = sent;
        S.sLast = now;
    });
    // Réception (avec débit + annulation)
    $("#btnCancelRecv").onclick = () => {
        invoke("cancel_recv");
        log("Annulation de la réception…");
    };
    listen("ghost-recv-start", (e) => {
        $("#recvBox").classList.remove("hidden");
        $("#btnCancelRecv").classList.remove("hidden");
        $("#recvName").textContent = e.payload.name;
        $("#recvBar").style.width = "0%";
        $("#recvPct").textContent = "0%";
        S.rT = 0;
        S.rB = 0;
        S.rSpd = 0;
        S.rLast = 0;
        log("⬇️ Réception de « " + e.payload.name + " » (" + fmt(e.payload.size) + ")…");
    });
    listen("ghost-recv-progress", (e) => {
        const { received, size } = e.payload;
        const now = performance.now();
        const p = size ? Math.round((received / size) * 100) : 0;
        $("#recvBar").style.width = p + "%";
        if (S.rT === 0) {
            S.rT = now;
            S.rB = received;
            S.rLast = now;
            $("#recvPct").textContent = p + "%";
            return;
        }
        if (now - S.rLast < 400)
            return;
        const dt = (now - S.rT) / 1000;
        const inst = dt > 0 ? (received - S.rB) / dt : 0;
        S.rSpd = S.rSpd > 0 ? S.rSpd * 0.6 + inst * 0.4 : inst;
        const eta = S.rSpd > 0 ? (size - received) / S.rSpd : 0;
        $("#recvPct").textContent = p + "% · " + fmt(S.rSpd) + "/s · ⏳ " + etaStr(eta);
        S.rT = now;
        S.rB = received;
        S.rLast = now;
    });
    listen("ghost-recv-done", (e) => {
        $("#recvBox").classList.add("hidden");
        const { name, path } = e.payload;
        const empty = $("#recvList").querySelector(".hint");
        if (empty)
            empty.remove();
        const d = document.createElement("div");
        d.className = "xfer";
        d.innerHTML =
            '<span style="font-size:18px">✅</span><div class="meta"><div class="nm"></div><div class="pth"></div></div>';
        d.querySelector(".nm").textContent = name;
        d.querySelector(".pth").textContent = path;
        $("#recvList").prepend(d);
        log("Fichier reçu : " + name);
    });
    listen("ghost-recv-cancel", (e) => {
        $("#recvBox").classList.add("hidden");
        log("Réception annulée : " + ((e.payload && e.payload.name) || ""));
    });
    // Multi-flux (0.20.0) : intégrité SHA-256 invalide → fichier rejeté.
    listen("ghost-recv-corrupt", (e) => {
        $("#recvBox").classList.add("hidden");
        log("⚠️ Fichier corrompu (intégrité invalide) — rejeté : " + ((e.payload && e.payload.name) || ""));
    });
    // Acceptation d'un fichier entrant (avant réception)
    listen("ghost-recv-offer", (e) => {
        const p = e.payload || {};
        S.fileOfferId = p.id ?? null;
        $("#fileOfferText").textContent =
            '📥 « ' + (p.name || "fichier") + " » (" + fmt(p.size || 0) + ") — accepter ce fichier ?";
        $("#fileOfferBanner").classList.remove("hidden");
    });
    $("#btnFileAccept").onclick = () => {
        if (S.fileOfferId != null)
            invoke("respond_file", { id: S.fileOfferId, accept: true }).catch(() => { });
        $("#fileOfferBanner").classList.add("hidden");
        S.fileOfferId = null;
    };
    $("#btnFileReject").onclick = () => {
        if (S.fileOfferId != null)
            invoke("respond_file", { id: S.fileOfferId, accept: false }).catch(() => { });
        $("#fileOfferBanner").classList.add("hidden");
        S.fileOfferId = null;
        log("Fichier refusé.");
    };
    listen("ghost-recv-rejected", (e) => {
        $("#recvBox").classList.add("hidden");
        log("Fichier refusé : " + ((e.payload && e.payload.name) || ""));
    });
    // Chat
    $("#btnChat").onclick = sendChat;
    $("#chatInput").onkeydown = (e) => {
        if (e.key === "Enter") {
            e.preventDefault();
            sendChat();
        }
    };
    listen("ghost-chat", (e) => {
        addMsg(e.payload.text, "them", e.payload.name);
    });
    // Glisser-déposer natif
    listen("tauri://drag-enter", () => $("#drop").classList.add("over"));
    listen("tauri://drag-over", () => $("#drop").classList.add("over"));
    listen("tauri://drag-leave", () => $("#drop").classList.remove("over"));
    listen("tauri://drag-drop", (e) => {
        $("#drop").classList.remove("over");
        const paths = e.payload && e.payload.paths;
        if (paths && paths.length)
            setFile(paths[0]);
    });
}
