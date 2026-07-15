// Groupes : channel multi-pairs (chat), appel de groupe (audio), vidéo (WebRTC), fichiers.
import { invoke, listen } from "./tauri.js";
import { $, log, fmt } from "./dom.js";
import { S, PINV, GDECL, iceConfig, nativeVideoWanted, loadGroups, saveGroups, loadFriends, friendsOnly, memberName, myName, } from "./state.js";
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
        const c = document.createElement("div");
        c.className = "mem";
        const d = document.createElement("span");
        d.className = online ? "dot on" : "dot";
        const t = document.createElement("span");
        t.className = "grow";
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
            const pct = document.createElement("span");
            pct.style.cssText = "font-size:11px;color:var(--muted);min-width:34px;text-align:right;flex:0 0 auto;font-variant-numeric:tabular-nums";
            pct.textContent = r.value + "%";
            r.oninput = () => {
                S.groupGains[code] = +r.value;
                pct.textContent = r.value + "%";
                invoke("group_call_volume", { peer: code, vol: +r.value / 100 }).catch(() => { });
            };
            c.appendChild(r);
            c.appendChild(pct);
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
        del.onclick = (e) => {
            e.stopPropagation();
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
        d.onclick = () => openGroup(g.id);
        d.appendChild(av);
        d.appendChild(nm);
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
    renderGroups(); // BUG : déplacer le surlignage « actif » vers le groupe ouvert (sinon il reste collé au 1er).
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
// L'encodeur MATÉRIEL H.264 est confirmé actif → le partage a été remonté en 1080p.
// Jamais posé sur encodeur logiciel (le 1080p logiciel ramènerait le plancher 3 fps).
let screenUpgraded = false;
function vgrid() {
    return $("#groupVideos");
}
function maybeHideGrid() {
    const g = vgrid();
    if (g && !g.children.length)
        g.classList.add("hidden");
}
// Agrandir une vignette (cam / partage d'écran) en plein écran. En plein écran,
// le CSS bascule en object-fit:contain pour voir TOUT l'écran partagé sans rognage.
function toggleTileFullscreen(el) {
    if (document.fullscreenElement) {
        document.exitFullscreen().catch(() => { });
        if (document.fullscreenElement === el)
            return; // c'était cette vignette : on l'a juste refermée
    }
    if (document.fullscreenElement !== el && el.requestFullscreen) {
        el.requestFullscreen().catch(() => log("Plein écran indisponible sur cette vue."));
    }
}
function showTile(key, label, stream, self, peer) {
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
        max.onclick = (e) => {
            e.stopPropagation();
            toggleTileFullscreen(wrap);
        };
        w.onclick = () => toggleTileFullscreen(wrap);
        w.appendChild(v);
        w.appendChild(tag);
        w.appendChild(max);
        if (!self) {
            // Bouton son DU PARTAGE, sur la vignette (accessible aussi en plein écran).
            // Coupe localement le son de ce pair — LES DEUX voies : l'audio d'une piste
            // WebRTC (v.muted) ET le son système capté en natif, qui ne passe PAS par la
            // vidéo mais par le mixeur de l'appel (screen_audio_mute). L'état est stocké
            // PAR PAIR (S.screenMuted) : un pair peut avoir 2 vignettes (cam + écran) —
            // toutes ses vignettes partagent le même état et sont synchronisées au clic.
            const snd = document.createElement("button");
            snd.className = "vidsnd";
            snd.type = "button";
            if (peer)
                snd.dataset.snd = peer;
            const muted0 = !!peer && !!S.screenMuted[peer];
            snd.textContent = muted0 ? "🔇" : "🔊";
            snd.title = "Couper / remettre le son de ce partage";
            snd.onclick = (e) => {
                e.stopPropagation();
                if (!peer) {
                    v.muted = !v.muted;
                    snd.textContent = v.muted ? "🔇" : "🔊";
                    return;
                }
                const on = !S.screenMuted[peer];
                S.screenMuted[peer] = on;
                invoke("screen_audio_mute", { peer, on }).catch(() => { });
                // Synchroniser TOUTES les vignettes du pair (cam + écran) : élément vidéo + icône.
                document.querySelectorAll('[id^="vidw_' + peer + '_"] video').forEach((el) => {
                    el.muted = on;
                });
                document.querySelectorAll('[data-snd="' + peer + '"]').forEach((b) => {
                    b.textContent = on ? "🔇" : "🔊";
                });
            };
            w.appendChild(snd);
            // Ré-affirmer la coupure au backend : après une reconnexion, receive_group_voice
            // a pu réinitialiser le gain à 1.0 — sans ça, l'icône dirait 🔇 mais le son jouerait.
            if (muted0)
                invoke("screen_audio_mute", { peer: peer, on: true }).catch(() => { });
        }
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
// Vignette à CANVAS (vidéo native décodée par WebCodecs — pas de MediaStream).
// Même enveloppe que showTile : plein écran au clic, bouton son du partage pour un
// pair (le son natif SCREEN_TAG passe par le mixeur de l'appel, pas par la vignette).
function showCanvasTile(key, label, peer) {
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
        max.onclick = (e) => {
            e.stopPropagation();
            toggleTileFullscreen(wrap);
        };
        w.onclick = () => toggleTileFullscreen(wrap);
        w.appendChild(c);
        w.appendChild(tag);
        w.appendChild(max);
        if (peer) {
            // Ici il n'y a pas d'élément <video> : seule la voie « son système natif »
            // (screen_audio_mute) est à couper — l'état par pair reste S.screenMuted.
            const snd = document.createElement("button");
            snd.className = "vidsnd";
            snd.type = "button";
            snd.dataset.snd = peer;
            const muted0 = !!S.screenMuted[peer];
            snd.textContent = muted0 ? "🔇" : "🔊";
            snd.title = "Couper / remettre le son de ce partage";
            snd.onclick = (e) => {
                e.stopPropagation();
                const on = !S.screenMuted[peer];
                S.screenMuted[peer] = on;
                invoke("screen_audio_mute", { peer, on }).catch(() => { });
                document.querySelectorAll('[id^="vidw_' + peer + '_"] video').forEach((el) => {
                    el.muted = on;
                });
                document.querySelectorAll('[data-snd="' + peer + '"]').forEach((b) => {
                    b.textContent = on ? "🔇" : "🔊";
                });
            };
            w.appendChild(snd);
            if (muted0)
                invoke("screen_audio_mute", { peer, on: true }).catch(() => { });
        }
        vgrid().appendChild(w);
    }
    vgrid().classList.remove("hidden");
    return document.getElementById("vid_" + key);
}
function dropPeerTiles(peer) {
    // Nettoyage WebRTC uniquement : la vignette du partage NATIF (suffixe _natscr)
    // partage le préfixe d'id mais vit sur le flux QUIC, indépendant des
    // RTCPeerConnection — un échec ICE (caméra) ne doit pas la détruire.
    document.querySelectorAll('[id^="vidw_' + peer + '_"]').forEach((w) => {
        if (!w.id.endsWith(NATIVE_KEY))
            w.remove();
    });
    maybeHideGrid();
}
function sigSend(peer, payload) {
    invoke("send_signal", { peer, data: JSON.stringify(payload) }).catch(() => { });
}
function localStreams() {
    return [S.localCam, S.localScreen].filter(Boolean);
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
function tuneVideoSender(pc, track) {
    if (track.kind !== "video")
        return;
    const isScreen = !!(S.localScreen && S.localScreen.getTracks().includes(track));
    try {
        track.contentHint = isScreen ? "detail" : "motion";
    }
    catch {
        /* ignore */
    }
    const sender = pc.getSenders().find((se) => se.track === track);
    if (!sender)
        return;
    try {
        const p = sender.getParameters();
        if (!p.encodings || !p.encodings.length)
            p.encodings = [{}];
        p.encodings[0].maxFramerate = 30;
        // Écran : 3 Mb/s en 720p (base), 4,5 Mb/s une fois remonté en 1080p sur
        // encodeur matériel confirmé (VID-6) — sinon une renégociation écraserait
        // le débit relevé par maybeUpgradeScreen.
        p.encodings[0].maxBitrate = isScreen ? (screenUpgraded ? 4500000 : 3000000) : 5000000;
        p.degradationPreference =
            isScreen ? "maintain-resolution" : "maintain-framerate";
        // Échec visible en DevTools : un setParameters avalé en silence peut masquer des
        // réglages jamais appliqués (encodings encore vides avant la 1re négociation).
        void sender.setParameters(p).catch((err) => console.warn("setParameters:", err));
    }
    catch (err) {
        console.warn("tuneVideoSender:", err);
    }
}
// Ré-appliquer le réglage une fois la négociation « stable » : sur d'anciens moteurs,
// encodings est vide avant la 1re offre et setParameters échoue en silence — après
// l'answer, il existe toujours et le plafond fps/bitrate prend à coup sûr.
function retuneSenders(pc) {
    pc.getSenders().forEach((se) => {
        if (se.track)
            tuneVideoSender(pc, se.track);
    });
}
// VID-6 : préférer H.264 sur chaque transceiver vidéo. Chromium n'encode VP8/VP9
// qu'en LOGICIEL (libvpx, un cœur CPU saturé = tout l'historique yo-yo/3 fps),
// alors que H.264 passe par l'encodeur MATÉRIEL du GPU (Media Foundation :
// NVENC/AMF/QuickSync). Les deux extrémités sont le même moteur WebView2 → le
// décodage H.264 est garanti. À appeler côté offreur (avant la négociation) ET
// côté répondeur (ontrack, avant l'answer — c'est l'ordre de l'ANSWER qui gagne).
function preferH264(tr) {
    try {
        // receiver.track existe toujours (même sans média) et porte le kind du
        // transceiver : ne toucher QUE la vidéo (poser des codecs vidéo sur un
        // transceiver AUDIO — piste audio navigateur du partage — serait rejeté).
        if (!tr.receiver.track || tr.receiver.track.kind !== "video")
            return;
        // Caps du RÉCEPTEUR : depuis Chromium M124, setCodecPreferences valide la liste
        // contre RTCRtpReceiver.getCapabilities — une liste bâtie sur les caps du sender
        // peut contenir un codec inconnu du receiver et tout rejeter (silencieusement ici).
        const caps = RTCRtpReceiver.getCapabilities("video");
        if (!caps || !caps.codecs.length)
            return;
        const isH264 = (c) => /h264/i.test(c.mimeType);
        const h264 = caps.codecs.filter(isH264);
        if (!h264.length)
            return; // pas de H.264 sur ce moteur : négociation par défaut
        // packetization-mode=1 d'abord (le profil le plus largement accéléré).
        h264.sort((a, b) => Number(/packetization-mode=1/.test(b.sdpFmtpLine || "")) -
            Number(/packetization-mode=1/.test(a.sdpFmtpLine || "")));
        tr.setCodecPreferences([...h264, ...caps.codecs.filter((c) => !isH264(c))]);
    }
    catch {
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
let screenWatchdog = null;
/// Le sender VIDÉO du partage d'écran sur un pc (jamais l'audio : la piste audio
/// navigateur arrive AVANT la vidéo dans getTracks() et n'a pas d'encoderImplementation).
function screenVideoSenderOf(pc) {
    return pc
        .getSenders()
        .find((se) => se.track && se.track.kind === "video" && S.localScreen && S.localScreen.getTracks().includes(se.track));
}
/// encoderImplementation de l'encodeur écran d'UN pc ("" = pas encore disponible).
async function screenEncoderImpl(pc) {
    const sender = screenVideoSenderOf(pc);
    if (!sender)
        return "";
    try {
        const stats = await sender.getStats();
        let impl = "";
        stats.forEach((r) => {
            const o = r;
            if (o.type === "outbound-rtp" && o.encoderImplementation)
                impl = o.encoderImplementation;
        });
        return impl;
    }
    catch {
        return "";
    }
}
function applyScreenBitrate(bps) {
    Object.values(S.pcs).forEach((st) => {
        const se = screenVideoSenderOf(st.pc);
        if (!se)
            return;
        try {
            const p = se.getParameters();
            if (p.encodings && p.encodings.length) {
                p.encodings[0].maxBitrate = bps;
                void se.setParameters(p).catch((err) => console.warn("setParameters:", err));
            }
        }
        catch (err) {
            console.warn("bitrate écran:", err);
        }
    });
}
// Vérifie quel encodeur tourne VRAIMENT (stats WebRTC) sur TOUS les pairs : upgrade
// seulement à l'unanimité matérielle. Ré-essaie quelques fois : encoderImplementation
// n'apparaît qu'après les premières trames encodées.
function maybeUpgradeScreen(attempt = 0) {
    if (screenUpgraded || screenEncoderChecked || screenCheckBusy || attempt > 3 || !S.localScreen)
        return;
    const track = S.localScreen.getVideoTracks()[0];
    if (!track)
        return;
    screenCheckBusy = true;
    setTimeout(async () => {
        try {
            if (screenUpgraded || screenEncoderChecked || !S.localScreen || !S.localScreen.getVideoTracks().includes(track))
                return;
            const pcs = Object.values(S.pcs)
                .map((st) => st.pc)
                .filter((pc) => screenVideoSenderOf(pc));
            if (!pcs.length)
                return; // pas encore de sender écran : un prochain « stable » relancera
            const impls = await Promise.all(pcs.map(screenEncoderImpl));
            if (impls.some((i) => !i || /unknown/i.test(i))) {
                screenCheckBusy = false;
                maybeUpgradeScreen(attempt + 1); // au moins un pair n'a pas encore de stats
                return;
            }
            const soft = impls.find((i) => !HW_ENCODER.test(i));
            if (soft !== undefined) {
                screenEncoderChecked = true; // verdict pour CE partage : ne plus re-vérifier ni re-logger
                log("ℹ️ Encodeur logiciel détecté (" + soft + ") — le partage reste en 720p30.");
                return;
            }
            // Re-vérification de vie APRÈS les await : le partage a pu s'arrêter entre-temps.
            if (screenUpgraded || !S.localScreen || !S.localScreen.getVideoTracks().includes(track))
                return;
            screenUpgraded = true;
            try {
                await track.applyConstraints({
                    width: { max: 1920 },
                    height: { max: 1080 },
                    frameRate: { ideal: 30, max: 30 },
                });
            }
            catch {
                screenUpgraded = false;
                return;
            }
            applyScreenBitrate(4500000);
            log("🎮 Encodeur matériel confirmé sur tous les pairs — partage remonté en 1080p30.");
            armScreenWatchdog(track);
        }
        finally {
            screenCheckBusy = false;
        }
    }, 2500 + attempt * 3000);
}
// Après l'upgrade : re-vérification périodique. libwebrtc peut retomber en LOGICIEL
// en cours de flux (échec Media Foundation, sessions GPU épuisées — y compris à cause
// de la reconfiguration 1080p elle-même) sans AUCUN événement JS ; et un pair qui
// REJOINT après l'upgrade peut n'obtenir qu'un encodeur logiciel. Dans les deux cas :
// redescendre en 720p30 et ne plus retenter pour ce partage.
function armScreenWatchdog(track) {
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
        if (bad === undefined)
            return;
        screenUpgraded = false;
        screenEncoderChecked = true;
        disarmScreenWatchdog();
        try {
            await track.applyConstraints({ width: { max: 1280 }, height: { max: 720 }, frameRate: { ideal: 30, max: 30 } });
        }
        catch {
            /* le track a pu se terminer : rien à faire */
        }
        applyScreenBitrate(3000000);
        log("ℹ️ Encodeur logiciel apparu (" + bad + ") — partage redescendu en 720p30.");
    }, 12000);
}
function disarmScreenWatchdog() {
    if (screenWatchdog != null) {
        clearInterval(screenWatchdog);
        screenWatchdog = null;
    }
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
            tuneVideoSender(pc, t);
        }
        catch {
            /* ignore */
        }
    }));
    pc.getTransceivers().forEach(preferH264); // AVANT la 1re négociation (VID-6)
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
        // Côté répondeur : ontrack arrive pendant setRemoteDescription, AVANT l'answer —
        // c'est le bon moment pour imposer H.264 (l'ordre de l'answer fait foi).
        if (ev.transceiver)
            preferH264(ev.transceiver);
        const stream = ev.streams[0];
        if (!stream)
            return;
        const key = peer + "_" + stream.id; // une vignette par flux (cam ET écran)
        showTile(key, memberName(peer), stream, false, peer);
        const drop = () => dropTile(key);
        // VID-2 : NE PAS retirer sur `onmute` — un mute survient sur une perte de paquets
        // transitoire (puis unmute) ; retirer ferait clignoter/disparaître la vignette à tort.
        ev.track.onended = drop;
        stream.onremovetrack = () => {
            if (!stream.getTracks().length)
                drop();
        };
    };
    pc.onconnectionstatechange = () => {
        // VID-2 : `disconnected` est souvent transitoire (il peut repasser `connected`).
        // On ne nettoie que sur un état réellement terminal.
        if (["failed", "closed"].includes(pc.connectionState))
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
    Object.values(S.pcs).forEach((st) => {
        stream.getTracks().forEach((t) => {
            if (!st.pc.getSenders().some((se) => se.track === t)) {
                try {
                    st.pc.addTrack(t, stream);
                    tuneVideoSender(st.pc, t);
                }
                catch {
                    /* ignore */
                }
            }
        });
        st.pc.getTransceivers().forEach(preferH264); // les nouveaux transceivers (VID-6)
    });
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
// Comme Discord : on ne partage cam/écran QUE pendant l'appel de groupe
// (sinon le maillage des autres n'est pas prêt et personne ne voit le flux).
function inThisCall() {
    return S.inGroupCall && S.groupCallId === S.openGroupId;
}
async function startCam() {
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
    if (!videoPrivacyOk())
        return;
    if (firstPrivacy) {
        log("Confidentialité acceptée ✔ — re-clique sur 📹 Caméra pour l'activer.");
        return;
    }
    let s;
    try {
        s = await navigator.mediaDevices.getUserMedia({ video: { frameRate: { ideal: 30 } }, audio: false });
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
    if (screenBusy)
        return; // sélecteur/démarrage déjà en cours (double-clic)
    if (!loadGroups().some((x) => x.id === S.openGroupId)) {
        log("Ouvre un groupe d'abord.");
        return;
    }
    if (!inThisCall()) {
        log("📞 Rejoins d'abord l'appel de groupe — comme sur Discord, l'écran se partage dans l'appel.");
        return;
    }
    // 🧪 Chemin NATIF (Réglages) : pas de getDisplayMedia, pas de WebRTC — et pas de
    // confirm() de confidentialité : aucune IP n'est exposée, aucun STUN contacté.
    if (nativeVideoWanted()) {
        await startScreenNative();
        return;
    }
    // Au TOUT premier usage vidéo, le confirm() de confidentialité consomme le délai
    // d'activation utilisateur (~5 s, Chromium 111+) : getDisplayMedia rejetterait
    // InvalidStateError. On s'arrête proprement et on demande un clic « frais ».
    const firstPrivacy = localStorage.getItem("ghostlink_video_ok") !== "1";
    if (!videoPrivacyOk())
        return;
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
        const opts = wantBrowserAudio
            ? {
                video: vconf,
                audio: true,
                systemAudio: "include", // fait apparaître la case (onglet « Écran entier »)
                restrictOwnAudio: true, // exclut le son émis par la WebView elle-même
            }
            : { video: vconf };
        let s;
        try {
            s = await navigator.mediaDevices.getDisplayMedia(opts);
        }
        catch (e) {
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
            }
            catch (e2) {
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
                svt.contentHint = "detail";
            }
            catch {
                /* ignore */
            }
        }
        S.localScreen = s;
        showTile("moi_screen", "Moi (écran)", s, true);
        ensureGroupPcs();
        addStreamToPcs(s);
        $("#btnGroupScreen").textContent = "⏹️ Écran";
        if (svt)
            svt.onended = () => stopScreen();
        log("🖥️ Partage d'écran lancé.");
        // ---- Décision « son », le flux vidéo étant déjà parti ----
        if (s.getAudioTracks().length) {
            log("🔊 Son système partagé avec l'écran (navigateur).");
            return;
        }
        const surface = svt
            ? svt.getSettings().displaySurface
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
        const wantNative = confirm("Partager aussi le SON ?\n\nLe navigateur ne fournit pas l'audio ici — ghost link peut capter le son système en natif (TOUT le son du PC, pas seulement la fenêtre partagée). Le flux chiffré part vers les membres du groupe en ligne ; seuls ceux qui ont rejoint l'appel l'entendent.\n\nOK = capter le son système · Annuler = vidéo seule (pour ajouter le son ensuite : arrête ⏹️ puis relance le partage)");
        if (!wantNative)
            return;
        // Garde anti-course : le partage a pu être arrêté PENDANT le confirm (bouton stop
        // de la barre Chromium → onended en file d'attente). Ne rien capter dans ce cas.
        if (S.localScreen !== s)
            return;
        const g = loadGroups().find((x) => x.id === S.openGroupId);
        try {
            await invoke("screen_audio_start", { members: g ? g.members : [] });
            // Le partage a pu être arrêté PENDANT l'await (jusqu'à 5 s côté Rust) : stopScreen
            // a alors vu screenAudioNative=false et n'a rien arrêté — compenser ici, sinon la
            // capture du son système continuerait ORPHELINE (fuite de confidentialité).
            if (S.localScreen !== s) {
                invoke("screen_audio_stop").catch(() => { });
                return;
            }
            screenAudioNative = true;
            // Indicateur permanent tant que le son système est capté en natif.
            $("#btnGroupScreen").textContent = "⏹️ Écran · 🔴 son système";
            // Anti-écho À LA SOURCE : la capture exclut le processus de ghost link
            // (process-loopback EXCLUDE), donc les voix de l'appel qu'on joue ne sont jamais
            // réinjectées — les autres n'entendent QUE le son de l'appli partagée.
            log("🔊 Son système capté en natif — les voix de l'appel sont exclues du flux (pas d'écho).");
        }
        catch (e) {
            log("🔇 Repli natif indisponible (" + e + ").");
        }
    }
    finally {
        screenBusy = false;
    }
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
    // Invalide toute init de partage natif encore en vol (voir nativeShareEpoch) —
    // y compris quand S.localScreenNative n'est pas encore posé (raccrochage pendant
    // l'init : c'est exactement la fenêtre où l'état fantôme naissait).
    nativeShareEpoch += 1;
    if (S.localScreenNative) {
        S.localScreenNative = false;
        invoke("video_share_stop").catch(() => { });
        // Signal d'arrêt aux DESTINATAIRES du partage (mémorisés au démarrage) — pas au
        // groupe actuellement ouvert, qui a pu changer entre-temps.
        (nativeShareMembers || []).forEach((m) => {
            if (S.meshOnline.has(m))
                sigSend(m, { nativeVideo: { start: false } });
        });
        nativeShareMembers = null;
        dropTile("moi" + NATIVE_KEY);
    }
    screenUpgraded = false; // le prochain partage repart en 720p jusqu'à confirmation
    screenEncoderChecked = false;
    disarmScreenWatchdog();
    if (screenAudioNative) {
        screenAudioNative = false;
        invoke("screen_audio_stop").catch(() => { });
    }
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
const nativeRx = {};
const utf8 = new TextDecoder();
// Pierre tombale anti-résurrection : après un arrêt, des images encore en vol
// recréeraient la vignette (figée pour toujours). On les ignore quelques secondes —
// sauf si la trame porte le bit « nouvelle session » (un partage relancé, légitime).
const nativeTomb = {};
// Flux déclaré INDÉCODABLE sur ce moteur (15 resyncs sans une image) : on arrête
// d'essayer jusqu'à une VRAIE relance (bit newSession ou signal start) — sinon la
// pierre tombale expirerait et le cycle vignette noire → fermeture repartirait.
const nativeBroken = new Set();
// Membres et groupe du partage natif ÉMIS en cours : le signal d'arrêt doit aller
// aux destinataires du partage, pas aux membres du groupe actuellement OUVERT.
let nativeShareMembers = null;
// Époque du partage natif émis : incrémentée par TOUT arrêt (stopScreen, erreur
// encodeur). startScreenNative la snapshote avant l'invoke et n'engage l'état
// « partage actif » que si rien ne l'a interrompu PENDANT l'init.
let nativeShareEpoch = 0;
/// La vidéo native d'un pair n'est décodée/affichée QUE si on est dans l'appel du
/// groupe dont il est membre — même règle que l'émission (« comme Discord ») ; un
/// ami hors de ce cadre ne peut pas imposer une vignette ni brûler du CPU décodeur.
function nativePeerAllowed(peer) {
    if (!S.inGroupCall || !S.groupCallId)
        return false;
    const g = loadGroups().find((x) => x.id === S.groupCallId);
    return !!g && g.members.includes(peer);
}
/// Codec WebCodecs selon la résolution annoncée (H.264 Main ; niveau ≥ résolution).
function nativeCodecOf(w, h) {
    const px = w * h;
    if (px > 0 && px <= 1280 * 720)
        return "avc1.4d401f"; // Main 3.1
    if (px > 0 && px <= 1920 * 1080)
        return "avc1.4d4028"; // Main 4.0
    return "avc1.4d4033"; // Main 5.1 — couvre 1440p+ et les tailles inconnues
}
function ensureNativeRx(peer, w, h, fps) {
    let st = nativeRx[peer];
    if (!st) {
        const canvas = showCanvasTile(peer + NATIVE_KEY, memberName(peer) + " (écran)", peer);
        st = { dec: null, waitKey: true, fps: fps || 30, w, h, errors: 0, lastFrameAt: 0, canvas };
        nativeRx[peer] = st;
    }
    else {
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
function resetNativeDecoder(st) {
    if (st.dec && st.dec.state !== "closed") {
        try {
            st.dec.close();
        }
        catch {
            /* ignore */
        }
    }
    st.dec = null;
    st.waitKey = true;
}
function closeNativeRx(peer) {
    const st = nativeRx[peer];
    if (!st)
        return;
    resetNativeDecoder(st);
    delete nativeRx[peer];
    nativeTomb[peer] = Date.now();
    dropTile(peer + NATIVE_KEY);
}
/// Une image reçue sur le canal binaire. Le bench exp3 a montré que les payloads
/// arrivent en Array de nombres (pas d'ArrayBuffer) : on normalise en Uint8Array.
function handleNativeFrame(raw) {
    let b;
    if (raw instanceof ArrayBuffer)
        b = new Uint8Array(raw);
    else if (ArrayBuffer.isView(raw))
        b = new Uint8Array(raw.buffer, raw.byteOffset, raw.byteLength);
    else if (Array.isArray(raw))
        b = Uint8Array.from(raw);
    else
        return;
    if (b.length < 11)
        return;
    const pl = b[0];
    if (b.length <= 10 + pl)
        return;
    const peer = utf8.decode(b.subarray(1, 1 + pl));
    const key = (b[1 + pl] & 1) === 1;
    const newSession = (b[1 + pl] & 2) === 2; // 1re trame d'un (re)démarrage du partage
    let id = 0; // u64 BE lu en Number : 2^53 trames = des millénaires à 30 fps
    for (let i = 2 + pl; i < 10 + pl; i++)
        id = id * 256 + b[i];
    const data = b.subarray(10 + pl);
    if (typeof VideoDecoder === "undefined")
        return; // moteur sans WebCodecs
    if (!nativePeerAllowed(peer))
        return; // hors appel du groupe : ne rien décoder
    // Images en vol après un arrêt : ne pas ressusciter la vignette (sauf vraie relance).
    if (newSession) {
        delete nativeTomb[peer];
        nativeBroken.delete(peer);
    }
    else {
        if (nativeBroken.has(peer))
            return; // flux déclaré indécodable : attendre une relance
        if (nativeTomb[peer] && Date.now() - nativeTomb[peer] < 3000)
            return;
    }
    // Vignette/état créés au besoin : les images peuvent devancer le signal de début.
    const st = ensureNativeRx(peer, 0, 0, 30);
    st.lastFrameAt = Date.now();
    if (newSession)
        resetNativeDecoder(st); // nouveau flux = nouveaux timestamps/SPS
    if (st.waitKey && !key)
        return;
    if (!st.dec || st.dec.state === "closed") {
        if (!key) {
            st.waitKey = true;
            return;
        }
        const decoder = new VideoDecoder({
            output: (frame) => {
                st.errors = 0;
                const c = st.canvas;
                if (c.width !== frame.displayWidth || c.height !== frame.displayHeight) {
                    c.width = frame.displayWidth;
                    c.height = frame.displayHeight;
                }
                const ctx = c.getContext("2d");
                if (ctx)
                    ctx.drawImage(frame, 0, 0);
                frame.close();
            },
            // Erreur de décodage (référence manquante, reconfiguration…) : on repart
            // proprement sur la prochaine image clé au lieu d'afficher de la bouillie.
            error: () => noteNativeDecodeError(peer, st),
        });
        try {
            decoder.configure({ codec: nativeCodecOf(st.w, st.h), optimizeForLatency: true });
        }
        catch {
            return; // codec refusé : on retentera à la prochaine clé
        }
        st.dec = decoder;
    }
    st.waitKey = false;
    try {
        st.dec.decode(new EncodedVideoChunk({
            type: key ? "key" : "delta",
            timestamp: Math.round((id * 1000000) / (st.fps || 30)),
            data,
        }));
    }
    catch {
        noteNativeDecodeError(peer, st);
    }
}
/// Erreur de décodage : visible (une fois par rafale), et si ça ne se remet JAMAIS
/// (15 resyncs consécutifs sans une seule image sortie), on ferme au lieu de boucler
/// en silence sur une vignette noire.
function noteNativeDecodeError(peer, st) {
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
function handleNativeSignal(peer, nv) {
    if (nv.start) {
        if (!nativePeerAllowed(peer)) {
            log("🖥️ " + memberName(peer) + " partage son écran (natif) — rejoins l'appel du groupe pour le voir.");
            return;
        }
        delete nativeTomb[peer];
        nativeBroken.delete(peer);
        ensureNativeRx(peer, nv.w || 0, nv.h || 0, nv.fps || 30);
        log("🖥️ " + memberName(peer) + " partage son écran (natif" + (nv.w ? " " + nv.w + "×" + nv.h : "") + ").");
    }
    else {
        closeNativeRx(peer);
    }
}
/// Abonne cette page au flux vidéo natif entrant (une fois, au chargement).
function initNativeVideoRx() {
    try {
        const ch = new window.__TAURI__.core.Channel();
        ch.onmessage = handleNativeFrame;
        invoke("video_receive_attach", { channel: ch }).catch((e) => {
            log("⚠️ Réception vidéo native indisponible (" + e + ") — les partages natifs des autres ne s'afficheront pas.");
        });
    }
    catch {
        log("⚠️ Réception vidéo native indisponible sur ce moteur (pas de canal binaire).");
    }
}
/// Vignette locale du partage natif : un panneau statique (pas d'aperçu — les images
/// encodées ne repassent pas par la WebView, c'est le prix du zéro-copie local).
function showNativePlaceholder(w, h) {
    const c = showCanvasTile("moi" + NATIVE_KEY, "Moi (écran · natif)");
    c.width = 320;
    c.height = 240;
    const ctx = c.getContext("2d");
    if (!ctx)
        return;
    ctx.fillStyle = "#0b0b10";
    ctx.fillRect(0, 0, c.width, c.height);
    ctx.fillStyle = "#8b8b9a";
    ctx.font = "13px sans-serif";
    ctx.textAlign = "center";
    ctx.fillText("🖥️ Écran partagé en " + w + "×" + h, c.width / 2, c.height / 2 - 8);
    ctx.fillText("(natif, sans aperçu local)", c.width / 2, c.height / 2 + 14);
}
async function startScreenNative() {
    const g = loadGroups().find((x) => x.id === S.openGroupId);
    if (!g)
        return;
    screenBusy = true;
    try {
        const epoch0 = nativeShareEpoch;
        let info;
        try {
            info = await invoke("video_share_start", { members: g.members });
        }
        catch (e) {
            log("🧪 Partage natif impossible : " + e + " — décoche « Partage d'écran natif » dans Réglages pour repasser en WebRTC.");
            return;
        }
        // L'init a pu durer plusieurs secondes : si l'appel s'est terminé PENDANT (même
        // s'il a été REJOINT depuis), ou si l'encodeur est déjà mort (ghost-video-ended
        // incrémente l'époque), ne pas afficher un état « partage actif » fantôme.
        if (!inThisCall() || nativeShareEpoch !== epoch0) {
            invoke("video_share_stop").catch(() => { });
            log("Partage natif annulé — interrompu pendant le démarrage.");
            return;
        }
        S.localScreenNative = true;
        nativeShareMembers = g.members.slice(); // destinataires du signal d'arrêt
        // Annonce aux membres en ligne. Limite v1 : un membre qui arrive APRÈS le
        // démarrage ne reçoit pas ce partage (relancer ⏹️/🖥️ pour l'inclure).
        g.members.forEach((m) => {
            if (S.meshOnline.has(m))
                sigSend(m, { nativeVideo: { start: true, w: info.w, h: info.h, fps: info.fps } });
        });
        showNativePlaceholder(info.w, info.h);
        $("#btnGroupScreen").textContent = "⏹️ Écran";
        log("🖥️ Partage d'écran NATIF lancé (" + info.w + "×" + info.h + "@" + info.fps + ", H.264 matériel, sans WebRTC/STUN).");
        // Son : le chemin natif n'a jamais d'audio navigateur — proposer directement le
        // repli système (loopback anti-écho), comme pour une fenêtre en WebRTC.
        const wantNative = confirm("Partager aussi le SON ?\n\nghost link peut capter le son système en natif (TOUT le son du PC). Le flux chiffré part vers les membres du groupe en ligne ; seuls ceux qui ont rejoint l'appel l'entendent.\n\nOK = capter le son système · Annuler = vidéo seule");
        if (!wantNative || !S.localScreenNative)
            return;
        try {
            await invoke("screen_audio_start", { members: g.members });
            if (!S.localScreenNative) {
                // Partage arrêté PENDANT l'await : ne pas laisser la capture orpheline.
                invoke("screen_audio_stop").catch(() => { });
                return;
            }
            screenAudioNative = true;
            $("#btnGroupScreen").textContent = "⏹️ Écran · 🔴 son système";
            log("🔊 Son système capté en natif — les voix de l'appel sont exclues du flux (pas d'écho).");
        }
        catch (e) {
            log("🔇 Son système indisponible (" + e + ").");
        }
    }
    finally {
        screenBusy = false;
    }
}
export function initGroups() {
    initNativeVideoRx();
    // L'émetteur natif s'est arrêté sur une ERREUR (encodeur, GPU…) — pas via stop().
    // Pas de garde sur S.localScreenNative : une erreur immédiate peut arriver AVANT
    // que le drapeau soit posé — le message doit sortir dans tous les cas.
    listen("ghost-video-ended", (e) => {
        const p = e.payload || {};
        nativeShareEpoch += 1; // invalide aussi une init encore en vol (état fantôme)
        log("⚠️ Partage natif interrompu : " + (p.reason || "erreur d'encodage"));
        if (S.localScreenNative)
            stopScreen();
    });
    // Le flux vidéo natif d'un pair s'est terminé (arrêt, erreur, connexion perdue) :
    // fermer sa vignette au lieu de la laisser figée en ayant l'air vivante.
    listen("ghost-video-rx-end", (e) => {
        const peer = e.payload;
        if (!peer || !nativeRx[peer])
            return;
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
                if (st && Date.now() - st.lastFrameAt >= 1800)
                    bye();
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
        if (S.localScreen || S.localScreenNative)
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
            // VID-1 : si je partage déjà ma cam/écran, pousser la vidéo vers un arrivant
            // tardif (membre du groupe ouvert) en créant sa connexion → offre + mes pistes.
            if ((S.localCam || S.localScreen) && !S.pcs[e.payload]) {
                const g = loadGroups().find((x) => x.id === S.openGroupId);
                if (g && g.members.includes(e.payload))
                    getPc(e.payload);
            }
            // Partage NATIF : pas d'ajout dynamique en v1 — le dire plutôt que laisser
            // l'arrivant devant un écran vide sans explication.
            if (S.localScreenNative && nativeShareMembers && nativeShareMembers.includes(e.payload)) {
                log("ℹ️ " + memberName(e.payload) + " vient de se connecter — relance le partage (⏹️ puis 🖥️) pour l'inclure.");
            }
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
        // Vidéo NATIVE : signal de contrôle pur — surtout NE PAS créer de
        // RTCPeerConnection pour ça (getPc ouvrirait une négociation WebRTC).
        if (msg.nativeVideo) {
            handleNativeSignal(peer, msg.nativeVideo);
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
                if (pc.signalingState === "stable") {
                    retuneSenders(pc);
                    maybeUpgradeScreen(); // unanimité matérielle → 1080p (VID-6)
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
