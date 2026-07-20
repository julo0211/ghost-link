// Groupes : channel multi-pairs (chat), appel de groupe (audio), vidéo (WebRTC), fichiers.

import { invoke, listen } from "./tauri.js";
import { $, log, fmt, addImgBubble } from "./dom.js";
import {
  S,
  PINV,
  GDECL,
  iceConfig,
  nativeVideoWanted,
  loadGroups,
  saveGroups,
  loadFriends,
  friendsOnly,
  memberName,
  myName,
  type Group,
  type PInvItem,
} from "./state.js";
import { showTab } from "./session.js";

// ----- Invitations en attente (BUG-1 : fiables, ré-envoyées à la reconnexion) -----
function loadPInv(): PInvItem[] {
  try {
    return (JSON.parse(localStorage.getItem(PINV) || "") as PInvItem[]) || [];
  } catch {
    return [];
  }
}
function savePInv(a: PInvItem[]): void {
  localStorage.setItem(PINV, JSON.stringify(a));
}
function addPInv(member: string, gid: string, name: string, csv: string): void {
  const a = loadPInv();
  if (!a.some((x) => x.member === member && x.gid === gid)) {
    a.push({ member, gid, name, csv });
    savePInv(a);
  }
}
function clearPInvGroup(gid: string): void {
  savePInv(loadPInv().filter((x) => x.gid !== gid));
}
function flushPInv(member: string): void {
  const mine = loadPInv().filter((x) => x.member === member);
  if (!mine.length) return;
  // Ne retirer chaque entrée QU'APRÈS le succès de son propre envoi : si l'invoke échoue
  // (pair redevenu injoignable pendant le flap), l'entrée reste pour le réessai 60 s —
  // sinon l'invitation « fiable » serait définitivement perdue. L'anti-doublon côté
  // récepteur ignore un groupe déjà rejoint/refusé, donc un double-envoi est inoffensif.
  mine.forEach((x) =>
    invoke("send_ginvite", { member: x.member, gid: x.gid, name: x.name, members: x.csv })
      .then(() => savePInv(loadPInv().filter((y) => !(y.member === x.member && y.gid === x.gid))))
      .catch(() => {}),
  );
}
function declinedGroups(): string[] {
  try {
    return (JSON.parse(localStorage.getItem(GDECL) || "") as string[]) || [];
  } catch {
    return [];
  }
}
function addDeclined(id: string): void {
  const a = declinedGroups();
  if (!a.includes(id)) {
    a.push(id);
    localStorage.setItem(GDECL, JSON.stringify(a));
  }
}

// ----- Rendu des groupes / membres -----
function updateGroupLine(g: Group): void {
  const total = g.members.length + 1;
  const online = 1 + g.members.filter((c) => S.meshOnline.has(c)).length;
  $("#groupMembersLine").textContent = "👥 " + total + " membres · " + online + " en ligne";
}
function renderGroupMembers(g: Group): void {
  const box = $("#groupMembers");
  if (!box || !g) return;
  box.innerHTML = "";
  const callActive = S.inGroupCall && S.groupCallId === g.id;
  const chip = (code: string, label: string, online: boolean, self: boolean): HTMLElement => {
    const c = document.createElement("div");
    c.className = "mem";
    c.dataset.code = self ? "me" : code; // cible des mises à jour d'activité vocale
    // État initial d'activité — seulement si CE groupe est celui de mon appel.
    const act = S.inGroupCall && S.groupCallId === g.id ? S.voiceAct[self ? "me" : code] : undefined;
    if (act?.inCall) c.classList.add("incall");
    if (act?.speaking) c.classList.add("speaking");
    const d = document.createElement("span");
    d.className = online ? "dot on" : "dot";
    const t = document.createElement("span");
    t.className = "grow";
    t.textContent = label;
    c.appendChild(d);
    c.appendChild(t);
    if (!self) {
      // Vote d'exclusion (60 % des en-ligne). Discret, apparaît au survol du chip.
      const kick = document.createElement("button");
      kick.className = "memkick";
      kick.type = "button";
      kick.textContent = "🚫";
      kick.title = "Voter pour exclure ce membre";
      kick.onclick = (e: MouseEvent) => {
        e.stopPropagation();
        castKick(g, code);
      };
      c.appendChild(kick);
    }
    if (callActive && !self) {
      const r = document.createElement("input");
      r.type = "range";
      r.min = "0";
      r.max = "200";
      r.step = "5";
      r.value = String(S.groupGains[code] != null ? S.groupGains[code] : 100);
      r.title = "Volume";
      r.style.cssText = "width:64px;flex:0 0 auto";
      const pct = document.createElement("span");
      pct.style.cssText = "font-size:11px;color:var(--muted);min-width:34px;text-align:right;flex:0 0 auto;font-variant-numeric:tabular-nums";
      pct.textContent = r.value + "%";
      r.oninput = () => {
        S.groupGains[code] = +r.value;
        pct.textContent = r.value + "%";
        invoke("group_call_volume", { peer: code, vol: +r.value / 100 }).catch(() => {});
      };
      c.appendChild(r);
      c.appendChild(pct);
    }
    return c;
  };
  box.appendChild(chip("moi", "Moi", true, true));
  g.members.forEach((code) => box.appendChild(chip(code, memberName(code), S.meshOnline.has(code), false)));
  paintVoicePresence(); // ré-applique la pastille « dans le vocal » après le re-render des chips
}
// Présence vocale groupe (Task 4.3) : pastille STATIQUE « dans le vocal » (.inbooth),
// diffusée à TOUT le groupe via le beacon ~1 Hz (startGroupCall/stopGroupCall) + TTL
// côté récepteur (initGroups, ghost-voice-presence). Visible même hors appel — à ne
// PAS confondre avec `.incall`/`.speaking` (ghost-voice-activity), qui ne concernent
// que les participants de MON appel. Les deux classes coexistent sur un même chip.
function paintVoicePresence(): void {
  const gid = S.openGroupId || "";
  const present = S.voicePresence[gid] || {};
  let n = 0;
  document.querySelectorAll(".mem[data-code]").forEach((el) => {
    const code = (el as HTMLElement).dataset.code || "";
    const on = code === "me" ? !!(S.inGroupCall && S.groupCallId === gid) : !!present[code];
    el.classList.toggle("inbooth", on);
    if (on) n++;
  });
  const hdr = $("#groupVoiceCount");
  if (hdr) hdr.textContent = n ? "🔊 " + n + " dans le vocal" : "";
}
function refreshGroupCounts(): void {
  const g = loadGroups().find((x) => x.id === S.openGroupId);
  if (g) {
    updateGroupLine(g);
    renderGroupMembers(g);
  }
  renderGroups();
}
export function renderGroupFriends(): void {
  const box = $("#groupFriends");
  if (!box) return;
  box.innerHTML = "";
  const fr = loadFriends();
  if (!fr.length) {
    box.innerHTML = '<span class="hint">Ajoute d\'abord des amis (onglet Amis).</span>';
    return;
  }
  fr.forEach((f) => {
    const lab = document.createElement("label");
    lab.className = "row";
    lab.style.cssText = "gap:8px;cursor:pointer";
    const cb = document.createElement("input");
    cb.type = "checkbox";
    cb.value = f.code;
    cb.style.cssText = "width:auto;flex:0 0 auto";
    const sp = document.createElement("span");
    sp.style.fontSize = "13.5px";
    sp.textContent = f.name + (f.mutual ? "" : " (non mutuel)");
    lab.appendChild(cb);
    lab.appendChild(sp);
    box.appendChild(lab);
  });
}
// ----- Vote-kick décentralisé (≥ 60 % des membres EN LIGNE) -----
// Un mesh sans serveur ne peut pas EXPULSER de force : chaque client tallie les votes
// qu'il reçoit et, au quorum, retire la cible de SA liste + la met en liste noire de
// groupe (pour qu'une sync de roster ne la ré-ajoute pas). Advisory : un client
// malveillant peut s'ignorer, mais les honnêtes le lâchent.
const KICK_TTL = 5 * 60 * 1000; // votes périmés après 5 min
function kickKey(gid: string, target: string): string {
  return gid + "|" + target;
}
function kickedSet(gid: string): Set<string> {
  try {
    const m = JSON.parse(localStorage.getItem("ghostlink_kicked") || "{}") as Record<string, string[]>;
    return new Set(m[gid] || []);
  } catch {
    return new Set();
  }
}
function saveKicked(gid: string, codes: Set<string>): void {
  let m: Record<string, string[]> = {};
  try {
    m = JSON.parse(localStorage.getItem("ghostlink_kicked") || "{}") as Record<string, string[]>;
  } catch {
    /* repart d'un objet vide */
  }
  if (codes.size) m[gid] = Array.from(codes);
  else delete m[gid];
  localStorage.setItem("ghostlink_kicked", JSON.stringify(m));
}
// Diffuse MON set de membres pour ce groupe (union côté récepteur). `to` = destinataires
// (défaut : tous les membres du groupe). Les tombstones de kick (kickedSet) restent LOCAUX
// et ne partent JAMAIS sur le fil : la convergence des exclusions passe par le re-gossip
// de votes authentifiés (voir myVotes / ghost-mesh-up), pas par des tombstones.
function broadcastRoster(g: { id: string; name: string; members: string[] }, to?: string[]): void {
  const roster = [S.myCode, ...g.members].join(",");
  invoke("send_gmembers", { members: to ?? g.members, gid: g.id, name: g.name, roster }).catch(() => {});
}
// Mes votes de kick persistés par groupe : localStorage["ghostlink_myvotes"] = { gid: [cibles] }.
// Uniquement MES propres votes — re-gossipés (authentifiés, voter = S.myCode) à un pair qui
// (re)vient en ligne pour que sa tally atteigne le quorum même s'il a manqué le vote initial.
function myVotes(gid: string): string[] {
  try {
    const m = JSON.parse(localStorage.getItem("ghostlink_myvotes") || "{}") as Record<string, string[]>;
    return m[gid] || [];
  } catch {
    return [];
  }
}
function saveMyVote(gid: string, target: string, on: boolean): void {
  let m: Record<string, string[]> = {};
  try {
    m = JSON.parse(localStorage.getItem("ghostlink_myvotes") || "{}") as Record<string, string[]>;
  } catch {
    /* repart d'un objet vide */
  }
  const s = new Set(m[gid] || []);
  if (on) s.add(target);
  else s.delete(target);
  if (s.size) m[gid] = Array.from(s);
  else delete m[gid];
  localStorage.setItem("ghostlink_myvotes", JSON.stringify(m));
}
function onlineMemberCount(g: Group): number {
  return 1 + g.members.filter((c) => S.meshOnline.has(c)).length; // + moi
}
function kickQuorum(g: Group): number {
  // 60 % des en-ligne, plancher à 2 (jamais d'exclusion « solo »).
  return Math.max(2, Math.ceil(0.6 * onlineMemberCount(g)));
}
function recordKickVote(gid: string, target: string, voter: string): number {
  const key = kickKey(gid, target);
  const now = Date.now();
  const votes = (S.kickVotes[key] = S.kickVotes[key] || {});
  votes[voter] = now;
  for (const v of Object.keys(votes)) if (now - votes[v] > KICK_TTL) delete votes[v];
  return Object.keys(votes).length;
}
function applyKick(g: Group, target: string): void {
  const groups = loadGroups();
  const gi = groups.find((x) => x.id === g.id);
  if (!gi || !gi.members.includes(target)) return;
  gi.members = gi.members.filter((c) => c !== target);
  saveGroups(groups);
  const s = kickedSet(g.id);
  s.add(target);
  saveKicked(g.id, s);
  delete S.kickVotes[kickKey(g.id, target)];
  renderGroups();
  if (S.openGroupId === g.id) refreshGroupCounts();
  // Tombstone LOCAL uniquement : PAS de gossip du tombstone (invariant de sécurité). La
  // convergence chez les pairs hors-ligne/late-joiners se fait par re-gossip de MES votes.
  log("🚫 " + memberName(target) + " exclu du groupe « " + g.name + " » (vote atteint).");
}
function tallyKick(g: Group, target: string): void {
  const n = Object.keys(S.kickVotes[kickKey(g.id, target)] || {}).length;
  const q = kickQuorum(g);
  if (n >= q) {
    if (target === S.myCode) {
      // C'est MOI qui suis exclu : quitter le groupe et le dire (sinon je resterais
      // dedans à envoyer/recevoir sans que personne ne me voie).
      leaveGroupSelf(g, "🚫 Tu as été exclu du groupe « " + g.name + " » (vote des membres).");
    } else {
      applyKick(g, target);
    }
  } else if (S.openGroupId === g.id) {
    log("🗳️ Exclure " + memberName(target) + " : " + n + "/" + q + " votes.");
  }
}
/// Je quitte le groupe (exclu par vote) : arrêt d'appel/partage, retrait local, message.
function leaveGroupSelf(g: Group, msg: string): void {
  if (S.inGroupCall && S.groupCallId === g.id) stopGroupCall();
  const groups = loadGroups().filter((x) => x.id !== g.id);
  saveGroups(groups);
  delete S.kickVotes[kickKey(g.id, S.myCode)];
  if (S.openGroupId === g.id) closeGroup();
  renderGroups();
  log(msg);
}
function castKick(g: Group, target: string): void {
  if (!S.myCode || target === S.myCode || !g.members.includes(target)) return;
  if (!confirm("Voter pour exclure " + memberName(target) + " du groupe « " + g.name + " » ?")) return;
  recordKickVote(g.id, target, S.myCode);
  invoke("send_kick", { members: g.members, gid: g.id, target, voter: S.myCode }).catch(() => {});
  saveMyVote(g.id, target, true); // mémoriser MON vote → re-gossip aux pairs qui reviennent en ligne
  tallyKick(g, target);
}

// ----- Ajouter des membres à un groupe existant -----
function renderAddMembersFriends(g: Group): void {
  const box = $("#addMembersFriends");
  box.replaceChildren();
  const inGroup = new Set([S.myCode, ...g.members]);
  const cand = loadFriends().filter((f) => f.code && !inGroup.has(f.code));
  if (!cand.length) {
    const s = document.createElement("span");
    s.className = "hint";
    s.textContent = "Tous tes amis sont déjà dans ce groupe.";
    box.appendChild(s);
    return;
  }
  cand.forEach((f) => {
    const lab = document.createElement("label");
    lab.className = "row";
    lab.style.cssText = "gap:6px;cursor:pointer;flex:0 0 auto";
    const cb = document.createElement("input");
    cb.type = "checkbox";
    cb.value = f.code;
    cb.style.cssText = "width:auto;flex:0 0 auto";
    const sp = document.createElement("span");
    sp.style.fontSize = "13px";
    sp.textContent = f.name;
    lab.appendChild(cb);
    lab.appendChild(sp);
    box.appendChild(lab);
  });
}
function addMembersToGroup(g: Group, newCodes: string[]): void {
  if (!newCodes.length || !S.myCode) return;
  const groups = loadGroups();
  const gi = groups.find((x) => x.id === g.id);
  if (!gi) return;
  // Ré-ajout explicite : lever la liste noire LOCALE pour ces personnes (ré-invitation) ET
  // cesser de re-gossiper mon vote contre elles (sinon je les kickerais à nouveau chez les
  // autres). Advisory : les votes obsolètes des autres expirent (TTL) ou tombent au ré-ajout.
  const kset = kickedSet(g.id);
  newCodes.forEach((c) => {
    kset.delete(c);
    saveMyVote(g.id, c, false);
  });
  saveKicked(g.id, kset);
  gi.members = Array.from(new Set([...gi.members, ...newCodes])).filter((c) => c && c !== S.myCode);
  saveGroups(groups);
  const rosterCsv = [S.myCode, ...gi.members].join(",");
  // Les NOUVEAUX : invitation (bannière chez eux) + mise en attente si hors ligne.
  newCodes.forEach((code) => {
    addPInv(code, g.id, g.name, rosterCsv);
    invoke("send_ginvite", { member: code, gid: g.id, name: g.name, members: rosterCsv }).catch(() => {});
  });
  // Les membres DÉJÀ présents : sync du roster (ils font l'union des membres).
  broadcastRoster(gi);
  invoke("open_group", { members: friendsOnly(gi.members) }).catch(() => {});
  renderGroups();
  refreshGroupCounts();
  log("➕ " + newCodes.length + " membre(s) ajouté(s) au groupe « " + g.name + " ».");
}
export function renderGroups(): void {
  const box = $("#groupList");
  if (!box) return;
  box.innerHTML = "";
  const gs = loadGroups();
  if (!gs.length) {
    box.innerHTML = '<div class="empty">Aucun groupe.</div>';
    return;
  }
  gs.forEach((g, i) => {
    // Item compact façon Discord : avatar (initiale) + nom, clic = ouvrir, ✕ au survol = supprimer/quitter.
    const d = document.createElement("div");
    d.className = "item" + (g.id === S.openGroupId ? " active" : "");
    const av = document.createElement("span");
    av.className = "av";
    av.textContent = (g.name.trim()[0] || "#").toUpperCase();
    const onl = 1 + g.members.filter((c) => S.meshOnline.has(c)).length;
    const nm = document.createElement("span");
    nm.className = "grow";
    nm.textContent = g.name;
    nm.title = g.members.length + 1 + " membres · " + onl + " en ligne";
    const del = document.createElement("button");
    del.className = "iconx";
    del.textContent = "✕";
    del.title = "Supprimer / quitter";
    del.onclick = (e: MouseEvent) => {
      e.stopPropagation();
      if (!confirm('Supprimer / quitter le groupe « ' + g.name + " » ?")) return;
      // Si un appel de CE groupe est en cours (même s'il n'est pas le groupe OUVERT),
      // raccrocher AVANT de le supprimer : sinon micro/partage continueraient d'émettre
      // vers un groupe disparu et aucun bouton ne permettrait plus de raccrocher.
      if (S.inGroupCall && S.groupCallId === g.id) stopGroupCall();
      const a = loadGroups();
      a.splice(i, 1);
      saveGroups(a);
      clearPInvGroup(g.id);
      myVotes(g.id).forEach((t) => saveMyVote(g.id, t, false)); // ne plus re-gossiper mes votes d'un groupe quitté
      delete S.groupMsgs[g.id]; // purge de l'historique de chat du groupe supprimé
      delete S.voicePresence[g.id]; // purge des pastilles « dans le vocal » de ce groupe
      renderGroups();
      if (S.openGroupId === g.id) closeGroup();
    };
    d.onclick = () => openGroup(g.id);
    d.appendChild(av);
    d.appendChild(nm);
    d.appendChild(del);
    box.appendChild(d);
  });
}
function openGroup(id: string, skipDial?: boolean): void {
  const g = loadGroups().find((x) => x.id === id);
  if (!g) return;
  S.openGroupId = id;
  $("#groupChannelName").textContent = "👪 " + g.name;
  updateGroupLine(g);
  renderGroupMembers(g);
  renderGroups(); // BUG : déplacer le surlignage « actif » vers le groupe ouvert (sinon il reste collé au 1er).
  $("#groupChannelCard").classList.remove("hidden");
  $("#addMembersPanel").classList.add("hidden");
  showTab("group");
  if (!skipDial) invoke("open_group", { members: friendsOnly(g.members) }).catch(() => {});
  renderGroupMsgs();
  refreshGroupCallUI();
}
function closeGroup(): void {
  // Ne couper la vidéo QUE si l'appel en cours est celui du groupe fermé — stopGroupCall
  // appelle déjà stopVideo. Fermer le panneau d'un AUTRE groupe pendant que je suis en
  // appel ailleurs ne doit PAS tuer la vidéo/les partages de cet appel actif.
  if (S.inGroupCall && S.groupCallId === S.openGroupId) stopGroupCall();
  S.openGroupId = null;
  $("#groupChannelCard").classList.add("hidden");
  showTab("connect");
}

// ----- Chat de groupe -----
function addGroupMsgDom(author: string, text: string, who: string): void {
  const box = $("#groupChatLog");
  const m = document.createElement("div");
  m.className = "msg " + (who === "me" ? "me" : "them");
  if (who !== "me" && author) {
    const au = document.createElement("div");
    au.style.cssText = "font-size:11px;font-weight:700;opacity:.8;margin-bottom:2px";
    au.textContent = author;
    m.appendChild(au);
  }
  const b = document.createElement("div");
  b.textContent = text;
  m.appendChild(b);
  box.appendChild(m);
  box.scrollTop = box.scrollHeight;
}
function renderGroupMsgs(): void {
  const box = $("#groupChatLog");
  box.innerHTML = "";
  (S.groupMsgs[S.openGroupId || ""] || []).forEach((m) => addGroupMsgDom(m.author, m.text, m.who));
}
function pushGroupMsg(id: string, author: string, text: string, who: string): void {
  (S.groupMsgs[id] = S.groupMsgs[id] || []).push({ author, text, who });
  if (S.groupMsgs[id].length > 200) S.groupMsgs[id].shift();
  if (S.openGroupId === id) addGroupMsgDom(author, text, who);
}
function sendGroupMsg(): void {
  if (!S.openGroupId) return;
  const text = $<HTMLInputElement>("#groupChatInput").value.trim();
  if (!text) return;
  const g = loadGroups().find((x) => x.id === S.openGroupId);
  if (!g) return;
  invoke("send_gchat", { members: g.members, gid: g.id, name: myName(), text }).catch((e) => log("Groupe : " + e));
  pushGroupMsg(g.id, myName() || "moi", text, "me");
  $<HTMLInputElement>("#groupChatInput").value = "";
}

// ----- Images/GIF inline de groupe (Task 3.4) -----
// Même limite que le 1-à-1 : au-delà, pas de chemin fichier disponible pour un
// File du picker/presse-papiers — le glisser-déposer garde le flux fichier
// existant (send_gfile), sans rendu inline côté groupe (voir listener
// ghost-grecv-done plus bas : l'événement groupe ne porte pas de `path`).
const MAX_INLINE_GIMG = 5 * 1024 * 1024;
async function sendImageGroup(f: File): Promise<void> {
  if (!S.openGroupId) return;
  const g = loadGroups().find((x) => x.id === S.openGroupId);
  if (!g) return;
  if (f.size > MAX_INLINE_GIMG) {
    log("Image > 5 Mo — glisse-la sur la fenêtre pour l'envoyer en fichier.");
    return;
  }
  try {
    const buf = new Uint8Array(await f.arrayBuffer());
    await invoke("send_gimg", {
      members: g.members,
      gid: g.id,
      author: myName(),
      name: f.name,
      mime: f.type,
      data: Array.from(buf),
    });
    // L'utilisateur a pu ouvrir un AUTRE groupe pendant les await (arrayBuffer + invoke) :
    // ne pas écrire ma bulle dans le #groupChatLog d'un groupe différent de celui d'envoi.
    if (S.openGroupId !== g.id) return;
    const box = $("#groupChatLog");
    const u = URL.createObjectURL(f);
    addImgBubble(box, u, "me");
    // Révoquer le blob une fois l'image affichée (sinon fuite mémoire cumulative : une bulle
    // effacée par un changement de groupe garderait son objet URL jusqu'à la fermeture).
    const img = box.lastElementChild?.querySelector("img") as HTMLImageElement | null;
    if (img) img.onload = () => URL.revokeObjectURL(u);
  } catch (e) {
    log("Image de groupe : " + e);
  }
}
function pickAndSendImageGroup(): void {
  const inp = document.createElement("input");
  inp.type = "file";
  inp.accept = "image/png,image/jpeg,image/gif,image/webp";
  inp.onchange = () => {
    const f = inp.files?.[0];
    if (f) void sendImageGroup(f);
  };
  inp.click();
}

// ----- Appel de groupe (audio) -----
// Beacon de présence vocale (Task 4.3) : tant que JE suis en appel de groupe, annonce
// ~1 Hz "je suis dans le vocal de ce groupe" à TOUT le groupe (pas seulement les
// participants de l'appel) — voir S.voicePresence / paintVoicePresence / ghost-voice-presence.
let voiceBeacon = 0;
function refreshGroupCallUI(): void {
  const active = S.inGroupCall && S.groupCallId === S.openGroupId;
  const b = $<HTMLButtonElement>("#btnGroupCall");
  if (b) b.textContent = active ? "📵 Raccrocher" : "📞 Appel de groupe";
  const m = $("#btnGroupMute");
  if (m) m.classList.toggle("hidden", !active);
  const s = $("#groupCallStatus");
  if (s)
    s.textContent = active
      ? S.groupMuted
        ? "🔇 Micro coupé"
        : "🔊 En appel"
      : S.inGroupCall
        ? "🔊 En appel (autre groupe)"
        : "";
  const g = loadGroups().find((x) => x.id === S.openGroupId);
  if (g) renderGroupMembers(g);
}
async function startGroupCall(g: Group, announce: boolean): Promise<void> {
  $<HTMLButtonElement>("#btnGroupCall").disabled = true;
  try {
    await invoke("group_call_start", { members: g.members, gid: g.id, announce });
    S.inGroupCall = true;
    S.groupCallId = g.id;
    S.groupMuted = false;
    if (voiceBeacon) clearInterval(voiceBeacon);
    voiceBeacon = window.setInterval(() => {
      const gg = loadGroups().find((x) => x.id === S.groupCallId);
      if (gg) invoke("voice_presence", { members: gg.members, gid: gg.id, inCall: true }).catch(() => {});
    }, 1000);
    refreshGroupCallUI();
    log("📞 Appel de groupe " + (announce ? "lancé" : "rejoint") + ".");
  } catch (e) {
    log("Appel groupe : " + e);
    const s = $("#groupCallStatus");
    if (s) s.textContent = "erreur : " + e;
  } finally {
    $<HTMLButtonElement>("#btnGroupCall").disabled = false;
  }
}
function stopGroupCall(): void {
  invoke("group_call_stop").catch(() => {});
  if (voiceBeacon) {
    clearInterval(voiceBeacon);
    voiceBeacon = 0;
  }
  // Beacon final "je quitte le vocal" — AVANT de remettre S.groupCallId à null, sinon
  // on ne saurait plus quel groupe/quels membres notifier (les autres pairs attendraient
  // sinon la péremption TTL de 4 s pour éteindre ma pastille).
  const gg = loadGroups().find((x) => x.id === S.groupCallId);
  if (gg) invoke("voice_presence", { members: gg.members, gid: gg.id, inCall: false }).catch(() => {});
  S.inGroupCall = false;
  S.groupCallId = null;
  refreshGroupCallUI();
  stopVideo();
  log("Appel de groupe terminé.");
}

// ----- Vidéo de groupe : webcam + partage d'écran (WebRTC) -----
// Repli natif du SON d'écran : WebView2 ne capture jamais l'audio d'une fenêtre
// partagée (MicrosoftEdge/WebView2Feedback#4327). Quand l'utilisateur veut le son
// mais que getDisplayMedia ne fournit AUCUNE piste audio, Rust capte le son système
// (loopback WASAPI) et l'envoie en Opus sur le maillage — audible par les membres
// DANS l'appel de groupe. Ce drapeau dit si cette capture native est active.
let screenAudioNative = false;
// Après un échec du partage AVEC audio (rejet en bloc de getDisplayMedia), le retry
// vidéo-seule immédiat peut lui-même échouer : l'activation utilisateur (~5 s après
// le clic, consommée par le 1er appel — Chromium 111+) est morte. Ce drapeau fait
// que le PROCHAIN clic sur 🖥️ Écran repart directement sans audio navigateur.
let retryVideoOnly = false;
// Verrou de réentrance : S.localScreen n'est posé qu'APRÈS l'await getDisplayMedia,
// donc sans ce drapeau un double-clic sur 🖥️ Écran lancerait deux sélecteurs et le
// premier flux fuirait (diffusé aux pairs, inarrêtable depuis l'UI).
let screenBusy = false;
// Même verrou de réentrance pour la caméra : S.localCam n'est posé qu'APRÈS l'await
// getUserMedia — sans ce drapeau un double-clic sur 📹 Caméra lancerait deux captures et
// le premier flux fuirait (diffusé aux pairs, inarrêtable depuis l'UI).
let camBusy = false;
// L'encodeur MATÉRIEL H.264 est confirmé actif → le partage a été remonté en 1080p.
// Jamais posé sur encodeur logiciel (le 1080p logiciel ramènerait le plancher 3 fps).
let screenUpgraded = false;
function vgrid(): any {
  return $("#groupVideos");
}
function maybeHideGrid(): void {
  const g = vgrid();
  if (g && !g.children.length) g.classList.add("hidden");
}
// Agrandir une vignette (cam / partage d'écran) en plein écran. En plein écran,
// le CSS bascule en object-fit:contain pour voir TOUT l'écran partagé sans rognage.
function toggleTileFullscreen(el: HTMLElement): void {
  if (document.fullscreenElement) {
    document.exitFullscreen().catch(() => {});
    if (document.fullscreenElement === el) return; // c'était cette vignette : on l'a juste refermée
  }
  if (document.fullscreenElement !== el && el.requestFullscreen) {
    el.requestFullscreen().catch(() => log("Plein écran indisponible sur cette vue."));
  }
}
function showTile(key: string, label: string, stream: MediaStream, self: boolean, peer?: string): void {
  let w = document.getElementById("vidw_" + key);
  if (!w) {
    w = document.createElement("div");
    w.id = "vidw_" + key;
    w.className = "vidtile";
    w.style.cssText = "position:relative;border-radius:12px;overflow:hidden;background:#000;cursor:pointer";
    w.title = "Cliquer pour agrandir (plein écran)";
    const v = document.createElement("video");
    v.id = "vid_" + key;
    v.autoplay = true;
    v.playsInline = true;
    // Vignette recréée (redémarrage du partage, renégociation) : refléter l'état de
    // coupure DÉJÀ choisi pour ce pair, sinon on afficherait 🔊 sur un son coupé.
    v.muted = !!self || (!!peer && !!S.screenMuted[peer]);
    v.style.cssText = "width:100%;aspect-ratio:4/3;object-fit:cover;display:block";
    const tag = document.createElement("div");
    tag.style.cssText = "position:absolute;bottom:4px;left:6px;font-size:11px;font-weight:700;color:#fff;text-shadow:0 1px 3px #000";
    tag.textContent = label;
    const max = document.createElement("button");
    max.className = "vidmax";
    max.type = "button";
    max.textContent = "⛶";
    max.title = "Plein écran";
    const wrap = w;
    max.onclick = (e: MouseEvent) => {
      e.stopPropagation();
      toggleTileFullscreen(wrap);
    };
    w.onclick = () => toggleTileFullscreen(wrap);
    w.appendChild(v);
    w.appendChild(tag);
    w.appendChild(max);
    if (!self && peer) attachStreamAudio(w, peer, v);
    vgrid().appendChild(w);
  }
  (document.getElementById("vid_" + key) as HTMLVideoElement).srcObject = stream;
  vgrid().classList.remove("hidden");
}
function dropTile(key: string): void {
  const w = document.getElementById("vidw_" + key);
  if (w) w.remove();
  maybeHideGrid();
}
// Contrôles audio du partage d'un pair, sur sa vignette : bouton 🔇/🔊 (raccourci
// mute) + curseur de VOLUME 0–200 % (« le stream qu'on regarde »). L'état est PAR PAIR
// (S.screenMuted / S.screenGains) : un pair peut avoir plusieurs vignettes (cam +
// écran), toutes synchronisées via les sélecteurs data-snd / data-vol. Le son passe
// par le mixeur natif de l'appel (screen_audio_gain) et, si présent, par la piste
// WebRTC du <video> (video.volume, plafonné à 1.0).
function applyStreamGain(peer: string, video?: HTMLVideoElement): void {
  const muted = !!S.screenMuted[peer];
  const pct = S.screenGains[peer] ?? 100;
  invoke("screen_audio_gain", { peer, vol: muted ? 0 : pct / 100 }).catch(() => {});
  // Piloter AUSSI l'état .muted du <video> WebRTC : showTile le fige à la création
  // depuis S.screenMuted, donc sans le réécrire ici une vignette recréée en muet ne
  // se démuterait jamais (le curseur/le bouton ne toucheraient que le volume).
  document.querySelectorAll('[id^="vidw_' + peer + '_"] video').forEach((el) => {
    const v = el as HTMLVideoElement;
    v.muted = muted;
    v.volume = Math.min(1, pct / 100);
  });
  if (video) {
    video.muted = muted;
    video.volume = Math.min(1, pct / 100);
  }
}
function attachStreamAudio(w: HTMLElement, peer: string, video?: HTMLVideoElement): void {
  const snd = document.createElement("button");
  snd.className = "vidsnd";
  snd.type = "button";
  snd.dataset.snd = peer;
  snd.title = "Couper / remettre le son de ce partage";
  const syncIcons = (): void => {
    document.querySelectorAll('[data-snd="' + peer + '"]').forEach((b) => {
      (b as HTMLElement).textContent = S.screenMuted[peer] ? "🔇" : "🔊";
    });
  };
  snd.textContent = S.screenMuted[peer] ? "🔇" : "🔊";
  snd.onclick = (e: MouseEvent) => {
    e.stopPropagation();
    S.screenMuted[peer] = !S.screenMuted[peer];
    applyStreamGain(peer);
    syncIcons();
  };
  w.appendChild(snd);
  // Curseur de volume (apparaît au survol, comme les boutons de la vignette).
  const vol = document.createElement("input");
  vol.type = "range";
  vol.min = "0";
  vol.max = "200";
  vol.step = "5";
  vol.className = "vidvol";
  vol.dataset.vol = peer;
  vol.value = String(S.screenGains[peer] ?? 100);
  vol.title = "Volume du partage";
  vol.onclick = (e: MouseEvent) => e.stopPropagation(); // ne pas déclencher le plein écran
  vol.oninput = () => {
    const v = +vol.value;
    S.screenGains[peer] = v;
    if (v > 0 && S.screenMuted[peer]) S.screenMuted[peer] = false; // bouger = démuter
    applyStreamGain(peer);
    syncIcons();
    document.querySelectorAll('[data-vol="' + peer + '"]').forEach((r) => {
      (r as HTMLInputElement).value = String(v);
    });
  };
  w.appendChild(vol);
  // Ré-affirmer l'état au backend : après une reconnexion, receive_group_voice a pu
  // réinitialiser le gain à 1.0 — sans ça, l'icône dirait 🔇 mais le son jouerait.
  applyStreamGain(peer, video);
}
// Vignette à CANVAS (vidéo native décodée par WebCodecs — pas de MediaStream).
// Même enveloppe que showTile : plein écran au clic, bouton son du partage pour un
// pair (le son natif SCREEN_TAG passe par le mixeur de l'appel, pas par la vignette).
function showCanvasTile(key: string, label: string, peer?: string): HTMLCanvasElement {
  let w = document.getElementById("vidw_" + key);
  if (!w) {
    w = document.createElement("div");
    w.id = "vidw_" + key;
    w.className = "vidtile";
    w.style.cssText = "position:relative;border-radius:12px;overflow:hidden;background:#000;cursor:pointer";
    w.title = "Cliquer pour agrandir (plein écran)";
    const c = document.createElement("canvas");
    c.id = "vid_" + key;
    c.style.cssText = "width:100%;aspect-ratio:4/3;object-fit:cover;display:block";
    const tag = document.createElement("div");
    tag.style.cssText = "position:absolute;bottom:4px;left:6px;font-size:11px;font-weight:700;color:#fff;text-shadow:0 1px 3px #000";
    tag.textContent = label;
    const max = document.createElement("button");
    max.className = "vidmax";
    max.type = "button";
    max.textContent = "⛶";
    max.title = "Plein écran";
    const wrap = w;
    max.onclick = (e: MouseEvent) => {
      e.stopPropagation();
      toggleTileFullscreen(wrap);
    };
    w.onclick = () => toggleTileFullscreen(wrap);
    w.appendChild(c);
    w.appendChild(tag);
    w.appendChild(max);
    if (peer) attachStreamAudio(w, peer); // pas de <video> ici : gain natif seulement
    vgrid().appendChild(w);
  }
  vgrid().classList.remove("hidden");
  return document.getElementById("vid_" + key) as HTMLCanvasElement;
}
function dropPeerTiles(peer: string): void {
  // Nettoyage WebRTC uniquement : la vignette du partage NATIF (suffixe _natscr)
  // partage le préfixe d'id mais vit sur le flux QUIC, indépendant des
  // RTCPeerConnection — un échec ICE (caméra) ne doit pas la détruire.
  document.querySelectorAll('[id^="vidw_' + peer + '_"]').forEach((w) => {
    if (!w.id.endsWith(NATIVE_KEY)) w.remove();
  });
  maybeHideGrid();
}
function sigSend(peer: string, payload: unknown): void {
  invoke("send_signal", { peer, data: JSON.stringify(payload) }).catch(() => {});
}
function localStreams(): MediaStream[] {
  return [S.localCam, S.localScreen].filter(Boolean) as MediaStream[];
}
// Deux profils d'adaptation distincts (VID-4, yo-yo 1080p↔144p) :
// - CAMÉRA : contentHint=motion + maintain-framerate — fluidité d'abord, la
//   résolution peut descendre, personne ne lit du texte sur une webcam.
// - ÉCRAN : contentHint=detail + maintain-resolution — désactive le quality scaler
//   QP de libwebrtc, dont l'UNIQUE axe d'adaptation est la résolution : c'est lui
//   qui pompait 1080p→144p→1080p en boucle (adaptation.md libwebrtc : « The QP
//   scaler is only enabled when the degradation_preference is MAINTAIN_FRAMERATE
//   or BALANCED »). La congestion se paie désormais en fps (texte toujours
//   lisible), et le plafond écran passe de 5 à 3 Mb/s : 5 dépassait la capacité
//   réelle des liens et entretenait les cycles overshoot/chute de goog-cc.
function tuneVideoSender(pc: RTCPeerConnection, track: MediaStreamTrack): void {
  if (track.kind !== "video") return;
  const isScreen = !!(S.localScreen && S.localScreen.getTracks().includes(track));
  try {
    (track as MediaStreamTrack & { contentHint?: string }).contentHint = isScreen ? "detail" : "motion";
  } catch {
    /* ignore */
  }
  const sender = pc.getSenders().find((se) => se.track === track);
  if (!sender) return;
  try {
    const p = sender.getParameters();
    if (!p.encodings || !p.encodings.length) p.encodings = [{}];
    p.encodings[0].maxFramerate = 30;
    // Écran : 3 Mb/s en 720p (base), 4,5 Mb/s une fois remonté en 1080p sur
    // encodeur matériel confirmé (VID-6) — sinon une renégociation écraserait
    // le débit relevé par maybeUpgradeScreen.
    p.encodings[0].maxBitrate = isScreen ? (screenUpgraded ? 4_500_000 : 3_000_000) : 5_000_000;
    (p as RTCRtpSendParameters & { degradationPreference?: string }).degradationPreference =
      isScreen ? "maintain-resolution" : "maintain-framerate";
    // Échec visible en DevTools : un setParameters avalé en silence peut masquer des
    // réglages jamais appliqués (encodings encore vides avant la 1re négociation).
    void sender.setParameters(p).catch((err) => console.warn("setParameters:", err));
  } catch (err) {
    console.warn("tuneVideoSender:", err);
  }
}
// Ré-appliquer le réglage une fois la négociation « stable » : sur d'anciens moteurs,
// encodings est vide avant la 1re offre et setParameters échoue en silence — après
// l'answer, il existe toujours et le plafond fps/bitrate prend à coup sûr.
function retuneSenders(pc: RTCPeerConnection): void {
  pc.getSenders().forEach((se) => {
    if (se.track) tuneVideoSender(pc, se.track);
  });
}
// VID-6 : préférer H.264 sur chaque transceiver vidéo. Chromium n'encode VP8/VP9
// qu'en LOGICIEL (libvpx, un cœur CPU saturé = tout l'historique yo-yo/3 fps),
// alors que H.264 passe par l'encodeur MATÉRIEL du GPU (Media Foundation :
// NVENC/AMF/QuickSync). Les deux extrémités sont le même moteur WebView2 → le
// décodage H.264 est garanti. À appeler côté offreur (avant la négociation) ET
// côté répondeur (ontrack, avant l'answer — c'est l'ordre de l'ANSWER qui gagne).
function preferH264(tr: RTCRtpTransceiver): void {
  try {
    // receiver.track existe toujours (même sans média) et porte le kind du
    // transceiver : ne toucher QUE la vidéo (poser des codecs vidéo sur un
    // transceiver AUDIO — piste audio navigateur du partage — serait rejeté).
    if (!tr.receiver.track || tr.receiver.track.kind !== "video") return;
    // Caps du RÉCEPTEUR : depuis Chromium M124, setCodecPreferences valide la liste
    // contre RTCRtpReceiver.getCapabilities — une liste bâtie sur les caps du sender
    // peut contenir un codec inconnu du receiver et tout rejeter (silencieusement ici).
    const caps = RTCRtpReceiver.getCapabilities("video");
    if (!caps || !caps.codecs.length) return;
    const isH264 = (c: RTCRtpCodec) => /h264/i.test(c.mimeType);
    const h264 = caps.codecs.filter(isH264);
    if (!h264.length) return; // pas de H.264 sur ce moteur : négociation par défaut
    // packetization-mode=1 d'abord (le profil le plus largement accéléré).
    h264.sort(
      (a, b) =>
        Number(/packetization-mode=1/.test(b.sdpFmtpLine || "")) -
        Number(/packetization-mode=1/.test(a.sdpFmtpLine || "")),
    );
    tr.setCodecPreferences([...h264, ...caps.codecs.filter((c) => !isH264(c))]);
  } catch {
    /* transceiver arrêté ou API absente : la négociation par défaut s'applique */
  }
}
// ---- VID-6 : montée adaptative 720p → 1080p sur encodeur matériel CONFIRMÉ ----
// En maillage, CHAQUE RTCPeerConnection a son propre encodeur pour le même track :
// la décision d'upgrade doit être UNANIME (un seul pair en logiciel prendrait du
// 1080p logiciel = retour du plancher 3-5 fps). Liste BLANCHE matérielle : tout nom
// inconnu est traité comme logiciel (mode d'échec conservateur = rester en 720p).
const HW_ENCODER = /MediaFoundation|ExternalEncoder|VideoToolbox|NVENC|QSV|AMF|VAAPI|HardwareVideoEncoder/i;
let screenEncoderChecked = false; // verdict « logiciel » mémorisé pour CE partage
let screenCheckBusy = false; // une seule chaîne de vérification à la fois
let screenWatchdog: number | null = null;

/// Le sender VIDÉO du partage d'écran sur un pc (jamais l'audio : la piste audio
/// navigateur arrive AVANT la vidéo dans getTracks() et n'a pas d'encoderImplementation).
function screenVideoSenderOf(pc: RTCPeerConnection): RTCRtpSender | undefined {
  return pc
    .getSenders()
    .find(
      (se) =>
        se.track && se.track.kind === "video" && S.localScreen && S.localScreen.getTracks().includes(se.track),
    );
}

/// encoderImplementation de l'encodeur écran d'UN pc ("" = pas encore disponible).
async function screenEncoderImpl(pc: RTCPeerConnection): Promise<string> {
  const sender = screenVideoSenderOf(pc);
  if (!sender) return "";
  try {
    const stats = await sender.getStats();
    let impl = "";
    stats.forEach((r) => {
      const o = r as unknown as { type?: string; encoderImplementation?: string };
      if (o.type === "outbound-rtp" && o.encoderImplementation) impl = o.encoderImplementation;
    });
    return impl;
  } catch {
    return "";
  }
}

function applyScreenBitrate(bps: number): void {
  Object.values(S.pcs).forEach((st) => {
    const se = screenVideoSenderOf(st.pc);
    if (!se) return;
    try {
      const p = se.getParameters();
      if (p.encodings && p.encodings.length) {
        p.encodings[0].maxBitrate = bps;
        void se.setParameters(p).catch((err) => console.warn("setParameters:", err));
      }
    } catch (err) {
      console.warn("bitrate écran:", err);
    }
  });
}

// Vérifie quel encodeur tourne VRAIMENT (stats WebRTC) sur TOUS les pairs : upgrade
// seulement à l'unanimité matérielle. Ré-essaie quelques fois : encoderImplementation
// n'apparaît qu'après les premières trames encodées.
function maybeUpgradeScreen(attempt = 0): void {
  if (screenUpgraded || screenEncoderChecked || screenCheckBusy || attempt > 3 || !S.localScreen) return;
  const track = S.localScreen.getVideoTracks()[0];
  if (!track) return;
  screenCheckBusy = true;
  // Le verrou screenCheckBusy est tenu par TOUTE la chaîne de retries (une seule chaîne à
  // la fois). On ne le relâche qu'à la fin : soit on a replanifié un retry (verrou gardé,
  // la planification interne réutilise le même verrou au lieu de repasser par le guard),
  // soit la chaîne se termine (verrou relâché). Ne PAS relâcher/reprendre entre deux
  // essais éviterait la course où le finally du parent rouvrait la porte à une 2e chaîne.
  const attemptCheck = (n: number): void => {
    setTimeout(async () => {
      let replanned = false;
      try {
        if (screenUpgraded || screenEncoderChecked || !S.localScreen || !S.localScreen.getVideoTracks().includes(track)) return;
        const pcs = Object.values(S.pcs)
          .map((st) => st.pc)
          .filter((pc) => screenVideoSenderOf(pc));
        if (!pcs.length) return; // pas encore de sender écran : un prochain « stable » relancera
        const impls = await Promise.all(pcs.map(screenEncoderImpl));
        if (impls.some((i) => !i || /unknown/i.test(i))) {
          if (n < 3) {
            replanned = true; // au moins un pair n'a pas encore de stats — garder le verrou
            attemptCheck(n + 1);
          }
          return;
        }
        const soft = impls.find((i) => !HW_ENCODER.test(i));
        if (soft !== undefined) {
          screenEncoderChecked = true; // verdict pour CE partage : ne plus re-vérifier ni re-logger
          log("ℹ️ Encodeur logiciel détecté (" + soft + ") — le partage reste en 720p30.");
          return;
        }
        // Re-vérification de vie APRÈS les await : le partage a pu s'arrêter entre-temps.
        if (screenUpgraded || !S.localScreen || !S.localScreen.getVideoTracks().includes(track)) return;
        screenUpgraded = true;
        try {
          await track.applyConstraints({
            width: { max: 1920 },
            height: { max: 1080 },
            frameRate: { ideal: 30, max: 30 },
          });
        } catch {
          screenUpgraded = false;
          return;
        }
        applyScreenBitrate(4_500_000);
        log("🎮 Encodeur matériel confirmé sur tous les pairs — partage remonté en 1080p30.");
        armScreenWatchdog(track);
      } finally {
        if (!replanned) screenCheckBusy = false;
      }
    }, 2500 + n * 3000);
  };
  attemptCheck(attempt);
}

// Après l'upgrade : re-vérification périodique. libwebrtc peut retomber en LOGICIEL
// en cours de flux (échec Media Foundation, sessions GPU épuisées — y compris à cause
// de la reconfiguration 1080p elle-même) sans AUCUN événement JS ; et un pair qui
// REJOINT après l'upgrade peut n'obtenir qu'un encodeur logiciel. Dans les deux cas :
// redescendre en 720p30 et ne plus retenter pour ce partage.
function armScreenWatchdog(track: MediaStreamTrack): void {
  disarmScreenWatchdog();
  screenWatchdog = window.setInterval(async () => {
    if (!screenUpgraded || !S.localScreen || !S.localScreen.getVideoTracks().includes(track)) {
      disarmScreenWatchdog();
      return;
    }
    const pcs = Object.values(S.pcs)
      .map((st) => st.pc)
      .filter((pc) => screenVideoSenderOf(pc));
    const impls = await Promise.all(pcs.map(screenEncoderImpl));
    const bad = impls.find((i) => i && !/unknown/i.test(i) && !HW_ENCODER.test(i));
    if (bad === undefined) return;
    screenUpgraded = false;
    screenEncoderChecked = true;
    disarmScreenWatchdog();
    try {
      await track.applyConstraints({ width: { max: 1280 }, height: { max: 720 }, frameRate: { ideal: 30, max: 30 } });
    } catch {
      /* le track a pu se terminer : rien à faire */
    }
    applyScreenBitrate(3_000_000);
    log("ℹ️ Encodeur logiciel apparu (" + bad + ") — partage redescendu en 720p30.");
  }, 12_000);
}
function disarmScreenWatchdog(): void {
  if (screenWatchdog != null) {
    clearInterval(screenWatchdog);
    screenWatchdog = null;
  }
}
function getPc(peer: string) {
  if (S.pcs[peer]) return S.pcs[peer];
  const pc = new RTCPeerConnection(iceConfig());
  const st = { pc, makingOffer: false, polite: (S.myCode || "") < peer };
  S.pcs[peer] = st;
  localStreams().forEach((s) =>
    s.getTracks().forEach((t) => {
      try {
        pc.addTrack(t, s);
        tuneVideoSender(pc, t);
      } catch {
        /* ignore */
      }
    }),
  );
  pc.getTransceivers().forEach(preferH264); // AVANT la 1re négociation (VID-6)
  pc.onnegotiationneeded = async () => {
    try {
      st.makingOffer = true;
      await pc.setLocalDescription();
      sigSend(peer, { description: pc.localDescription });
    } catch (e) {
      log("Vidéo: " + e);
    } finally {
      st.makingOffer = false;
    }
  };
  pc.onicecandidate = (ev) => {
    if (ev.candidate) sigSend(peer, { candidate: ev.candidate });
  };
  pc.ontrack = (ev) => {
    // Côté répondeur : ontrack arrive pendant setRemoteDescription, AVANT l'answer —
    // c'est le bon moment pour imposer H.264 (l'ordre de l'answer fait foi).
    if (ev.transceiver) preferH264(ev.transceiver);
    const stream = ev.streams[0];
    if (!stream) return;
    const key = peer + "_" + stream.id; // une vignette par flux (cam ET écran)
    showTile(key, memberName(peer), stream, false, peer);
    const drop = () => dropTile(key);
    // VID-2 : NE PAS retirer sur `onmute` — un mute survient sur une perte de paquets
    // transitoire (puis unmute) ; retirer ferait clignoter/disparaître la vignette à tort.
    ev.track.onended = drop;
    stream.onremovetrack = () => {
      if (!stream.getTracks().length) drop();
    };
  };
  pc.onconnectionstatechange = () => {
    // VID-2 : `disconnected` est souvent transitoire (il peut repasser `connected`).
    // On ne nettoie que sur un état réellement terminal.
    if (["failed", "closed"].includes(pc.connectionState)) {
      dropPeerTiles(peer);
      // Fermer ET retirer le pc mort de S.pcs : sinon la garde `!S.pcs[peer]` (ghost-mesh-up
      // VID-1) court-circuiterait la nouvelle offre quand ce pair revient (il ne recevrait
      // plus jamais le flux), et un sender écran fantôme fausserait maybeUpgradeScreen/
      // armScreenWatchdog. Ne supprimer que si l'entrée pointe encore CE pc (pas un remplaçant).
      try {
        pc.close();
      } catch {
        /* ignore */
      }
      if (S.pcs[peer] && S.pcs[peer].pc === pc) delete S.pcs[peer];
    }
  };
  return st;
}
function ensureGroupPcs(): void {
  const g = loadGroups().find((x) => x.id === S.openGroupId);
  if (g) g.members.forEach((code) => getPc(code));
}
function addStreamToPcs(stream: MediaStream): void {
  Object.values(S.pcs).forEach((st) => {
    stream.getTracks().forEach((t) => {
      if (!st.pc.getSenders().some((se) => se.track === t)) {
        try {
          st.pc.addTrack(t, stream);
          tuneVideoSender(st.pc, t);
        } catch {
          /* ignore */
        }
      }
    });
    st.pc.getTransceivers().forEach(preferH264); // les nouveaux transceivers (VID-6)
  }
  );
}
function removeStreamFromPcs(stream: MediaStream): void {
  Object.values(S.pcs).forEach((st) =>
    st.pc
      .getSenders()
      .filter((se) => se.track && stream.getTracks().includes(se.track))
      .forEach((se) => {
        try {
          st.pc.removeTrack(se);
        } catch {
          /* ignore */
        }
      }),
  );
}
function videoPrivacyOk(): boolean {
  if (localStorage.getItem("ghostlink_video_ok") === "1") return true;
  const ok = confirm(
    "⚠️ Confidentialité — la caméra et le partage d'écran ouvrent une connexion vidéo DIRECTE (WebRTC) : ton adresse IP devient visible par les membres du groupe, et un serveur STUN public (Google) est contacté. Le chat, les fichiers et la voix restent relayés. Activer la vidéo quand même ?",
  );
  if (ok) localStorage.setItem("ghostlink_video_ok", "1");
  return ok;
}
// Comme Discord : on ne partage cam/écran QUE pendant l'appel de groupe
// (sinon le maillage des autres n'est pas prêt et personne ne voit le flux).
function inThisCall(): boolean {
  return S.inGroupCall && S.groupCallId === S.openGroupId;
}
async function startCam(): Promise<void> {
  if (camBusy) return; // capture déjà en cours de démarrage (double-clic)
  if (!loadGroups().some((x) => x.id === S.openGroupId)) {
    log("Ouvre un groupe d'abord.");
    return;
  }
  if (!inThisCall()) {
    log("📞 Rejoins d'abord l'appel de groupe — comme sur Discord, la caméra se partage dans l'appel.");
    return;
  }
  // Même garde que startScreen : le confirm() du tout premier usage vidéo consomme
  // l'activation utilisateur — getUserMedia est moins strict que getDisplayMedia,
  // mais on garde un comportement identique et prévisible.
  const firstPrivacy = localStorage.getItem("ghostlink_video_ok") !== "1";
  if (!videoPrivacyOk()) return;
  if (firstPrivacy) {
    log("Confidentialité acceptée ✔ — re-clique sur 📹 Caméra pour l'activer.");
    return;
  }
  camBusy = true;
  try {
    let s: MediaStream;
    try {
      s = await navigator.mediaDevices.getUserMedia({ video: { frameRate: { ideal: 30 } }, audio: false });
    } catch (e) {
      log("Caméra : accès refusé ou indisponible (" + e + ")");
      return;
    }
    S.localCam = s;
    showTile("moi_cam", "Moi (cam)", s, true);
    ensureGroupPcs();
    addStreamToPcs(s);
    $("#btnGroupCam").textContent = "⏹️ Caméra";
    const vt = s.getVideoTracks()[0];
    if (vt) vt.onended = () => stopCam();
    log("📹 Caméra activée.");
  } finally {
    camBusy = false;
  }
}
async function startScreen(): Promise<void> {
  if (screenBusy) return; // sélecteur/démarrage déjà en cours (double-clic)
  if (!loadGroups().some((x) => x.id === S.openGroupId)) {
    log("Ouvre un groupe d'abord.");
    return;
  }
  if (!inThisCall()) {
    log("📞 Rejoins d'abord l'appel de groupe — comme sur Discord, l'écran se partage dans l'appel.");
    return;
  }
  // 🧪 Chemin NATIF (Réglages) : ouvrir le picker (écran OU fenêtre). Pas de
  // getDisplayMedia, pas de WebRTC, pas de confirm() de confidentialité — aucune IP
  // exposée, aucun STUN contacté.
  if (nativeVideoWanted()) {
    const g = loadGroups().find((x) => x.id === S.openGroupId);
    if (g) await openNativePicker(g);
    return;
  }
  // Au TOUT premier usage vidéo, le confirm() de confidentialité consomme le délai
  // d'activation utilisateur (~5 s, Chromium 111+) : getDisplayMedia rejetterait
  // InvalidStateError. On s'arrête proprement et on demande un clic « frais ».
  const firstPrivacy = localStorage.getItem("ghostlink_video_ok") !== "1";
  if (!videoPrivacyOk()) return;
  if (firstPrivacy) {
    log("Confidentialité acceptée ✔ — re-clique sur 🖥️ Écran pour lancer le partage.");
    return;
  }
  screenBusy = true;
  try {
    // VID-5 : capture DÉMARRE à 720p/30 — c'est la base sûre pour l'encodeur
    // LOGICIEL (en 1080p logiciel, plancher screencast VP9 à ~3-5 fps constaté en
    // test réel, bug WebRTC 42223195). VID-6 : une fois l'encodeur MATÉRIEL H.264
    // confirmé par les stats (maybeUpgradeScreen), le track est remonté à 1080p30
    // via applyConstraints. getDisplayMedia n'accepte que des bornes max
    // (min/exact = TypeError) ; Chromium redimensionne en gardant les proportions.
    const vconf = { width: { max: 1280 }, height: { max: 720 }, frameRate: { ideal: 30, max: 30 } };
    screenUpgraded = false;
    screenEncoderChecked = false;
    disarmScreenWatchdog();
    // Son : on demande TOUJOURS l'audio — la case « Partager l'audio » du sélecteur est
    // la SEULE source de vérité (l'ancien confirm() bloquant ici pouvait faire expirer
    // l'activation utilisateur et getDisplayMedia rejetait InvalidStateError sans même
    // ouvrir le sélecteur : « le stream ne se lance pas »). windowAudio est
    // volontairement ABSENT : le sélecteur WebView2 n'implémente pas l'audio de
    // fenêtre (WebView2Feedback#4327), ce hint n'avait aucun effet.
    const wantBrowserAudio = !retryVideoOnly;
    let audioFailed = retryVideoOnly; // l'audio navigateur a déjà échoué au clic précédent
    retryVideoOnly = false;
    const opts: DisplayMediaStreamOptions & {
      systemAudio?: string;
      restrictOwnAudio?: boolean;
    } = wantBrowserAudio
      ? {
          video: vconf,
          audio: true,
          systemAudio: "include", // fait apparaître la case (onglet « Écran entier »)
          restrictOwnAudio: true, // exclut le son émis par la WebView elle-même
        }
      : { video: vconf };
    let s: MediaStream;
    try {
      s = await navigator.mediaDevices.getDisplayMedia(opts);
    } catch (e) {
      const name = e instanceof DOMException ? e.name : String(e);
      if (!wantBrowserAudio || name === "NotAllowedError") {
        // Vrai refus / annulation du sélecteur.
        log("Écran : accès refusé ou annulé (" + name + ")");
        return;
      }
      // Contrairement à la spéc idéale, Chromium/Windows PEUT rejeter TOUT le partage
      // quand la capture du son système échoue (NotReadableError « Could not start
      // audio source », cf. jitsi/jitsi-meet#15417) ou quand l'activation a expiré
      // (InvalidStateError). On retente aussitôt en vidéo seule — le son passera par
      // le repli natif. (C'est le retry que v0.26.1 avait supprimé à tort.)
      audioFailed = true;
      log("Écran+son : échec (" + name + ") — nouvelle tentative sans audio…");
      try {
        s = await navigator.mediaDevices.getDisplayMedia({ video: vconf });
      } catch (e2) {
        const n2 = e2 instanceof DOMException ? e2.name : String(e2);
        if (n2 === "NotAllowedError") {
          // Annulation volontaire du 2e sélecteur : ne rien mémoriser.
          log("Écran : partage annulé.");
          return;
        }
        // L'activation du clic initial est consommée : impossible de réessayer sans
        // nouveau geste. Le prochain clic partira directement sans audio navigateur.
        retryVideoOnly = true;
        log("⚠️ Partage avec son impossible (" + n2 + "). Re-clique sur 🖥️ Écran : la vidéo partira aussitôt et le son système pourra être capté en natif.");
        return;
      }
    }
    // La VIDÉO d'abord : vignette + envoi aux pairs immédiatement. La décision « son »
    // vient APRÈS — plus rien ne doit pouvoir bloquer ou annuler le lancement du flux.
    const svt = s.getVideoTracks()[0];
    if (svt) {
      try {
        // « detail » (pas « motion ») : mode screencast de l'encodeur, cohérent avec
        // maintain-resolution posé par tuneVideoSender. Décidé AVANT addTrack.
        (svt as MediaStreamTrack & { contentHint?: string }).contentHint = "detail";
      } catch {
        /* ignore */
      }
    }
    S.localScreen = s;
    showTile("moi_screen", "Moi (écran)", s, true);
    ensureGroupPcs();
    addStreamToPcs(s);
    $("#btnGroupScreen").textContent = "⏹️ Écran";
    if (svt) svt.onended = () => stopScreen();
    log("🖥️ Partage d'écran lancé.");
    // ---- Décision « son », le flux vidéo étant déjà parti ----
    if (s.getAudioTracks().length) {
      log("🔊 Son système partagé avec l'écran (navigateur).");
      return;
    }
    const surface = svt
      ? (svt.getSettings() as MediaTrackSettings & { displaySurface?: string }).displaySurface
      : undefined;
    if (surface === "monitor" && !audioFailed) {
      // Écran entier : la case « Partager l'audio » était disponible et NON cochée —
      // choix explicite de l'utilisateur, on ne capte rien.
      log("🔇 Sans le son (case « Partager l'audio » non cochée). Pour le son : relance avec la case cochée, ou partage une fenêtre (repli natif proposé).");
      return;
    }
    // Fenêtre partagée (le sélecteur WebView2 n'y propose JAMAIS l'audio — #4327) ou
    // capture audio navigateur en échec : proposer le repli natif. Ce confirm() arrive
    // APRÈS le lancement du partage — plus d'activation utilisateur à préserver.
    const wantNative = confirm(
      "Partager aussi le SON ?\n\nLe navigateur ne fournit pas l'audio ici — ghost link peut capter le son système en natif (TOUT le son du PC, pas seulement la fenêtre partagée). Le flux chiffré part vers les membres du groupe en ligne ; seuls ceux qui ont rejoint l'appel l'entendent.\n\nOK = capter le son système · Annuler = vidéo seule (pour ajouter le son ensuite : arrête ⏹️ puis relance le partage)",
    );
    if (!wantNative) return;
    // Garde anti-course : le partage a pu être arrêté PENDANT le confirm (bouton stop
    // de la barre Chromium → onended en file d'attente). Ne rien capter dans ce cas.
    if (S.localScreen !== s) return;
    const g = loadGroups().find((x) => x.id === S.openGroupId);
    try {
      // Partage WebRTC (fenêtre sans audio navigateur) → son SYSTÈME complet (pid null).
      await invoke("screen_audio_start", { members: g ? g.members : [], pid: null });
      // Le partage a pu être arrêté PENDANT l'await (jusqu'à 5 s côté Rust) : stopScreen
      // a alors vu screenAudioNative=false et n'a rien arrêté — compenser ici, sinon la
      // capture du son système continuerait ORPHELINE (fuite de confidentialité).
      if (S.localScreen !== s) {
        invoke("screen_audio_stop").catch(() => {});
        return;
      }
      screenAudioNative = true;
      // Indicateur permanent tant que le son système est capté en natif.
      $("#btnGroupScreen").textContent = "⏹️ Écran · 🔴 son système";
      // Anti-écho À LA SOURCE : la capture exclut le processus de ghost link
      // (process-loopback EXCLUDE), donc les voix de l'appel qu'on joue ne sont jamais
      // réinjectées — les autres n'entendent QUE le son de l'appli partagée.
      log("🔊 Son système capté en natif — les voix de l'appel sont exclues du flux (pas d'écho).");
    } catch (e) {
      log("🔇 Repli natif indisponible (" + e + ").");
    }
  } finally {
    screenBusy = false;
  }
}
function stopCam(): void {
  if (S.localCam) {
    removeStreamFromPcs(S.localCam);
    S.localCam.getTracks().forEach((t) => t.stop());
    S.localCam = null;
  }
  dropTile("moi_cam");
  $("#btnGroupCam").textContent = "📹 Caméra";
}
function stopScreen(): void {
  // Invalide toute init de partage natif encore en vol (voir nativeShareEpoch) —
  // y compris quand S.localScreenNative n'est pas encore posé (raccrochage pendant
  // l'init : c'est exactement la fenêtre où l'état fantôme naissait).
  nativeShareEpoch += 1;
  if (S.localScreenNative) {
    S.localScreenNative = false;
    invoke("video_share_stop").catch(() => {});
    // Signal d'arrêt aux DESTINATAIRES du partage (mémorisés au démarrage) — pas au
    // groupe actuellement ouvert, qui a pu changer entre-temps.
    (nativeShareMembers || []).forEach((m) => {
      if (S.meshOnline.has(m)) sigSend(m, { nativeVideo: { start: false, gid: nativeShareGidLocal || "" } });
    });
    nativeShareMembers = null;
    nativeShareGidLocal = null;
    nativeShareName = "";
    nativeSharePid = null;
    dropTile("moi" + NATIVE_KEY);
  }
  screenUpgraded = false; // le prochain partage repart en 720p jusqu'à confirmation
  screenEncoderChecked = false;
  disarmScreenWatchdog();
  if (screenAudioNative) {
    screenAudioNative = false;
    invoke("screen_audio_stop").catch(() => {});
  }
  if (S.localScreen) {
    removeStreamFromPcs(S.localScreen);
    S.localScreen.getTracks().forEach((t) => t.stop());
    S.localScreen = null;
  }
  dropTile("moi_screen");
  $("#btnGroupScreen").textContent = "🖥️ Écran";
}
function stopVideo(): void {
  stopCam();
  stopScreen();
  Object.keys(S.pcs).forEach((peer) => {
    try {
      S.pcs[peer].pc.close();
    } catch {
      /* ignore */
    }
    dropPeerTiles(peer);
    delete S.pcs[peer];
  });
  Object.keys(nativeRx).forEach(closeNativeRx);
  vgrid().classList.add("hidden");
}

// ----- Vidéo NATIVE (partage d'écran sans WebRTC : video.rs) -----
// Émission : Rust capture l'écran + encode en H.264 matériel et écrit un flux QUIC
// par pair (aucun MediaStream local, donc pas d'aperçu). Réception : les images
// arrivent par UN canal binaire Tauri (video_receive_attach) sous la forme
// [u8 peer_len][peer][u8 flags bit0=key][u64 frame_id][H.264 Annex-B], décodées par
// WebCodecs et dessinées dans une vignette à canvas. Activation : Réglages → 🧪.
const NATIVE_KEY = "_natscr"; // suffixe de clé de vignette (préfixé par le code du pair)
interface NativeRx {
  dec: VideoDecoder | null;
  waitKey: boolean; // après une perte/erreur : ne reprendre que sur une image clé
  fps: number;
  w: number;
  h: number;
  errors: number; // erreurs de décodage consécutives (remis à 0 à chaque image sortie)
  lastFrameAt: number; // horodatage de la dernière trame reçue (garde anti rx-end périmé)
  canvas: HTMLCanvasElement;
}
const nativeRx: Record<string, NativeRx> = {};
const utf8 = new TextDecoder();
// Pierre tombale anti-résurrection : après un arrêt, des images encore en vol
// recréeraient la vignette (figée pour toujours). On les ignore quelques secondes —
// sauf si la trame porte le bit « nouvelle session » (un partage relancé, légitime).
const nativeTomb: Record<string, number> = {};
// Flux déclaré INDÉCODABLE sur ce moteur (15 resyncs sans une image) : on arrête
// d'essayer jusqu'à une VRAIE relance (bit newSession ou signal start) — sinon la
// pierre tombale expirerait et le cycle vignette noire → fermeture repartirait.
const nativeBroken = new Set<string>();
// Item 5 : gid du partage natif ANNONCÉ par chaque pair — n'afficher sa vignette que si
// ce gid == le groupe de MON appel courant (un partage destiné à A ne fuit pas dans B).
const nativeShareGid: Record<string, string> = {};
// Membres et groupe du partage natif ÉMIS en cours : le signal d'arrêt doit aller
// aux destinataires du partage, pas aux membres du groupe actuellement OUVERT.
let nativeShareMembers: string[] | null = null;
// gid du groupe dont le partage natif est ÉMIS en cours (posé au démarrage, remis à
// null à l'arrêt) : porté par le signal d'arrêt pour que le récepteur sache quel
// partage cesser, même si le groupe actuellement OUVERT a changé entre-temps.
let nativeShareGidLocal: string | null = null;
// Nom de l'écran RÉELLEMENT capturé (repli inclus) — affiché dans la vignette d'état.
let nativeShareName = "";
// Époque du partage natif émis : incrémentée par TOUT arrêt (stopScreen, erreur
// encodeur). startScreenNative la snapshote avant l'invoke et n'engage l'état
// « partage actif » que si rien ne l'a interrompu PENDANT l'init.
let nativeShareEpoch = 0;

/// La vidéo native d'un pair n'est décodée/affichée QUE si on est dans l'appel du
/// groupe dont il est membre — même règle que l'émission (« comme Discord ») ; un
/// ami hors de ce cadre ne peut pas imposer une vignette ni brûler du CPU décodeur.
function nativePeerAllowed(peer: string): boolean {
  if (!S.inGroupCall || !S.groupCallId) return false;
  const g = loadGroups().find((x) => x.id === S.groupCallId);
  return !!g && g.members.includes(peer);
}

/// Codec WebCodecs selon la résolution annoncée (H.264 Main ; niveau ≥ résolution).
function nativeCodecOf(w: number, h: number): string {
  const px = w * h;
  if (px > 0 && px <= 1280 * 720) return "avc1.4d401f"; // Main 3.1
  if (px > 0 && px <= 1920 * 1080) return "avc1.4d4028"; // Main 4.0
  return "avc1.4d4033"; // Main 5.1 — couvre 1440p+ et les tailles inconnues
}

function ensureNativeRx(peer: string, w: number, h: number, fps: number): NativeRx {
  let st = nativeRx[peer];
  if (!st) {
    const canvas = showCanvasTile(peer + NATIVE_KEY, memberName(peer) + " (écran)", peer);
    st = { dec: null, waitKey: true, fps: fps || 30, w, h, errors: 0, lastFrameAt: 0, canvas };
    nativeRx[peer] = st;
  } else {
    // Filet : si la vignette a été retirée du DOM par un autre chemin, la recréer —
    // sinon on décoderait pour toujours dans un canvas détaché (partage invisible).
    if (!st.canvas.isConnected) {
      st.canvas = showCanvasTile(peer + NATIVE_KEY, memberName(peer) + " (écran)", peer);
    }
    if (w && h) {
      st.w = w;
      st.h = h;
      st.fps = fps || st.fps;
    }
  }
  return st;
}

function resetNativeDecoder(st: NativeRx): void {
  if (st.dec && st.dec.state !== "closed") {
    try {
      st.dec.close();
    } catch {
      /* ignore */
    }
  }
  st.dec = null;
  st.waitKey = true;
}

function closeNativeRx(peer: string): void {
  const st = nativeRx[peer];
  if (!st) return;
  resetNativeDecoder(st);
  delete nativeRx[peer];
  nativeTomb[peer] = Date.now();
  dropTile(peer + NATIVE_KEY);
}

/// Une image reçue sur le canal binaire. Le bench exp3 a montré que les payloads
/// arrivent en Array de nombres (pas d'ArrayBuffer) : on normalise en Uint8Array.
function handleNativeFrame(raw: unknown): void {
  let b: Uint8Array;
  if (raw instanceof ArrayBuffer) b = new Uint8Array(raw);
  else if (ArrayBuffer.isView(raw)) b = new Uint8Array(raw.buffer, raw.byteOffset, raw.byteLength);
  else if (Array.isArray(raw)) b = Uint8Array.from(raw as number[]);
  else return;
  if (b.length < 11) return;
  const pl = b[0];
  if (b.length <= 10 + pl) return;
  const peer = utf8.decode(b.subarray(1, 1 + pl));
  const key = (b[1 + pl] & 1) === 1;
  const newSession = (b[1 + pl] & 2) === 2; // 1re trame d'un (re)démarrage du partage
  let id = 0; // u64 BE lu en Number : 2^53 trames = des millénaires à 30 fps
  for (let i = 2 + pl; i < 10 + pl; i++) id = id * 256 + b[i];
  const data = b.subarray(10 + pl);
  if (typeof VideoDecoder === "undefined") return; // moteur sans WebCodecs
  // Item 5 : une trame ne s'affiche que si on connaît le gid du partage de ce pair ET
  // que ce gid est celui de mon appel courant. Trame arrivée avant le signal → ignorée
  // (le signal start, envoyé au démarrage, arrive normalement en tête ; sinon on attend).
  if (!nativePeerAllowed(peer)) return;
  if (nativeShareGid[peer] !== S.groupCallId) return;
  // Images en vol après un arrêt : ne pas ressusciter la vignette (sauf vraie relance).
  if (newSession) {
    delete nativeTomb[peer];
    nativeBroken.delete(peer);
  } else {
    if (nativeBroken.has(peer)) return; // flux déclaré indécodable : attendre une relance
    if (nativeTomb[peer] && Date.now() - nativeTomb[peer] < 3000) return;
  }
  // Vignette/état créés au besoin : les images peuvent devancer le signal de début.
  const st = ensureNativeRx(peer, 0, 0, 30);
  st.lastFrameAt = Date.now();
  if (newSession) resetNativeDecoder(st); // nouveau flux = nouveaux timestamps/SPS
  if (st.waitKey && !key) return;
  if (!st.dec || st.dec.state === "closed") {
    if (!key) {
      st.waitKey = true;
      return;
    }
    const decoder = new VideoDecoder({
      output: (frame: VideoFrame) => {
        st.errors = 0;
        const c = st.canvas;
        if (c.width !== frame.displayWidth || c.height !== frame.displayHeight) {
          c.width = frame.displayWidth;
          c.height = frame.displayHeight;
        }
        const ctx = c.getContext("2d");
        if (ctx) ctx.drawImage(frame, 0, 0);
        frame.close();
      },
      // Erreur de décodage (référence manquante, reconfiguration…) : on repart
      // proprement sur la prochaine image clé au lieu d'afficher de la bouillie.
      error: () => noteNativeDecodeError(peer, st),
    });
    try {
      decoder.configure({ codec: nativeCodecOf(st.w, st.h), optimizeForLatency: true });
    } catch {
      return; // codec refusé : on retentera à la prochaine clé
    }
    st.dec = decoder;
  }
  st.waitKey = false;
  try {
    st.dec.decode(
      new EncodedVideoChunk({
        type: key ? "key" : "delta",
        timestamp: Math.round((id * 1_000_000) / (st.fps || 30)),
        data,
      }),
    );
  } catch {
    noteNativeDecodeError(peer, st);
  }
}

/// Erreur de décodage : visible (une fois par rafale), et si ça ne se remet JAMAIS
/// (15 resyncs consécutifs sans une seule image sortie), on ferme au lieu de boucler
/// en silence sur une vignette noire.
function noteNativeDecodeError(peer: string, st: NativeRx): void {
  st.errors += 1;
  if (st.errors === 1) {
    log("⚠️ Décodage du partage de " + memberName(peer) + " en difficulté — resynchronisation…");
  }
  if (st.errors >= 15) {
    log("⚠️ Partage de " + memberName(peer) + " indécodable sur ce moteur — vignette fermée.");
    nativeBroken.add(peer); // ne plus réessayer avant une VRAIE relance (newSession/start)
    closeNativeRx(peer);
    return;
  }
  resetNativeDecoder(st);
}

/// Signal de contrôle reçu ({nativeVideo:{start,w,h,fps}} via GKIND_SIGNAL).
/// Le reset du décodeur n'est PAS fait ici : c'est le bit « nouvelle session » porté
/// par la première trame du flux qui s'en charge (aucune course signal/trames).
function handleNativeSignal(peer: string, nv: { start?: boolean; w?: number; h?: number; fps?: number; gid?: string }): void {
  if (nv.start) {
    // Item 5 : n'accepter que si le partage vise le groupe de mon appel courant.
    if (!nv.gid || !S.inGroupCall || S.groupCallId !== nv.gid) {
      if (nv.gid) log("🖥️ " + memberName(peer) + " partage son écran dans un autre groupe — rejoins CET appel pour le voir.");
      return;
    }
    // SÉCURITÉ (#47) : n'accepter la vignette QUE si `peer` est réellement membre du groupe
    // de l'appel — même prédicat que le filtrage des TRAMES (nativePeerAllowed). Un ami hors
    // du groupe qui connaît le gid ne peut ainsi pas imposer une vignette « X (écran) ».
    if (!nativePeerAllowed(peer)) return;
    nativeShareGid[peer] = nv.gid;
    delete nativeTomb[peer];
    nativeBroken.delete(peer);
    ensureNativeRx(peer, nv.w || 0, nv.h || 0, nv.fps || 30);
    log("🖥️ " + memberName(peer) + " partage son écran (natif" + (nv.w ? " " + nv.w + "×" + nv.h : "") + ").");
  } else {
    delete nativeShareGid[peer];
    closeNativeRx(peer);
  }
}

/// Abonne cette page au flux vidéo natif entrant (une fois, au chargement).
function initNativeVideoRx(): void {
  try {
    const ch = new window.__TAURI__.core.Channel();
    ch.onmessage = handleNativeFrame;
    invoke("video_receive_attach", { channel: ch }).catch((e) => {
      log("⚠️ Réception vidéo native indisponible (" + e + ") — les partages natifs des autres ne s'afficheront pas.");
    });
  } catch {
    log("⚠️ Réception vidéo native indisponible sur ce moteur (pas de canal binaire).");
  }
}

/// Vignette locale du partage natif : un panneau d'ÉTAT (pas d'aperçu — les images
/// encodées ne repassent pas par la WebView, c'est le prix du zéro-copie local).
/// Redessinée chaque seconde par ghost-video-stats (fps, débit, pairs, qualité).
function showNativePlaceholder(w: number, h: number): void {
  drawNativeStats(null, w, h);
}
let lastNativeLevel = 0;
function drawNativeStats(
  s: { fps: number; kbps: number; peers: number; peersOk: number; level: number; pct: number; dyn: boolean } | null,
  w: number,
  h: number,
): void {
  const c = showCanvasTile("moi" + NATIVE_KEY, "Moi (écran · natif)");
  c.width = 320;
  c.height = 240;
  const ctx = c.getContext("2d");
  if (!ctx) return;
  ctx.fillStyle = "#0b0b10";
  ctx.fillRect(0, 0, c.width, c.height);
  ctx.textAlign = "center";
  ctx.fillStyle = "#c8c8d4";
  ctx.font = "bold 13px sans-serif";
  ctx.fillText("🖥️ " + (nativeShareName || "Écran") + " · " + w + "×" + h, c.width / 2, 70);
  ctx.fillStyle = "#8b8b9a";
  ctx.font = "12px sans-serif";
  ctx.fillText("(natif, sans aperçu local)", c.width / 2, 90);
  if (!s) return;
  ctx.font = "bold 15px sans-serif";
  ctx.fillStyle = s.peersOk === s.peers ? "#7fd77f" : "#e6c86e";
  ctx.fillText(s.fps + " img/s · " + (s.kbps / 1000).toFixed(1) + " Mb/s", c.width / 2, 138);
  ctx.font = "13px sans-serif";
  ctx.fillStyle = "#8b8b9a";
  ctx.fillText("pairs servis : " + s.peersOk + "/" + s.peers, c.width / 2, 162);
  // Sans reconfiguration réelle du débit (dyn=false), le levier est le fps seul :
  // ne pas afficher un « % du débit » trompeur.
  if (s.level > 0) {
    ctx.fillStyle = "#e6c86e";
    ctx.fillText(s.dyn ? "qualité adaptée : " + s.pct + " %" : "fps réduit (réseau chargé)", c.width / 2, 184);
  } else {
    ctx.fillStyle = "#7fd77f";
    ctx.fillText("qualité maximale", c.width / 2, 184);
  }
}

// Cible de partage natif choisie dans le picker : un écran (szDevice) ou une fenêtre
// (HWND + PID pour capter son son). PID du partage courant (fenêtre) mémorisé pour
// l'arrêt / la ré-affirmation audio.
interface ShareTarget {
  kind: "screen" | "window";
  id: string;
  name: string;
  pid?: number;
}
let nativeSharePid: number | null = null;

// Item 3 (fps-only) : plafond de cadence choisi par l'utilisateur (l'adaptatif peut descendre en
// dessous si le réseau sature). La résolution reste native (downscale reportée).
type FpsPreset = { key: string; label: string; fps: number };
const FPS_PRESETS: FpsPreset[] = [
  { key: "60", label: "60 fps (max)", fps: 60 },
  { key: "30", label: "30 fps (moins de bande passante)", fps: 30 },
];
function currentFps(): FpsPreset {
  const k = localStorage.getItem("ghostlink_stream_quality") || "60";
  return FPS_PRESETS.find((q) => q.key === k) || FPS_PRESETS[0];
}

// ----- Picker de partage natif (écran OU fenêtre), au clic sur 🖥️ -----
function closeNativePicker(): void {
  $("#nativePickerWrap").classList.add("hidden");
}
function pickerItem(label: string, onClick: () => void): HTMLButtonElement {
  const b = document.createElement("button");
  b.type = "button";
  b.className = "item";
  b.style.cssText = "width:100%;text-align:left;cursor:pointer;border:none;background:transparent;color:var(--text);padding:8px 10px;font-size:13.5px";
  b.textContent = label;
  b.onclick = onClick;
  return b;
}
async function openNativePicker(g: Group): Promise<void> {
  const screensBox = $("#nativePickerScreens");
  const winsBox = $("#nativePickerWindows");
  screensBox.replaceChildren();
  winsBox.replaceChildren();
  const qSel = $<HTMLSelectElement>("#nativePickerQuality");
  qSel.replaceChildren();
  FPS_PRESETS.forEach((q) => {
    const o = document.createElement("option");
    o.value = q.key; o.textContent = q.label;
    if (q.key === currentFps().key) o.selected = true;
    qSel.appendChild(o);
  });
  qSel.onchange = () => localStorage.setItem("ghostlink_stream_quality", qSel.value);
  const start = (t: ShareTarget) => {
    closeNativePicker();
    void startScreenNative(g, t);
  };
  let mons: { id: string; name: string; w: number; h: number; primary: boolean }[] = [];
  try {
    mons = await invoke("video_list_monitors");
  } catch {
    /* liste vide → repli « Écran principal » ci-dessous */
  }
  if (mons.length) {
    mons.forEach((m) =>
      screensBox.appendChild(
        pickerItem("🖵 " + m.name + " — " + m.w + "×" + m.h + (m.primary ? " (principal)" : ""), () =>
          start({ kind: "screen", id: m.id, name: m.name }),
        ),
      ),
    );
  } else {
    screensBox.appendChild(pickerItem("🖵 Écran principal", () => start({ kind: "screen", id: "", name: "Écran principal" })));
  }
  let wins: { id: string; name: string; pid: number }[] = [];
  try {
    wins = await invoke("video_list_windows");
  } catch {
    /* liste vide */
  }
  if (wins.length) {
    wins.forEach((wd) =>
      winsBox.appendChild(
        pickerItem("🪟 " + wd.name, () => start({ kind: "window", id: wd.id, name: wd.name, pid: wd.pid })),
      ),
    );
  } else {
    const s = document.createElement("span");
    s.className = "hint";
    s.textContent = "Aucune fenêtre partageable détectée.";
    winsBox.appendChild(s);
  }
  $("#nativePickerWrap").classList.remove("hidden");
}

async function startScreenNative(g: Group, target: ShareTarget): Promise<void> {
  screenBusy = true;
  try {
    const epoch0 = nativeShareEpoch;
    const isWindow = target.kind === "window";
    // Écran = szDevice STABLE (Rust replie sur le principal si absent, monitorFound) ;
    // Fenêtre = HWND.
    const args = isWindow
      ? { members: g.members, monitor: null, window: target.id, maxFps: currentFps().fps }
      : { members: g.members, monitor: target.id || null, window: null, maxFps: currentFps().fps };
    let info: { w: number; h: number; fps: number; monitor: string; monitorFound: boolean };
    try {
      info = await invoke("video_share_start", args);
    } catch (e) {
      log("🧪 Partage natif impossible : " + e + " — décoche « Partage d'écran natif » dans Réglages pour repasser en WebRTC.");
      return;
    }
    // L'init a pu durer plusieurs secondes : si l'appel s'est terminé PENDANT (même
    // s'il a été REJOINT depuis), ou si l'encodeur est déjà mort (ghost-video-ended
    // incrémente l'époque), ne pas afficher un état « partage actif » fantôme.
    if (!inThisCall() || nativeShareEpoch !== epoch0) {
      invoke("video_share_stop").catch(() => {});
      log("Partage natif annulé — interrompu pendant le démarrage.");
      return;
    }
    S.localScreenNative = true;
    lastNativeLevel = 0; // chaque partage repart à qualité max
    nativeShareName = info.monitor; // nom réel de l'écran/fenêtre capturé (vignette + logs)
    nativeShareMembers = g.members.slice(); // destinataires du signal d'arrêt
    nativeShareGidLocal = g.id; // gid du partage émis, porté par le signal d'arrêt
    nativeSharePid = isWindow ? target.pid ?? null : null; // pour le son de la fenêtre
    // Annonce aux membres en ligne. Limite v1 : un membre qui arrive APRÈS le
    // démarrage ne reçoit pas ce partage (relancer ⏹️/🖥️ pour l'inclure).
    g.members.forEach((m) => {
      if (S.meshOnline.has(m)) sigSend(m, { nativeVideo: { start: true, w: info.w, h: info.h, fps: info.fps, gid: g.id } });
    });
    showNativePlaceholder(info.w, info.h);
    $("#btnGroupScreen").textContent = "⏹️ Écran";
    log("🖥️ Partage NATIF lancé — " + info.monitor + " (" + info.w + "×" + info.h + "@" + info.fps + ", H.264 matériel, sans WebRTC/STUN).");
    // Écran demandé introuvable (débranché) : Rust a replié sur le principal — le dire
    // fort, c'est peut-être un écran privé diffusé.
    if (!isWindow && target.id && !info.monitorFound) {
      log("⚠️ L'écran choisi est introuvable — c'est " + info.monitor + " (principal) qui est partagé.");
    }
    // Son : pas d'audio navigateur ici — proposer le repli natif. Pour une FENÊTRE, on
    // ne capte que le son de SON process (pid) ; pour un écran, tout le son système.
    const wantNative = confirm(
      isWindow
        ? "Partager aussi le SON de cette fenêtre ?\n\nghost link peut capter le son de CETTE application seulement. Le flux chiffré part vers les membres en ligne ; seuls ceux dans l'appel l'entendent.\n\nOK = capter le son de la fenêtre · Annuler = vidéo seule"
        : "Partager aussi le SON ?\n\nghost link peut capter le son système en natif (TOUT le son du PC). Le flux chiffré part vers les membres en ligne ; seuls ceux dans l'appel l'entendent.\n\nOK = capter le son système · Annuler = vidéo seule",
    );
    if (!wantNative || !S.localScreenNative) return;
    try {
      await invoke("screen_audio_start", { members: g.members, pid: nativeSharePid });
      if (!S.localScreenNative) {
        // Partage arrêté PENDANT l'await : ne pas laisser la capture orpheline.
        invoke("screen_audio_stop").catch(() => {});
        return;
      }
      screenAudioNative = true;
      $("#btnGroupScreen").textContent = isWindow ? "⏹️ Écran · 🔴 son fenêtre" : "⏹️ Écran · 🔴 son système";
      log(isWindow
        ? "🔊 Son de la fenêtre capté en natif (seulement cette appli)."
        : "🔊 Son système capté en natif — les voix de l'appel sont exclues du flux (pas d'écho).");
    } catch (e) {
      log("🔇 Son natif indisponible (" + e + ").");
    }
  } finally {
    screenBusy = false;
  }
}

export function initGroups(): void {
  initNativeVideoRx();
  // L'émetteur natif s'est arrêté sur une ERREUR (encodeur, GPU…) — pas via stop().
  // Pas de garde sur S.localScreenNative : une erreur immédiate peut arriver AVANT
  // que le drapeau soit posé — le message doit sortir dans tous les cas.
  listen("ghost-video-ended", (e) => {
    const p = e.payload || {};
    nativeShareEpoch += 1; // invalide aussi une init encore en vol (état fantôme)
    log("⚠️ Partage natif interrompu : " + (p.reason || "erreur d'encodage"));
    if (S.localScreenNative) stopScreen();
  });
  // Le flux vidéo natif d'un pair s'est terminé (arrêt, erreur, connexion perdue) :
  // fermer sa vignette au lieu de la laisser figée en ayant l'air vivante.
  listen("ghost-video-rx-end", (e) => {
    const peer = e.payload;
    if (!peer || !nativeRx[peer]) return;
    const bye = () => {
      closeNativeRx(peer);
      log("🖥️ Partage de " + memberName(peer) + " terminé.");
    };
    // rx-end ne porte pas d'identité de flux : la fin RETARDÉE d'un ancien flux ne
    // doit pas fermer une session relancée qui livre activement des trames. Si des
    // trames sont arrivées très récemment, on re-vérifie dans 2 s : toujours du flux
    // → c'était la fin de l'ANCIEN flux, on garde ; plus rien → vraie fin, on ferme.
    if (Date.now() - nativeRx[peer].lastFrameAt < 1500) {
      setTimeout(() => {
        const st = nativeRx[peer];
        if (st && Date.now() - st.lastFrameAt >= 1800) bye();
      }, 2000);
      return;
    }
    bye();
  });
  // Côté émetteur : un pair ne reçoit plus le partage (file morte, connexion tombée).
  listen("ghost-video-peer-dead", (e) => {
    if (e.payload && S.localScreenNative) {
      log("⚠️ " + memberName(e.payload) + " ne reçoit plus le partage d'écran (connexion interrompue).");
    }
  });
  // Stats de l'émetteur natif (1 Hz) : redessiner la vignette d'état + tracer les
  // changements de niveau du contrôleur adaptatif dans le journal.
  listen("ghost-video-stats", (e) => {
    if (!S.localScreenNative) return;
    const s = e.payload;
    drawNativeStats(s, s.w, s.h);
    if (s.level !== lastNativeLevel) {
      // Le % de débit n'a de sens que si l'encodeur est vraiment reconfigurable
      // (dyn) ; sinon c'est le fps seul qui bouge — le dire dans les DEUX sens.
      const desc = s.dyn ? s.pct + " % du débit" : "fps réduit (encodeur non reconfigurable)";
      const descUp = s.dyn ? "remonté à " + s.pct + " % du débit" : "fps rétabli";
      log(s.level > lastNativeLevel
        ? "📉 Réseau chargé — partage adapté : " + desc + "."
        : "📈 Réseau rétabli — " + descUp + ".");
      lastNativeLevel = s.level;
    }
  });
  $("#btnCreateGroup").onclick = () => {
    const name = $<HTMLInputElement>("#groupName").value.trim();
    if (!name) {
      log("Donne un nom au groupe.");
      return;
    }
    const selected = Array.from($("#groupFriends").querySelectorAll("input:checked"))
      .map((c: any) => c.value)
      .filter(Boolean);
    if (!selected.length) {
      log("Sélectionne au moins un ami.");
      return;
    }
    if (!S.myCode) {
      log("Code permanent indisponible — ré-essaie dans une seconde.");
      return;
    }
    const id = "g" + Date.now().toString(36) + Math.random().toString(36).slice(2, 7);
    const full = [S.myCode, ...selected];
    saveGroups([...loadGroups(), { id, name, members: selected }]);
    $<HTMLInputElement>("#groupName").value = "";
    renderGroups();
    const fullCsv = full.join(",");
    selected.forEach((code: string) => {
      addPInv(code, id, name, fullCsv);
      invoke("send_ginvite", { member: code, gid: id, name, members: fullCsv }).catch(() => {});
    });
    log("Groupe « " + name + " » créé — invitations envoyées (et mises en attente pour les membres hors ligne).");
    openGroup(id, true); // skipDial : send_ginvite a déjà connecté les membres en ligne
  };
  $("#btnCloseGroup").onclick = closeGroup;
  $("#btnNativePickerCancel").onclick = closeNativePicker;
  $("#btnAddMembers").onclick = () => {
    const g = loadGroups().find((x) => x.id === S.openGroupId);
    if (!g) return;
    const panel = $("#addMembersPanel");
    if (panel.classList.contains("hidden")) renderAddMembersFriends(g);
    panel.classList.toggle("hidden");
  };
  $("#btnAddMembersCancel").onclick = () => $("#addMembersPanel").classList.add("hidden");
  $("#btnAddMembersDo").onclick = () => {
    const g = loadGroups().find((x) => x.id === S.openGroupId);
    if (!g) return;
    const sel = Array.from($("#addMembersFriends").querySelectorAll("input:checked"))
      .map((c: any) => c.value)
      .filter(Boolean);
    if (!sel.length) {
      log("Sélectionne au moins un ami à ajouter.");
      return;
    }
    addMembersToGroup(g, sel);
    $("#addMembersPanel").classList.add("hidden");
  };
  $("#btnGroupSend").onclick = sendGroupMsg;
  $<HTMLInputElement>("#groupChatInput").onkeydown = (e: KeyboardEvent) => {
    if (e.key === "Enter") {
      e.preventDefault();
      sendGroupMsg();
    }
  };
  $("#btnGroupChatImg").onclick = pickAndSendImageGroup;
  $("#groupChatInput").addEventListener("paste", (e: Event) => {
    const it = (e as ClipboardEvent).clipboardData?.items;
    if (!it) return;
    for (const x of it) {
      if (x.type.startsWith("image/")) {
        const f = x.getAsFile();
        if (f) void sendImageGroup(f);
      }
    }
  });
  $<HTMLButtonElement>("#btnGroupCall").onclick = () => {
    const g = loadGroups().find((x) => x.id === S.openGroupId);
    if (!g) return;
    if (S.inGroupCall && S.groupCallId === g.id) {
      stopGroupCall();
      return;
    }
    // En appel dans un AUTRE groupe : raccrocher d'abord (group_call_stop coupe aussi les
    // partages orphelins et envoie le beacon final inCall:false) avant de démarrer le
    // nouvel appel — sinon un partage natif/son système continuerait d'émettre vers l'ancien.
    if (S.inGroupCall && S.groupCallId !== g.id) stopGroupCall();
    startGroupCall(g, true);
  };
  $("#btnGroupMute").onclick = () => {
    S.groupMuted = !S.groupMuted;
    invoke("group_call_mute", { on: S.groupMuted }).catch(() => {});
    refreshGroupCallUI();
  };
  $("#btnJoinGCall").onclick = () => {
    $("#gcallBanner").classList.add("hidden");
    const g = loadGroups().find((x) => x.id === S.pendingGCall);
    S.pendingGCall = null;
    if (!g || S.inGroupCall) return;
    openGroup(g.id);
    startGroupCall(g, false);
  };
  $("#btnDeclineGCall").onclick = () => {
    $("#gcallBanner").classList.add("hidden");
    S.pendingGCall = null;
  };
  $("#btnJoinGroup").onclick = () => {
    $("#groupInviteBanner").classList.add("hidden");
    if (!S.pendingInvite) return;
    const members = S.pendingInvite.full.filter((c) => c && c !== S.myCode);
    if (!loadGroups().some((x) => x.id === S.pendingInvite!.id)) {
      saveGroups([...loadGroups(), { id: S.pendingInvite.id, name: S.pendingInvite.name, members }]);
    }
    invoke("open_group", { members: friendsOnly(members) }).catch(() => {});
    renderGroups();
    log("Groupe « " + S.pendingInvite.name + " » rejoint.");
    S.pendingInvite = null;
  };
  $("#btnDeclineGroup").onclick = () => {
    if (S.pendingInvite) addDeclined(S.pendingInvite.id);
    $("#groupInviteBanner").classList.add("hidden");
    S.pendingInvite = null;
  };
  $("#btnGroupCam").onclick = () => {
    if (S.localCam) stopCam();
    else startCam();
  };
  $("#btnGroupScreen").onclick = () => {
    if (S.localScreen || S.localScreenNative) stopScreen();
    else startScreen();
  };
  $("#btnGroupFile").onclick = () => {
    const g = loadGroups().find((x) => x.id === S.openGroupId);
    if (!g) return;
    const path = $<HTMLInputElement>("#groupFilePath").value.trim();
    if (!path) {
      log("Colle le chemin d'un fichier à envoyer.");
      return;
    }
    invoke("send_gfile", { members: g.members, path })
      .then(() => {
        log("📎 Fichier envoyé au groupe.");
        $<HTMLInputElement>("#groupFilePath").value = "";
      })
      .catch((e) => log("Fichier groupe : " + e));
  };

  // Réessai doux des invitations encore en attente (membres hors ligne à la création).
  setInterval(() => {
    const p = loadPInv();
    if (!p.length) return;
    p.forEach((x) =>
      invoke("send_ginvite", { member: x.member, gid: x.gid, name: x.name, members: x.csv }).catch(() => {}),
    );
  }, 60000);

  // Listeners groupes
  listen("ghost-mesh-up", (e) => {
    if (e.payload) {
      S.meshOnline.add(e.payload);
      flushPInv(e.payload);
      // VID-1 : si je partage déjà ma cam/écran, pousser la vidéo vers un arrivant tardif.
      // Le partage appartient à l'APPEL (S.groupCallId), pas au groupe affiché : on peut
      // naviguer ailleurs pendant l'appel, donc tester le groupe de l'appel — sinon on
      // fuiterait mes pistes vers un membre du groupe OUVERT hors de l'appel, et on ne
      // resservirait pas un membre de l'appel qui revient pendant qu'un autre groupe est ouvert.
      if (S.inGroupCall && (S.localCam || S.localScreen) && !S.pcs[e.payload]) {
        const g = loadGroups().find((x) => x.id === S.groupCallId);
        if (g && g.members.includes(e.payload)) getPc(e.payload);
      }
      // Partage NATIF : pas d'ajout dynamique en v1 — le dire plutôt que laisser
      // l'arrivant devant un écran vide sans explication.
      if (S.localScreenNative && nativeShareMembers && nativeShareMembers.includes(e.payload)) {
        log("ℹ️ " + memberName(e.payload) + " vient de se connecter — relance le partage (⏹️ puis 🖥️) pour l'inclure.");
      }
      // Convergence de roster : à ce pair qui (re)vient en ligne, envoyer (1) MON set de
      // membres — rattrape les ajouts manqués pendant qu'il était hors ligne — et (2) MES
      // votes de kick, via le chemin GKIND_KICK authentifié (voter = S.myCode, inforgeable) :
      // le pair les tallie avec son propre handler ghost-kick et n'applique un retrait qu'au
      // quorum. Aucun tombstone ne circule ; un seul vote forgé ne peut jamais faire quorum.
      const peer = e.payload;
      loadGroups().forEach((g) => {
        if (!g.members.includes(peer)) return;
        broadcastRoster(g, [peer]); // membres → pair qui (re)vient
        myVotes(g.id).forEach((target) => {
          invoke("send_kick", { members: [peer], gid: g.id, target, voter: S.myCode }).catch(() => {});
        });
      });
    }
    refreshGroupCounts();
  });
  listen("ghost-mesh-down", (e) => {
    if (e.payload) {
      S.meshOnline.delete(e.payload);
      closeNativeRx(e.payload); // le flux vidéo natif de ce pair est mort avec la connexion
    }
    refreshGroupCounts();
  });
  listen("ghost-gchat", (e) => {
    const p = e.payload || ({} as { group?: string; author?: string; text?: string });
    if (!loadGroups().some((x) => x.id === p.group)) return;
    pushGroupMsg(p.group as string, p.author || "?", p.text || "", "them");
  });
  listen("ghost-gchat-img", (e) => {
    const p = e.payload || ({} as { group?: string; author?: string; mime?: string; dataB64?: string });
    if (!p.group || !loadGroups().some((x) => x.id === p.group)) return;
    // Contrairement au texte (pushGroupMsg), les images ne sont pas conservées
    // dans S.groupMsgs (pas d'historique par groupe) : on ne rend que si CE
    // groupe est actuellement ouvert, sinon l'image apparaîtrait à tort dans
    // le #groupChatLog d'un autre groupe actif à l'écran.
    if (S.openGroupId !== p.group) return;
    addImgBubble($("#groupChatLog"), `data:${p.mime};base64,${p.dataB64}`, "them", p.author);
  });
  listen("ghost-ginvite", (e) => {
    const p = e.payload || ({} as { id?: string; name?: string; members?: string });
    // BUG-1 : ignorer les ré-envois pour un groupe déjà rejoint ou déjà refusé.
    if (loadGroups().some((x) => x.id === p.id)) return;
    if (p.id && declinedGroups().includes(p.id)) return;
    const full = (p.members || "")
      .split(",")
      .map((s) => s.trim())
      .filter(Boolean);
    S.pendingInvite = { id: p.id as string, name: p.name || "Groupe", full };
    $("#groupInviteText").textContent =
      '👪 Invitation au groupe « ' + (p.name || "?") + " » (" + full.length + " membres).";
    $("#groupInviteBanner").classList.remove("hidden");
  });
  // Sync de roster : union des MEMBRES reçus (jamais de tombstones sur le fil) — puis
  // re-diffusion multi-saut si mon set de membres a réellement changé (convergence des
  // ajouts même sans lien direct avec tous les membres). Les exclusions convergent par un
  // canal distinct (re-gossip de votes authentifiés), pas ici.
  listen("ghost-gmembers", (e) => {
    const p = e.payload || ({} as { group?: string; members?: string; from?: string });
    const groups = loadGroups();
    const g = groups.find((x) => x.id === p.group);
    if (!g) return; // pas (encore) dans ce groupe → les nouveaux reçoivent une invite
    // SÉCURITÉ : n'accepter une sync de roster QUE d'un membre déjà connu (p.from =
    // remote_id authentifié) — sinon n'importe quel ami connaissant le gid pourrait
    // injecter des membres arbitraires dans le roster local des autres.
    if (!p.from || (!g.members.includes(p.from) && p.from !== S.myCode)) return;
    const kset = kickedSet(g.id); // tombstones LOCAUX (jamais reçus du fil) : ne pas ré-admettre un exclu
    const incoming = (p.members || "")
      .split(",")
      .map((s) => s.trim())
      .filter(Boolean);
    const merged = Array.from(new Set([...g.members, ...incoming])).filter(
      (c) => c && c !== S.myCode && !kset.has(c),
    );
    const changed = merged.length !== g.members.length || merged.some((c) => !g.members.includes(c));
    if (!changed) return; // anti-tempête : aucune re-diffusion sur un no-op
    g.members = merged;
    saveGroups(groups);
    renderGroups();
    if (S.openGroupId === g.id) refreshGroupCounts();
    invoke("open_group", { members: friendsOnly(merged) }).catch(() => {});
    broadcastRoster(g); // multi-saut : propager le nouveau set de membres aux en-ligne
    log("👥 Roster du groupe « " + g.name + " » mis à jour.");
  });
  // Activité vocale (~10 Hz) : qui est en appel / qui parle. On bascule des classes
  // CSS sur les chips par data-code — SANS re-render (coûteux à cette fréquence).
  // L'activité est GLOBALE (celle de l'appel où JE suis) : ne l'appliquer que si le
  // groupe ouvert EST celui de mon appel, sinon on allumerait des pastilles sur des
  // chips d'un autre groupe sans appel.
  listen("ghost-voice-activity", (e) => {
    const act = e.payload || {};
    S.voiceAct = act;
    const showHere = S.inGroupCall && S.groupCallId === S.openGroupId;
    document.querySelectorAll(".mem[data-code]").forEach((el) => {
      const code = (el as HTMLElement).dataset.code || "";
      const a = showHere ? act[code] : undefined;
      el.classList.toggle("incall", !!a?.inCall);
      el.classList.toggle("speaking", !!a?.speaking);
    });
  });
  // Présence vocale groupe (Task 4.3) : pastille « dans le vocal » (.inbooth), visible
  // par TOUT le groupe même hors appel — indépendante de ghost-voice-activity ci-dessus.
  // `code` = pair authentifié (remote_id côté Rust, voir net.rs), pas une donnée
  // auto-déclarée dans le payload.
  listen("ghost-voice-presence", (e) => {
    const p = e.payload || ({} as { group?: string; code?: string; inCall?: boolean });
    if (!p.group || !p.code) return;
    const g = (S.voicePresence[p.group] = S.voicePresence[p.group] || {});
    if (p.inCall) g[p.code] = Date.now();
    else delete g[p.code];
    paintVoicePresence();
  });
  // Balayage TTL : une présence sans beacon depuis > 4 s (beacon ~1 Hz) est périmée —
  // couvre un pair qui disparaît sans envoyer son beacon final inCall:false (crash,
  // perte réseau, fermeture brutale de l'appli).
  setInterval(() => {
    const now = Date.now();
    let changed = false;
    for (const gid of Object.keys(S.voicePresence)) {
      for (const code of Object.keys(S.voicePresence[gid])) {
        if (now - S.voicePresence[gid][code] > 4000) {
          delete S.voicePresence[gid][code];
          changed = true;
        }
      }
    }
    if (changed) paintVoicePresence();
  }, 2000);
  // Un vote d'exclusion reçu : tallie et applique au quorum (60 % des en-ligne).
  listen("ghost-kick", (e) => {
    const p = e.payload || ({} as { group?: string; target?: string; voter?: string; from?: string });
    if (!p.target || !p.voter) return;
    // SÉCURITÉ : le vote ne compte que si l'EXPÉDITEUR AUTHENTIFIÉ (p.from = remote_id,
    // inforgeable) est bien le votant déclaré. Sans ça, un seul pair pourrait forger le
    // quorum en envoyant des votes au nom de tous les autres membres.
    if (!p.from || p.from !== p.voter) return;
    const g = loadGroups().find((x) => x.id === p.group);
    if (!g) return;
    // Le votant doit être un membre du groupe (anti-vote d'un tiers).
    if (!g.members.includes(p.voter)) return;
    recordKickVote(g.id, p.target, p.voter);
    tallyKick(g, p.target);
  });
  listen("ghost-gcall", (e) => {
    const p = e.payload || ({} as { group?: string });
    if (S.inGroupCall) return;
    const g = loadGroups().find((x) => x.id === p.group);
    if (!g) return;
    S.pendingGCall = g.id;
    $("#gcallText").textContent = '📞 Appel dans le groupe « ' + g.name + " » — rejoindre ?";
    $("#gcallBanner").classList.remove("hidden");
  });
  listen("ghost-signal", async (e) => {
    const p = e.payload || ({} as { from?: string; data?: string });
    const peer = p.from;
    if (!peer) return;
    let msg: any;
    try {
      msg = JSON.parse(p.data as string);
    } catch {
      return;
    }
    // Vidéo NATIVE : signal de contrôle pur — surtout NE PAS créer de
    // RTCPeerConnection pour ça (getPc ouvrirait une négociation WebRTC).
    if (msg.nativeVideo) {
      handleNativeSignal(peer, msg.nativeVideo);
      return;
    }
    // SÉCURITÉ : n'ouvrir/alimenter une RTCPeerConnection QUE si je suis dans l'appel du
    // groupe dont `peer` est membre (même prédicat que le chemin natif, nativePeerAllowed).
    // Sinon un pair hors de ce cadre forcerait une négociation WebRTC (IP exposée, STUN
    // contacté, mes flux poussés, vignette imposée) sans jamais passer par le confirm de
    // confidentialité, qui ne couvre que l'ÉMISSION.
    if (!nativePeerAllowed(peer)) return;
    // Une candidate ICE tardive (après stopVideo) ne doit pas RESSUSCITER un pc supprimé :
    // n'ouvrir un pc que sur une description ; une candidate sans pc existant est ignorée.
    if (msg.candidate && !S.pcs[peer]) return;
    const st = getPc(peer);
    const pc = st.pc;
    try {
      if (msg.description) {
        const collision = msg.description.type === "offer" && (st.makingOffer || pc.signalingState !== "stable");
        if (collision && !st.polite) return;
        await pc.setRemoteDescription(msg.description);
        if (msg.description.type === "offer") {
          await pc.setLocalDescription();
          sigSend(peer, { description: pc.localDescription });
        }
        if (pc.signalingState === "stable") {
          retuneSenders(pc);
          maybeUpgradeScreen(); // unanimité matérielle → 1080p (VID-6)
        }
      } else if (msg.candidate) {
        try {
          await pc.addIceCandidate(msg.candidate);
        } catch {
          /* ignore */
        }
      }
    } catch (err) {
      log("Vidéo: " + err);
    }
  });
  // Fichiers de groupe (SEC-1 : accord avant enregistrement)
  listen("ghost-grecv-start", (e) => {
    const p = e.payload || ({} as { name?: string; from?: string });
    log("⬇️ Réception (groupe) de « " + (p.name || "") + " » de " + memberName(p.from || "") + "…");
  });
  listen("ghost-grecv-done", (e) => {
    const p = e.payload || ({} as { name?: string });
    log("✅ Reçu (groupe) : " + (p.name || ""));
    // LIMITATION (Task 3.4) : contrairement au 1-à-1 (ghost-recv-done), l'événement
    // groupe ne porte pas de `path` (voir tauri.ts, Events["ghost-grecv-done"] =
    // { name?: string }) — impossible d'appeler read_image_bytes ici sans fabriquer
    // un chemin. Une grosse image (> 5 Mo) envoyée en groupe reste donc une entrée
    // fichier normale, sans rendu inline à la réception. L'inline groupe ≤ 5 Mo
    // (ghost-gchat-img, envoyé via send_gimg) fonctionne normalement.
  });
  listen("ghost-grecv-offer", (e) => {
    const p = e.payload || ({} as { id?: number; name?: string; size?: number; from?: string });
    // Une offre précédente est encore en attente : la refuser explicitement avant que la
    // bannière ne soit écrasée — sinon son id est perdu et l'expéditeur resterait bloqué
    // en attente (aucun respond_gfile ne partirait jamais pour elle).
    if (S.gfileOfferId != null && S.gfileOfferId !== (p.id ?? null))
      invoke("respond_gfile", { id: S.gfileOfferId, accept: false }).catch(() => {});
    S.gfileOfferId = p.id ?? null;
    $("#gfileOfferText").textContent =
      '📥 (groupe) « ' + (p.name || "fichier") + " » (" + fmt(p.size || 0) + ") de " + memberName(p.from || "") + " — accepter ?";
    $("#gfileOfferBanner").classList.remove("hidden");
  });
  $("#btnGfileAccept").onclick = () => {
    if (S.gfileOfferId != null) invoke("respond_gfile", { id: S.gfileOfferId, accept: true }).catch(() => {});
    $("#gfileOfferBanner").classList.add("hidden");
    S.gfileOfferId = null;
  };
  $("#btnGfileReject").onclick = () => {
    if (S.gfileOfferId != null) invoke("respond_gfile", { id: S.gfileOfferId, accept: false }).catch(() => {});
    $("#gfileOfferBanner").classList.add("hidden");
    S.gfileOfferId = null;
    log("Fichier de groupe refusé.");
  };
  listen("ghost-grecv-rejected", (e) => {
    const p = e.payload || ({} as { name?: string });
    log("Fichier de groupe refusé : " + (p.name || ""));
  });
  listen("ghost-grecv-corrupt", (e) => {
    const p = e.payload || ({} as { name?: string });
    log("⚠️ Fichier de groupe corrompu (intégrité invalide) — rejeté : " + (p.name || ""));
  });
}
