// Carnet d'amis : rendu, présence, empreintes, demandes d'ami mutuelles.

import { invoke, listen } from "./tauri.js";
import { $, log, shortId } from "./dom.js";
import { S, loadFriends, saveFriends, pushFriendsToBackend, myName } from "./state.js";
import { connectTo } from "./session.js";

export function renderFriends(): void {
  const a = loadFriends();
  const box = $("#friendsList");
  box.innerHTML = "";
  if (!a.length) {
    box.innerHTML = '<div class="empty">Aucun ami enregistré.</div>';
    return;
  }
  a.forEach((f, i) => {
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
      '"></span><span class="grow"></span><button class="iconx" title="Retirer">✕</button>';
    const nm = d.querySelector(".grow") as HTMLElement;
    nm.textContent = f.name;
    nm.title = S.fpCache[f.code] || shortId(f.code);
    if (f.mutual) {
      const bg = document.createElement("span");
      bg.className = "badge";
      bg.textContent = "✓";
      bg.title = "ami mutuel";
      nm.appendChild(bg);
    }
    d.onclick = () => connectTo(f.code);
    (d.querySelector("button") as HTMLElement).onclick = (e: MouseEvent) => {
      e.stopPropagation();
      removeFriend(i);
    };
    box.appendChild(d);
  });
}
function addFriend(name: string, code: string): boolean {
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
function removeFriend(i: number): void {
  const a = loadFriends();
  a.splice(i, 1);
  saveFriends(a);
  renderFriends();
  pushFriendsToBackend();
}

// Présence : sonder les amis pour savoir qui est en ligne (P2P, sans serveur).
async function probeFriend(code: string): Promise<void> {
  S.presence[code] = "checking";
  renderFriends();
  let online = false;
  try {
    online = await invoke("probe", { id: code });
  } catch {
    online = false;
  }
  S.presence[code] = online ? "online" : "offline";
  renderFriends();
}
// BUG-4 : sonder par petits lots (et non tous d'un coup) pour ne pas saturer l'endpoint au démarrage.
export async function refreshPresence(): Promise<void> {
  const a = loadFriends();
  if (!a.length || S.presenceBusy) return;
  S.presenceBusy = true;
  try {
    const B = 3;
    for (let i = 0; i < a.length; i += B) {
      await Promise.all(a.slice(i, i + B).map((f) => probeFriend(f.code)));
    }
  } finally {
    S.presenceBusy = false;
  }
}

// Empreintes d'identité
export async function loadFingerprints(): Promise<void> {
  const a = loadFriends();
  let changed = false;
  for (const f of a) {
    if (!S.fpCache[f.code]) {
      try {
        S.fpCache[f.code] = await invoke("fingerprint", { code: f.code });
        changed = true;
      } catch {
        /* ignore */
      }
    }
  }
  if (changed) renderFriends();
}
export async function showFp(code: string): Promise<void> {
  try {
    $("#myFp").textContent = await invoke("fingerprint", { code });
    $("#fpBox").classList.remove("hidden");
  } catch {
    /* ignore */
  }
}

// Demandes d'ami (mutuelles)
function saveMutual(code: string, name?: string): void {
  if (!code) return;
  const label = name && name.trim() ? name.trim() : "Ami " + String(code).slice(0, 8);
  const a = loadFriends();
  let f = a.find((x) => x.code === code);
  if (!f) {
    f = { name: label, code, mutual: true };
    a.push(f);
  } else {
    f.mutual = true;
    if (name && name.trim()) f.name = name.trim();
  }
  saveFriends(a);
  renderFriends();
  loadFingerprints();
  pushFriendsToBackend();
}

export function initFriends(): void {
  $("#btnAddFriend").onclick = () => {
    if (addFriend($<HTMLInputElement>("#friendName").value, $<HTMLInputElement>("#friendCode").value)) {
      $<HTMLInputElement>("#friendName").value = "";
      $<HTMLInputElement>("#friendCode").value = "";
      log("Ami ajouté.");
    }
  };
  $("#btnRefreshPresence").onclick = refreshPresence;

  $("#btnFreq").onclick = async () => {
    try {
      await invoke("send_freq", { name: myName() });
      log("Demande d'ami envoyée.");
    } catch (e) {
      log("Demande : " + e);
    }
  };
  listen("ghost-freq", async (e) => {
    if (!S.currentPeer) return;
    S.pendingFreqName = e.payload && e.payload.name ? e.payload.name : "";
    // Enregistrer le code PERMANENT de l'autre (pas l'éphémère de la connexion).
    S.pendingFreqCode = e.payload && e.payload.code ? e.payload.code : S.currentPeer;
    let label = S.pendingFreqName.trim();
    if (!label) {
      label = S.fpCache[S.pendingFreqCode];
      if (!label) {
        try {
          label = await invoke("fingerprint", { code: S.pendingFreqCode });
          S.fpCache[S.pendingFreqCode] = label;
        } catch {
          label = shortId(S.pendingFreqCode);
        }
      }
    }
    $("#freqText").textContent = label + " veut t'ajouter en ami.";
    $("#freqBanner").classList.remove("hidden");
  });
  $("#btnFreqAccept").onclick = async () => {
    $("#freqBanner").classList.add("hidden");
    if (!S.currentPeer) return;
    saveMutual(S.pendingFreqCode || S.currentPeer, S.pendingFreqName);
    try {
      await invoke("send_faccept", { name: myName() });
    } catch {
      /* ignore */
    }
    log("Demande d'ami acceptée.");
  };
  $("#btnFreqRefuse").onclick = () => {
    $("#freqBanner").classList.add("hidden");
    log("Demande d'ami refusée.");
  };
  listen("ghost-faccept", (e) => {
    if (!S.currentPeer) return;
    const nm = e.payload && e.payload.name ? e.payload.name : "";
    const code = e.payload && e.payload.code ? e.payload.code : S.currentPeer;
    saveMutual(code, nm);
    log("Ami ajouté (mutuel) ✓" + (nm ? " — " + nm : ""));
  });
}
