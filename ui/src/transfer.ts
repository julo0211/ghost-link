// Transfert de fichiers 1-à-1 (envoi/réception + accord) + chat texte + glisser-déposer.

import { invoke, listen } from "./tauri.js";
import { $, log, fmt, etaStr, baseName, addImgBubble } from "./dom.js";
import { S, myName } from "./state.js";

// Images/GIF inline (Task 3.4) : au-delà, pas de chemin fichier disponible pour
// un `File` issu du picker/presse-papiers — repli documenté (log), pas d'échec silencieux.
const MAX_INLINE_IMG = 5 * 1024 * 1024;
/** Devine le mime à partir de l'extension (repli fichier → image reçue). */
function guessImageMime(name: string): string | null {
  const n = name.toLowerCase();
  if (n.endsWith(".png")) return "image/png";
  if (n.endsWith(".gif")) return "image/gif";
  if (n.endsWith(".webp")) return "image/webp";
  if (n.endsWith(".jpg") || n.endsWith(".jpeg")) return "image/jpeg";
  return null;
}

function setFile(path: string): void {
  $<HTMLInputElement>("#filePath").value = path;
  $("#drop").innerHTML = '<span class="big">📄</span> ' + baseName(path);
}

// Chat (texte chiffré par le canal iroh)
function addMsg(text: string, who: string, author?: string): void {
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
async function sendChat(): Promise<void> {
  const text = $<HTMLInputElement>("#chatInput").value.trim();
  if (!text) return;
  try {
    await invoke("send_chat", { text, name: myName() });
    addMsg(text, "me");
    $<HTMLInputElement>("#chatInput").value = "";
  } catch (e) {
    log("Chat : " + e);
  }
}

// Images/GIF inline 1-à-1 : uniquement pour un File issu du picker ou du
// presse-papiers (pas de chemin fichier disponible) — le glisser-déposer garde
// le flux fichier existant (send_file) et se rend inline côté récepteur (repli plus bas).
async function sendImage1to1(f: File): Promise<void> {
  if (f.size > MAX_INLINE_IMG) {
    log("Image > 5 Mo — glisse-la sur la fenêtre pour l'envoyer en fichier.");
    return;
  }
  try {
    const buf = new Uint8Array(await f.arrayBuffer());
    await invoke("send_img", { author: myName(), name: f.name, mime: f.type, data: Array.from(buf) });
    addImgBubble($("#chatLog"), URL.createObjectURL(f), "me");
  } catch (e) {
    log("Image : " + e);
  }
}
function pickAndSendImage(): void {
  const inp = document.createElement("input");
  inp.type = "file";
  inp.accept = "image/png,image/jpeg,image/gif,image/webp";
  inp.onchange = () => {
    const f = inp.files?.[0];
    if (f) void sendImage1to1(f);
  };
  inp.click();
}

export function initTransfer(): void {
  // Envoi (avec débit + annulation)
  $<HTMLButtonElement>("#btnSend").onclick = async () => {
    const path = $<HTMLInputElement>("#filePath").value.trim();
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
    $<HTMLButtonElement>("#btnSend").disabled = true;
    try {
      const name = await invoke("send_file", { path });
      $("#sendBar").style.width = "100%";
      $("#sendPct").textContent = "✅ envoyé";
      log("Fichier envoyé : " + name + " ✅");
    } catch (e) {
      $("#sendPct").textContent = "✗ " + (String(e) === "annulé" ? "annulé" : "erreur");
      log("Envoi : " + e);
    } finally {
      $<HTMLButtonElement>("#btnSend").disabled = false;
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
    if (p.status === "cleaned") log("🧹 Métadonnées retirées avant envoi : " + p.name);
    else if (p.status === "skipped")
      log("⚠️ Métadonnées NON retirées (" + (p.info || "format non pris en charge") + ") — fichier envoyé tel quel : " + p.name);
    else log("⚠️ Nettoyage des métadonnées échoué (" + (p.info || "?") + ") — fichier envoyé tel quel : " + p.name);
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
    if (now - S.sLast < 400) return; // rafraîchir vitesse/ETA au plus 1×/0,4 s
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
    if (now - S.rLast < 400) return;
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
    if (empty) empty.remove();
    const d = document.createElement("div");
    d.className = "xfer";
    d.innerHTML =
      '<span style="font-size:18px">✅</span><div class="meta"><div class="nm"></div><div class="pth"></div></div>';
    (d.querySelector(".nm") as HTMLElement).textContent = name;
    (d.querySelector(".pth") as HTMLElement).textContent = path;
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
        .catch(() => {});
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
    const p = e.payload || ({} as { id?: number; name?: string; size?: number });
    S.fileOfferId = p.id ?? null;
    $("#fileOfferText").textContent =
      '📥 « ' + (p.name || "fichier") + " » (" + fmt(p.size || 0) + ") — accepter ce fichier ?";
    $("#fileOfferBanner").classList.remove("hidden");
  });
  $("#btnFileAccept").onclick = () => {
    if (S.fileOfferId != null) invoke("respond_file", { id: S.fileOfferId, accept: true }).catch(() => {});
    $("#fileOfferBanner").classList.add("hidden");
    S.fileOfferId = null;
  };
  $("#btnFileReject").onclick = () => {
    if (S.fileOfferId != null) invoke("respond_file", { id: S.fileOfferId, accept: false }).catch(() => {});
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
  $<HTMLInputElement>("#chatInput").onkeydown = (e: KeyboardEvent) => {
    if (e.key === "Enter") {
      e.preventDefault();
      sendChat();
    }
  };
  listen("ghost-chat", (e) => {
    addMsg(e.payload.text, "them", e.payload.name);
  });

  // Images/GIF inline : bouton + coller (le glisser-déposer reste sur le flux fichier).
  $("#btnChatImg").onclick = pickAndSendImage;
  $("#chatInput").addEventListener("paste", (e: Event) => {
    const it = (e as ClipboardEvent).clipboardData?.items;
    if (!it) return;
    for (const x of it) {
      if (x.type.startsWith("image/")) {
        const f = x.getAsFile();
        if (f) void sendImage1to1(f);
      }
    }
  });
  listen("ghost-chat-img", (e) => {
    const p = e.payload;
    addImgBubble($("#chatLog"), `data:${p.mime};base64,${p.dataB64}`, "them", p.author);
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
