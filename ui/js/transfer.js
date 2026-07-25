// Transfert de fichiers 1-à-1 (envoi/réception + accord) + chat texte + glisser-déposer.
import { invoke, listen } from "./tauri.js";
import { $, log, fmt, etaStr, baseName, addImgBubble, clampLabel, playPing, trimTextBubbles } from "./dom.js";
import { S, myName, memberName, isDeclaredLabel } from "./state.js";
import { paintConvoUnread } from "./session.js";
// Images/GIF inline (Task 3.4) : au-delà, pas de chemin fichier disponible pour
// un `File` issu du picker/presse-papiers — repli documenté (log), pas d'échec silencieux.
const MAX_INLINE_IMG = 5 * 1024 * 1024;
/** Devine le mime à partir de l'extension (repli fichier → image reçue). */
function guessImageMime(name) {
    const n = name.toLowerCase();
    if (n.endsWith(".png"))
        return "image/png";
    if (n.endsWith(".gif"))
        return "image/gif";
    if (n.endsWith(".webp"))
        return "image/webp";
    if (n.endsWith(".jpg") || n.endsWith(".jpeg"))
        return "image/jpeg";
    return null;
}
function setFile(path) {
    $("#filePath").value = path;
    // Le nom de fichier est une DONNÉE : il ne doit pas atteindre un parseur HTML. C'était
    // le seul `innerHTML` du dépôt à concaténer une valeur variable — tout le reste du code
    // suit déjà le motif « gabarit statique + textContent ». Sous Windows `<` et `>` sont
    // interdits dans un nom, mais le code est multi-plateforme et rien ne garantit que ce
    // chemin restera alimenté par le seul glisser-déposer local.
    const box = $("#drop");
    box.textContent = "";
    const ico = document.createElement("span");
    ico.className = "big";
    ico.textContent = "📄";
    box.appendChild(ico);
    box.appendChild(document.createTextNode(" " + baseName(path)));
}
// Chat (texte chiffré par le canal iroh)
function addMsg(text, who, author) {
    const c = $("#chatLog");
    const m = document.createElement("div");
    m.className = "msg " + (who === "me" ? "me" : "them");
    if (who !== "me") {
        // ghost-chat ne transporte AUCUNE identité d'expéditeur (net.rs n'émet que
        // { name, text }, et le spawn par flux de run_conn ne clone jamais `peer`). On résout
        // donc depuis S.currentPeer = remote_id AUTHENTIFIÉ de la session.
        // LIMITE ASSUMÉE : lors d'un échange de session (accepter une connexion entrante ferme
        // la précédente), un message encore EN VOL du pair sortant s'afficherait sous le
        // libellé du nouveau pair. Fenêtre étroite — le log est vidé à la déconnexion — et
        // strictement meilleur que l'état antérieur, où le nom affiché était intégralement
        // usurpable. Correctif propre = 3 lignes de Rust, hors de ce lot.
        const peer = S.currentPeer || "";
        const label = peer ? memberName(peer, author) : clampLabel(author);
        if (label) {
            const au = document.createElement("div");
            au.className = "auth" + (peer && isDeclaredLabel(peer, author) ? " unverified" : "");
            au.style.cssText = "font-size:11px;font-weight:700;opacity:.8;margin-bottom:2px";
            if (peer)
                au.dataset.from = peer;
            au.textContent = label;
            m.appendChild(au);
        }
    }
    const b = document.createElement("div");
    b.textContent = text;
    const t = document.createElement("span");
    t.className = "t";
    t.textContent = new Date().toLocaleTimeString("fr-FR", { hour: "2-digit", minute: "2-digit" });
    m.appendChild(b);
    m.appendChild(t);
    c.appendChild(m);
    // Le chat 1-à-1 n'avait AUCUN plafond (contrairement au groupe, dont le tampon est
    // borné à 200) : le DOM grandissait indéfiniment. Élaguer AVANT le scroll.
    trimTextBubbles(c);
    c.scrollTop = c.scrollHeight;
}
/** Un message 1-à-1 vient d'arriver : bip, puis compteur de non-lu si je ne suis pas en
 *  train de le regarder (fenêtre au second plan, ou vue « session » masquée). */
function noteIncoming1to1() {
    playPing("msg");
    const sess = document.querySelector('[data-view="session"]');
    const visible = !!sess && !sess.classList.contains("view-hidden");
    if (!S.focused || !visible) {
        S.unread1to1++;
        paintConvoUnread();
    }
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
// Images/GIF inline 1-à-1 : uniquement pour un File issu du picker ou du
// presse-papiers (pas de chemin fichier disponible) — le glisser-déposer garde
// le flux fichier existant (send_file) et se rend inline côté récepteur (repli plus bas).
async function sendImage1to1(f) {
    if (f.size > MAX_INLINE_IMG) {
        log("Image > 5 Mo — glisse-la sur la fenêtre pour l'envoyer en fichier.");
        return;
    }
    try {
        const buf = new Uint8Array(await f.arrayBuffer());
        await invoke("send_img", { author: myName(), name: f.name, mime: f.type, data: Array.from(buf) });
        addImgBubble($("#chatLog"), URL.createObjectURL(f), "me");
    }
    catch (e) {
        log("Image : " + e);
    }
}
function pickAndSendImage() {
    const inp = document.createElement("input");
    inp.type = "file";
    inp.accept = "image/png,image/jpeg,image/gif,image/webp";
    inp.onchange = () => {
        const f = inp.files?.[0];
        if (f)
            void sendImage1to1(f);
    };
    inp.click();
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
        invoke("cancel_send").catch((e) => log("Annulation : " + e));
        log("Annulation de l'envoi…");
    };
    // BUG-9 : tant que le pair n'a pas accepté le fichier, la barre reste à 0 % — on l'indique.
    listen("ghost-send-await", () => {
        $("#sendPct").textContent = "⏳ en attente d'acceptation…";
    });
    // Nettoyage des métadonnées avant envoi (meta.rs) : rendre le résultat VISIBLE —
    // succès rassurant, et surtout jamais d'échec silencieux (confidentialité).
    listen("ghost-meta", (e) => {
        const p = e.payload;
        if (p.status === "cleaned")
            log("🧹 Métadonnées retirées avant envoi : " + p.name);
        else if (p.status === "skipped")
            log("⚠️ Métadonnées NON retirées (" + (p.info || "format non pris en charge") + ") — fichier envoyé tel quel : " + p.name);
        else
            log("⚠️ Nettoyage des métadonnées échoué (" + (p.info || "?") + ") — fichier envoyé tel quel : " + p.name);
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
        invoke("cancel_recv").catch((e) => log("Annulation : " + e));
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
        // Repli : grosse image (> 5 Mo) reçue via le flux fichier classique → rendu
        // inline dans le chat en plus de l'entrée "fichier reçu" ci-dessus.
        const mime = guessImageMime(name);
        if (mime) {
            invoke("read_image_bytes", { path })
                .then((bytes) => {
                const url = URL.createObjectURL(new Blob([new Uint8Array(bytes)], { type: mime }));
                addImgBubble($("#chatLog"), url, "them");
            })
                .catch(() => { });
        }
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
    // SEC-2 : espace disque insuffisant → fichier refusé automatiquement.
    listen("ghost-recv-nospace", (e) => {
        $("#recvBox").classList.add("hidden");
        log("⚠️ Espace disque insuffisant — fichier refusé : " + ((e.payload && e.payload.name) || ""));
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
        noteIncoming1to1();
        addMsg(e.payload.text, "them", e.payload.name);
    });
    // Images/GIF inline : bouton + coller (le glisser-déposer reste sur le flux fichier).
    $("#btnChatImg").onclick = pickAndSendImage;
    $("#chatInput").addEventListener("paste", (e) => {
        const it = e.clipboardData?.items;
        if (!it)
            return;
        for (const x of it) {
            if (x.type.startsWith("image/")) {
                const f = x.getAsFile();
                if (f)
                    void sendImage1to1(f);
            }
        }
    });
    listen("ghost-chat-img", (e) => {
        const p = e.payload;
        noteIncoming1to1();
        const peer = S.currentPeer || "";
        addImgBubble($("#chatLog"), `data:${p.mime};base64,${p.dataB64}`, "them", peer ? memberName(peer, p.author) : clampLabel(p.author), peer || undefined, !!(peer && isDeclaredLabel(peer, p.author)));
    });
    // Glisser-déposer natif
    listen("tauri://drag-enter", () => $("#drop").classList.add("over"));
    listen("tauri://drag-over", () => $("#drop").classList.add("over"));
    listen("tauri://drag-leave", () => $("#drop").classList.remove("over"));
    listen("tauri://drag-drop", (e) => {
        $("#drop").classList.remove("over");
        const paths = e.payload && e.payload.paths;
        if (paths && paths.length) {
            if (paths.length > 1) {
                log("Un seul fichier à la fois — « " + baseName(paths[0]) + " » sélectionné, " + (paths.length - 1) + " ignoré(s).");
            }
            setFile(paths[0]);
        }
    });
}
