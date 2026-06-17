// Groupes : channel multi-pairs (chat), appel de groupe (audio), vidéo (WebRTC), fichiers.
import { invoke, listen } from "./tauri.js";
import { $, log, fmt } from "./dom.js";
import { S, PINV, GDECL, iceConfig, loadGroups, saveGroups, loadFriends, friendsOnly, memberName, myName, } from "./state.js";
import { showTab } from "./session.js";
// ----- Invitations en attente (BUG-1 : fiables, ré-envoyées à la reconnexion) -----
function loadPInv() {
    try {
        return JSON.parse(localStorage.getItem(PINV) || "") || [];
    }
    catch {
        return [];
    }
}
function savePInv(a) {
    localStorage.setItem(PINV, JSON.stringify(a));
}
function addPInv(member, gid, name, csv) {
    const a = loadPInv();
    if (!a.some((x) => x.member === member && x.gid === gid)) {
        a.push({ member, gid, name, csv });
        savePInv(a);
    }
}
function clearPInvGroup(gid) {
    savePInv(loadPInv().filter((x) => x.gid !== gid));
}
function flushPInv(member) {
    const mine = loadPInv().filter((x) => x.member === member);
    if (!mine.length)
        return;
    mine.forEach((x) => invoke("send_ginvite", { member: x.member, gid: x.gid, name: x.name, members: x.csv }).catch(() => { }));
    savePInv(loadPInv().filter((x) => x.member !== member));
}
function declinedGroups() {
    try {
        return JSON.parse(localStorage.getItem(GDECL) || "") || [];
    }
    catch {
        return [];
    }
}
function addDeclined(id) {
    const a = declinedGroups();
    if (!a.includes(id)) {
        a.push(id);
        localStorage.setItem(GDECL, JSON.stringify(a));
    }
}
// ----- Rendu des groupes / membres -----
function updateGroupLine(g) {
    const total = g.members.length + 1;
    const online = 1 + g.members.filter((c) => S.meshOnline.has(c)).length;
    $("#groupMembersLine").textContent = "👥 " + total + " membres · " + online + " en ligne";
}
function renderGroupMembers(g) {
    const box = $("#groupMembers");
    if (!box || !g)
        return;
    box.innerHTML = "";
    const callActive = S.inGroupCall && S.groupCallId === g.id;
    const chip = (code, label, online, self) => {
        const c = document.createElement("span");
        c.style.cssText =
            "display:inline-flex;align-items:center;gap:6px;font-size:12px;padding:3px 9px;border-radius:11px;background:var(--field);border:1px solid var(--gborder)";
        const d = document.createElement("span");
        d.style.cssText =
            "width:7px;height:7px;border-radius:50%;flex:0 0 auto;background:" + (online ? "var(--good)" : "var(--muted)");
        const t = document.createElement("span");
        t.textContent = label;
        c.appendChild(d);
        c.appendChild(t);
        if (callActive && !self) {
            const r = document.createElement("input");
            r.type = "range";
            r.min = "0";
            r.max = "200";
            r.step = "5";
            r.value = String(S.groupGains[code] != null ? S.groupGains[code] : 100);
            r.title = "Volume";
            r.style.cssText = "width:64px;flex:0 0 auto";
            r.oninput = () => {
                S.groupGains[code] = +r.value;
                invoke("group_call_volume", { peer: code, vol: +r.value / 100 }).catch(() => { });
            };
            c.appendChild(r);
        }
        return c;
    };
    box.appendChild(chip("moi", "Moi", true, true));
    g.members.forEach((code) => box.appendChild(chip(code, memberName(code), S.meshOnline.has(code), false)));
}
function refreshGroupCounts() {
    const g = loadGroups().find((x) => x.id === S.openGroupId);
    if (g) {
        updateGroupLine(g);
        renderGroupMembers(g);
    }
    renderGroups();
}
export function renderGroupFriends() {
    const box = $("#groupFriends");
    if (!box)
        return;
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
export function renderGroups() {
    const box = $("#groupList");
    if (!box)
        return;
    box.innerHTML = "";
    const gs = loadGroups();
    if (!gs.length) {
        box.innerHTML = '<span class="hint">Aucun groupe.</span>';
        return;
    }
    gs.forEach((g, i) => {
        const d = document.createElement("div");
        d.className = "xfer";
        const meta = document.createElement("div");
        meta.className = "meta";
        const nm = document.createElement("div");
        nm.className = "nm";
        nm.textContent = g.name;
        const pth = document.createElement("div");
        pth.className = "pth";
        const onl = 1 + g.members.filter((c) => S.meshOnline.has(c)).length;
        pth.textContent = g.members.length + 1 + " membres · " + onl + " en ligne";
        meta.appendChild(nm);
        meta.appendChild(pth);
        const open = document.createElement("button");
        open.className = "btn sm";
        open.textContent = "Ouvrir";
        open.onclick = () => openGroup(g.id);
        const del = document.createElement("button");
        del.className = "btn sm ghost-btn";
        del.textContent = "✕";
        del.title = "Supprimer / quitter";
        del.onclick = () => {
            if (!confirm('Supprimer / quitter le groupe « ' + g.name + " » ?"))
                return;
            const a = loadGroups();
            a.splice(i, 1);
            saveGroups(a);
            clearPInvGroup(g.id);
            renderGroups();
            if (S.openGroupId === g.id)
                closeGroup();
        };
        d.appendChild(meta);
        d.appendChild(open);
        d.appendChild(del);
        box.appendChild(d);
    });
}
function openGroup(id, skipDial) {
    const g = loadGroups().find((x) => x.id === id);
    if (!g)
        return;
    S.openGroupId = id;
    $("#groupChannelName").textContent = "👪 " + g.name;
    updateGroupLine(g);
    renderGroupMembers(g);
    $("#groupChannelCard").classList.remove("hidden");
    showTab("group");
    if (!skipDial)
        invoke("open_group", { members: friendsOnly(g.members) }).catch(() => { });
    renderGroupMsgs();
    refreshGroupCallUI();
}
function closeGroup() {
    if (S.inGroupCall && S.groupCallId === S.openGroupId)
        stopGroupCall();
    stopVideo();
    S.openGroupId = null;
    $("#groupChannelCard").classList.add("hidden");
    showTab("connect");
}
// ----- Chat de groupe -----
function addGroupMsgDom(author, text, who) {
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
function renderGroupMsgs() {
    const box = $("#groupChatLog");
    box.innerHTML = "";
    (S.groupMsgs[S.openGroupId || ""] || []).forEach((m) => addGroupMsgDom(m.author, m.text, m.who));
}
function pushGroupMsg(id, author, text, who) {
    (S.groupMsgs[id] = S.groupMsgs[id] || []).push({ author, text, who });
    if (S.groupMsgs[id].length > 200)
        S.groupMsgs[id].shift();
    if (S.openGroupId === id)
        addGroupMsgDom(author, text, who);
}
function sendGroupMsg() {
    if (!S.openGroupId)
        return;
    const text = $("#groupChatInput").value.trim();
    if (!text)
        return;
    const g = loadGroups().find((x) => x.id === S.openGroupId);
    if (!g)
        return;
    invoke("send_gchat", { members: g.members, gid: g.id, name: myName(), text }).catch((e) => log("Groupe : " + e));
    pushGroupMsg(g.id, myName() || "moi", text, "me");
    $("#groupChatInput").value = "";
}
// ----- Appel de groupe (audio) -----
function refreshGroupCallUI() {
    const active = S.inGroupCall && S.groupCallId === S.openGroupId;
    const b = $("#btnGroupCall");
    if (b)
        b.textContent = active ? "📵 Raccrocher" : "📞 Appel de groupe";
    const m = $("#btnGroupMute");
    if (m)
        m.classList.toggle("hidden", !active);
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
    if (g)
        renderGroupMembers(g);
}
async function startGroupCall(g, announce) {
    $("#btnGroupCall").disabled = true;
    try {
        await invoke("group_call_start", { members: g.members, gid: g.id, announce });
        S.inGroupCall = true;
        S.groupCallId = g.id;
        S.groupMuted = false;
        refreshGroupCallUI();
        log("📞 Appel de groupe " + (announce ? "lancé" : "rejoint") + ".");
    }
    catch (e) {
        log("Appel groupe : " + e);
        const s = $("#groupCallStatus");
        if (s)
            s.textContent = "erreur : " + e;
    }
    finally {
        $("#btnGroupCall").disabled = false;
    }
}
function stopGroupCall() {
    invoke("group_call_stop").catch(() => { });
    S.inGroupCall = false;
    S.groupCallId = null;
    refreshGroupCallUI();
    stopVideo();
    log("Appel de groupe terminé.");
}
// ----- Vidéo de groupe : webcam + partage d'écran (WebRTC) -----
function vgrid() {
    return $("#groupVideos");
}
function maybeHideGrid() {
    const g = vgrid();
    if (g && !g.children.length)
        g.classList.add("hidden");
}
function showTile(key, label, stream, self) {
    let w = document.getElementById("vidw_" + key);
    if (!w) {
        w = document.createElement("div");
        w.id = "vidw_" + key;
        w.style.cssText = "position:relative;border-radius:12px;overflow:hidden;background:#000";
        const v = document.createElement("video");
        v.id = "vid_" + key;
        v.autoplay = true;
        v.playsInline = true;
        v.muted = !!self;
        v.style.cssText = "width:100%;aspect-ratio:4/3;object-fit:cover;display:block";
        const tag = document.createElement("div");
        tag.style.cssText = "position:absolute;bottom:4px;left:6px;font-size:11px;font-weight:700;color:#fff;text-shadow:0 1px 3px #000";
        tag.textContent = label;
        w.appendChild(v);
        w.appendChild(tag);
        vgrid().appendChild(w);
    }
    document.getElementById("vid_" + key).srcObject = stream;
    vgrid().classList.remove("hidden");
}
function dropTile(key) {
    const w = document.getElementById("vidw_" + key);
    if (w)
        w.remove();
    maybeHideGrid();
}
function dropPeerTiles(peer) {
    document.querySelectorAll('[id^="vidw_' + peer + '_"]').forEach((w) => w.remove());
    maybeHideGrid();
}
function sigSend(peer, payload) {
    invoke("send_signal", { peer, data: JSON.stringify(payload) }).catch(() => { });
}
function localStreams() {
    return [S.localCam, S.localScreen].filter(Boolean);
}
function getPc(peer) {
    if (S.pcs[peer])
        return S.pcs[peer];
    const pc = new RTCPeerConnection(iceConfig());
    const st = { pc, makingOffer: false, polite: (S.myCode || "") < peer };
    S.pcs[peer] = st;
    localStreams().forEach((s) => s.getTracks().forEach((t) => {
        try {
            pc.addTrack(t, s);
        }
        catch {
            /* ignore */
        }
    }));
    pc.onnegotiationneeded = async () => {
        try {
            st.makingOffer = true;
            await pc.setLocalDescription();
            sigSend(peer, { description: pc.localDescription });
        }
        catch (e) {
            log("Vidéo: " + e);
        }
        finally {
            st.makingOffer = false;
        }
    };
    pc.onicecandidate = (ev) => {
        if (ev.candidate)
            sigSend(peer, { candidate: ev.candidate });
    };
    pc.ontrack = (ev) => {
        const stream = ev.streams[0];
        if (!stream)
            return;
        const key = peer + "_" + stream.id; // une vignette par flux (cam ET écran)
        showTile(key, memberName(peer), stream, false);
        const drop = () => dropTile(key);
        ev.track.onended = drop;
        ev.track.onmute = drop;
        stream.onremovetrack = () => {
            if (!stream.getTracks().length)
                drop();
        };
    };
    pc.onconnectionstatechange = () => {
        if (["failed", "closed", "disconnected"].includes(pc.connectionState))
            dropPeerTiles(peer);
    };
    return st;
}
function ensureGroupPcs() {
    const g = loadGroups().find((x) => x.id === S.openGroupId);
    if (g)
        g.members.forEach((code) => getPc(code));
}
function addStreamToPcs(stream) {
    Object.values(S.pcs).forEach((st) => stream.getTracks().forEach((t) => {
        if (!st.pc.getSenders().some((se) => se.track === t)) {
            try {
                st.pc.addTrack(t, stream);
            }
            catch {
                /* ignore */
            }
        }
    }));
}
function removeStreamFromPcs(stream) {
    Object.values(S.pcs).forEach((st) => st.pc
        .getSenders()
        .filter((se) => se.track && stream.getTracks().includes(se.track))
        .forEach((se) => {
        try {
            st.pc.removeTrack(se);
        }
        catch {
            /* ignore */
        }
    }));
}
function videoPrivacyOk() {
    if (localStorage.getItem("ghostlink_video_ok") === "1")
        return true;
    const ok = confirm("⚠️ Confidentialité — la caméra et le partage d'écran ouvrent une connexion vidéo DIRECTE (WebRTC) : ton adresse IP devient visible par les membres du groupe, et un serveur STUN public (Google) est contacté. Le chat, les fichiers et la voix restent relayés. Activer la vidéo quand même ?");
    if (ok)
        localStorage.setItem("ghostlink_video_ok", "1");
    return ok;
}
async function startCam() {
    if (!loadGroups().some((x) => x.id === S.openGroupId)) {
        log("Ouvre un groupe d'abord.");
        return;
    }
    if (!videoPrivacyOk())
        return;
    let s;
    try {
        s = await navigator.mediaDevices.getUserMedia({ video: true, audio: false });
    }
    catch (e) {
        log("Caméra : accès refusé ou indisponible (" + e + ")");
        return;
    }
    S.localCam = s;
    showTile("moi_cam", "Moi (cam)", s, true);
    ensureGroupPcs();
    addStreamToPcs(s);
    $("#btnGroupCam").textContent = "⏹️ Caméra";
    const vt = s.getVideoTracks()[0];
    if (vt)
        vt.onended = () => stopCam();
    log("📹 Caméra activée.");
}
async function startScreen() {
    if (!loadGroups().some((x) => x.id === S.openGroupId)) {
        log("Ouvre un groupe d'abord.");
        return;
    }
    if (!videoPrivacyOk())
        return;
    let s;
    try {
        s = await navigator.mediaDevices.getDisplayMedia({ video: true, audio: false });
    }
    catch (e) {
        log("Écran : accès refusé ou annulé (" + e + ")");
        return;
    }
    S.localScreen = s;
    showTile("moi_screen", "Moi (écran)", s, true);
    ensureGroupPcs();
    addStreamToPcs(s);
    $("#btnGroupScreen").textContent = "⏹️ Écran";
    const vt = s.getVideoTracks()[0];
    if (vt)
        vt.onended = () => stopScreen();
    log("🖥️ Partage d'écran lancé.");
}
function stopCam() {
    if (S.localCam) {
        removeStreamFromPcs(S.localCam);
        S.localCam.getTracks().forEach((t) => t.stop());
        S.localCam = null;
    }
    dropTile("moi_cam");
    $("#btnGroupCam").textContent = "📹 Caméra";
}
function stopScreen() {
    if (S.localScreen) {
        removeStreamFromPcs(S.localScreen);
        S.localScreen.getTracks().forEach((t) => t.stop());
        S.localScreen = null;
    }
    dropTile("moi_screen");
    $("#btnGroupScreen").textContent = "🖥️ Écran";
}
function stopVideo() {
    stopCam();
    stopScreen();
    Object.keys(S.pcs).forEach((peer) => {
        try {
            S.pcs[peer].pc.close();
        }
        catch {
            /* ignore */
        }
        dropPeerTiles(peer);
        delete S.pcs[peer];
    });
    vgrid().classList.add("hidden");
}
export function initGroups() {
    $("#btnCreateGroup").onclick = () => {
        const name = $("#groupName").value.trim();
        if (!name) {
            log("Donne un nom au groupe.");
            return;
        }
        const selected = Array.from($("#groupFriends").querySelectorAll("input:checked"))
            .map((c) => c.value)
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
        $("#groupName").value = "";
        renderGroups();
        const fullCsv = full.join(",");
        selected.forEach((code) => {
            addPInv(code, id, name, fullCsv);
            invoke("send_ginvite", { member: code, gid: id, name, members: fullCsv }).catch(() => { });
        });
        log("Groupe « " + name + " » créé — invitations envoyées (et mises en attente pour les membres hors ligne).");
        openGroup(id, true); // skipDial : send_ginvite a déjà connecté les membres en ligne
    };
    $("#btnCloseGroup").onclick = closeGroup;
    $("#btnGroupSend").onclick = sendGroupMsg;
    $("#groupChatInput").onkeydown = (e) => {
        if (e.key === "Enter") {
            e.preventDefault();
            sendGroupMsg();
        }
    };
    $("#btnGroupCall").onclick = () => {
        const g = loadGroups().find((x) => x.id === S.openGroupId);
        if (!g)
            return;
        if (S.inGroupCall && S.groupCallId === g.id) {
            stopGroupCall();
            return;
        }
        startGroupCall(g, true);
    };
    $("#btnGroupMute").onclick = () => {
        S.groupMuted = !S.groupMuted;
        invoke("group_call_mute", { on: S.groupMuted }).catch(() => { });
        refreshGroupCallUI();
    };
    $("#btnJoinGCall").onclick = () => {
        $("#gcallBanner").classList.add("hidden");
        const g = loadGroups().find((x) => x.id === S.pendingGCall);
        S.pendingGCall = null;
        if (!g || S.inGroupCall)
            return;
        openGroup(g.id);
        startGroupCall(g, false);
    };
    $("#btnDeclineGCall").onclick = () => {
        $("#gcallBanner").classList.add("hidden");
        S.pendingGCall = null;
    };
    $("#btnJoinGroup").onclick = () => {
        $("#groupInviteBanner").classList.add("hidden");
        if (!S.pendingInvite)
            return;
        const members = S.pendingInvite.full.filter((c) => c && c !== S.myCode);
        if (!loadGroups().some((x) => x.id === S.pendingInvite.id)) {
            saveGroups([...loadGroups(), { id: S.pendingInvite.id, name: S.pendingInvite.name, members }]);
        }
        invoke("open_group", { members: friendsOnly(members) }).catch(() => { });
        renderGroups();
        log("Groupe « " + S.pendingInvite.name + " » rejoint.");
        S.pendingInvite = null;
    };
    $("#btnDeclineGroup").onclick = () => {
        if (S.pendingInvite)
            addDeclined(S.pendingInvite.id);
        $("#groupInviteBanner").classList.add("hidden");
        S.pendingInvite = null;
    };
    $("#btnGroupCam").onclick = () => {
        if (S.localCam)
            stopCam();
        else
            startCam();
    };
    $("#btnGroupScreen").onclick = () => {
        if (S.localScreen)
            stopScreen();
        else
            startScreen();
    };
    $("#btnGroupFile").onclick = () => {
        const g = loadGroups().find((x) => x.id === S.openGroupId);
        if (!g)
            return;
        const path = $("#groupFilePath").value.trim();
        if (!path) {
            log("Colle le chemin d'un fichier à envoyer.");
            return;
        }
        invoke("send_gfile", { members: g.members, path })
            .then(() => {
            log("📎 Fichier envoyé au groupe.");
            $("#groupFilePath").value = "";
        })
            .catch((e) => log("Fichier groupe : " + e));
    };
    // Réessai doux des invitations encore en attente (membres hors ligne à la création).
    setInterval(() => {
        const p = loadPInv();
        if (!p.length)
            return;
        p.forEach((x) => invoke("send_ginvite", { member: x.member, gid: x.gid, name: x.name, members: x.csv }).catch(() => { }));
    }, 60000);
    // Listeners groupes
    listen("ghost-mesh-up", (e) => {
        if (e.payload) {
            S.meshOnline.add(e.payload);
            flushPInv(e.payload);
        }
        refreshGroupCounts();
    });
    listen("ghost-mesh-down", (e) => {
        if (e.payload)
            S.meshOnline.delete(e.payload);
        refreshGroupCounts();
    });
    listen("ghost-gchat", (e) => {
        const p = e.payload || {};
        if (!loadGroups().some((x) => x.id === p.group))
            return;
        pushGroupMsg(p.group, p.author || "?", p.text || "", "them");
    });
    listen("ghost-ginvite", (e) => {
        const p = e.payload || {};
        // BUG-1 : ignorer les ré-envois pour un groupe déjà rejoint ou déjà refusé.
        if (loadGroups().some((x) => x.id === p.id))
            return;
        if (p.id && declinedGroups().includes(p.id))
            return;
        const full = (p.members || "")
            .split(",")
            .map((s) => s.trim())
            .filter(Boolean);
        S.pendingInvite = { id: p.id, name: p.name || "Groupe", full };
        $("#groupInviteText").textContent =
            '👪 Invitation au groupe « ' + (p.name || "?") + " » (" + full.length + " membres).";
        $("#groupInviteBanner").classList.remove("hidden");
    });
    listen("ghost-gcall", (e) => {
        const p = e.payload || {};
        if (S.inGroupCall)
            return;
        const g = loadGroups().find((x) => x.id === p.group);
        if (!g)
            return;
        S.pendingGCall = g.id;
        $("#gcallText").textContent = '📞 Appel dans le groupe « ' + g.name + " » — rejoindre ?";
        $("#gcallBanner").classList.remove("hidden");
    });
    listen("ghost-signal", async (e) => {
        const p = e.payload || {};
        const peer = p.from;
        if (!peer)
            return;
        let msg;
        try {
            msg = JSON.parse(p.data);
        }
        catch {
            return;
        }
        const st = getPc(peer);
        const pc = st.pc;
        try {
            if (msg.description) {
                const collision = msg.description.type === "offer" && (st.makingOffer || pc.signalingState !== "stable");
                if (collision && !st.polite)
                    return;
                await pc.setRemoteDescription(msg.description);
                if (msg.description.type === "offer") {
                    await pc.setLocalDescription();
                    sigSend(peer, { description: pc.localDescription });
                }
            }
            else if (msg.candidate) {
                try {
                    await pc.addIceCandidate(msg.candidate);
                }
                catch {
                    /* ignore */
                }
            }
        }
        catch (err) {
            log("Vidéo: " + err);
        }
    });
    // Fichiers de groupe (SEC-1 : accord avant enregistrement)
    listen("ghost-grecv-start", (e) => {
        const p = e.payload || {};
        log("⬇️ Réception (groupe) de « " + (p.name || "") + " » de " + memberName(p.from || "") + "…");
    });
    listen("ghost-grecv-done", (e) => {
        const p = e.payload || {};
        log("✅ Reçu (groupe) : " + (p.name || ""));
    });
    listen("ghost-grecv-offer", (e) => {
        const p = e.payload || {};
        S.gfileOfferId = p.id ?? null;
        $("#gfileOfferText").textContent =
            '📥 (groupe) « ' + (p.name || "fichier") + " » (" + fmt(p.size || 0) + ") de " + memberName(p.from || "") + " — accepter ?";
        $("#gfileOfferBanner").classList.remove("hidden");
    });
    $("#btnGfileAccept").onclick = () => {
        if (S.gfileOfferId != null)
            invoke("respond_gfile", { id: S.gfileOfferId, accept: true }).catch(() => { });
        $("#gfileOfferBanner").classList.add("hidden");
        S.gfileOfferId = null;
    };
    $("#btnGfileReject").onclick = () => {
        if (S.gfileOfferId != null)
            invoke("respond_gfile", { id: S.gfileOfferId, accept: false }).catch(() => { });
        $("#gfileOfferBanner").classList.add("hidden");
        S.gfileOfferId = null;
        log("Fichier de groupe refusé.");
    };
    listen("ghost-grecv-rejected", (e) => {
        const p = e.payload || {};
        log("Fichier de groupe refusé : " + (p.name || ""));
    });
    listen("ghost-grecv-corrupt", (e) => {
        const p = e.payload || {};
        log("⚠️ Fichier de groupe corrompu (intégrité invalide) — rejeté : " + (p.name || ""));
    });
}
