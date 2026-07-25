// ghost link — point d'entrée de l'UI. Orchestre les modules de domaine :
// identité + réglages + mises à jour + thème ici, le reste délégué aux init*().
import { invoke, listen } from "./tauri.js";
import { $, log, playPing, unlockAudio } from "./dom.js";
import { S, loadGroups, friendsOnly, pushFriendsToBackend, resetGains } from "./state.js";
import { showTab, initSession, paintConvoUnread } from "./session.js";
import { initCall } from "./call.js";
import { initFriends, renderFriends, loadFingerprints, showFp, refreshPresence } from "./friends.js";
import { initTransfer } from "./transfer.js";
import { initGroups, renderGroups, renderGroupFriends, clearUnreadIfWatching } from "./groups.js";
// ===== Identité (code permanent / éphémère) =====
function initIdentity() {
    // Mon code : caché par défaut, révélé/masqué via le bouton.
    $("#btnId").onclick = async () => {
        if ($("#myId").value) {
            $("#myId").value = "";
            $("#fpBox").classList.add("hidden");
            $("#btnId").textContent = "✨ Afficher mon code";
            return;
        }
        try {
            if (!S.myCode)
                S.myCode = await invoke("perm_code");
            $("#myId").value = S.myCode;
            showFp(S.myCode);
            $("#btnId").textContent = "🙈 Masquer mon code";
            log("Code ghost affiché.");
        }
        catch (e) {
            log("Erreur code : " + e);
        }
    };
    $("#btnCopyId").onclick = () => {
        if (S.myCode) {
            navigator.clipboard.writeText(S.myCode);
            log("Code permanent copié ✅");
        }
        else {
            log("Code indisponible.");
        }
    };
    $("#btnCopyEph").onclick = () => {
        const v = $("#ephId").value;
        if (v) {
            navigator.clipboard.writeText(v);
            log("Code éphémère copié ✅");
        }
    };
    $("#btnRotateEph").onclick = async () => {
        $("#btnRotateEph").disabled = true;
        try {
            const c = await invoke("rotate_eph_code");
            $("#ephId").value = c;
            log("🔄 Nouveau code éphémère — l'ancien ne marche plus.");
        }
        catch (e) {
            log("Rotation : " + e);
        }
        finally {
            $("#btnRotateEph").disabled = false;
        }
    };
}
// ===== Mises à jour automatiques =====
async function checkUpdate(silent) {
    if (!silent)
        $("#updateStatus").textContent = "Recherche…";
    try {
        const v = await invoke("check_update");
        if (v) {
            $("#updateStatus").textContent = "🎉 Nouvelle version disponible : " + v;
            $("#updateBox").classList.remove("hidden");
            log("Mise à jour disponible : " + v);
        }
        else if (!silent) {
            $("#updateStatus").textContent = "Tu es à jour ✅";
        }
    }
    catch (e) {
        if (!silent)
            $("#updateStatus").textContent = "Vérification impossible : " + e;
    }
}
function initUpdates() {
    $("#btnCheckUpdate").onclick = () => checkUpdate(false);
    $("#btnInstallUpdate").onclick = async () => {
        $("#btnInstallUpdate").disabled = true;
        S.dlBytes = 0;
        $("#updateStatus").textContent = "Téléchargement…";
        try {
            await invoke("install_update");
        }
        catch (e) {
            $("#updateStatus").textContent = "Échec : " + e;
            $("#btnInstallUpdate").disabled = false;
        }
    };
    listen("update-progress", (e) => {
        const { chunk, total } = e.payload || {};
        S.dlBytes += chunk || 0;
        if (total) {
            $("#updateBar").style.width = Math.round((S.dlBytes / total) * 100) + "%";
        }
    });
}
// ===== Réglages (engrenage ⚙️) =====
function initSettings() {
    $("#btnSettings").onclick = () => $("#settingsWrap").classList.toggle("hidden");
    $("#btnCloseSettings").onclick = () => $("#settingsWrap").classList.add("hidden");
    $("#btnSaveName").onclick = () => {
        localStorage.setItem("ghostlink_name", $("#setName").value.trim());
        log("Nom d'affichage enregistré.");
    };
    $("#btnSaveDir").onclick = async () => {
        try {
            await invoke("set_download_dir", { path: $("#setDir").value.trim() });
            const d = await invoke("get_download_dir");
            $("#setDir").value = d;
            log("Dossier de réception : " + d);
        }
        catch (e) {
            log("Dossier : " + e);
        }
    };
    $("#setOnlyFriends").onchange = () => {
        const on = $("#setOnlyFriends").checked;
        localStorage.setItem("ghostlink_onlyfriends", on ? "1" : "0");
        pushFriendsToBackend();
        invoke("set_only_friends", { on }).catch(() => { });
        log(on ? "Mode « amis uniquement » activé." : "Mode « amis uniquement » désactivé.");
    };
    // Vidéo : serveur STUN/TURN (vide = LAN uniquement, max vie privée).
    $("#setIce").value = localStorage.getItem("ghostlink_ice") ?? "stun:stun.l.google.com:19302";
    $("#btnSaveIce").onclick = () => {
        localStorage.setItem("ghostlink_ice", $("#setIce").value.trim());
        log("Serveur vidéo enregistré (appliqué au prochain appel vidéo).");
    };
    // Partage d'écran NATIF (expérimental) : appliqué au PROCHAIN partage.
    $("#setNativeVideo").checked = localStorage.getItem("ghostlink_native_video") === "1";
    $("#setNativeVideo").onchange = () => {
        const on = $("#setNativeVideo").checked;
        localStorage.setItem("ghostlink_native_video", on ? "1" : "0");
        log(on
            ? "🧪 Partage d'écran natif activé (appliqué au prochain partage) — sans WebRTC/STUN."
            : "Partage d'écran natif désactivé — retour au partage WebRTC.");
    };
    // Son de notification. ACTIVÉ par défaut (clé absente = activé) ; "0" = désactivé.
    // Cocher joue un bip de confirmation — c'est aussi le seul moyen simple, pour
    // l'utilisateur, de vérifier que le son sort vraiment sur sa machine.
    $("#setSound").checked = localStorage.getItem("ghostlink_sound") !== "0";
    $("#setSound").onchange = () => {
        const on = $("#setSound").checked;
        localStorage.setItem("ghostlink_sound", on ? "1" : "0");
        if (on)
            playPing("msg");
        log(on ? "🔔 Son de notification activé." : "Son de notification désactivé.");
    };
    // Soupape des volumes persistés : sans issue de secours VISIBLE, un réglage invisible et
    // durable est un piège. Portée complète — disque, mémoire, backend si un appel tourne, et
    // les curseurs affichés : dans un lot dont le sujet est justement un désync
    // curseur/backend, une réinitialisation partielle recréerait le bug volontairement.
    $("#btnResetGains").onclick = () => {
        const peers = new Set([...Object.keys(S.groupGains), ...Object.keys(S.screenGains)]);
        resetGains();
        if (S.inGroupCall) {
            peers.forEach((c) => {
                invoke("group_call_volume", { peer: c, vol: 1 }).catch(() => { });
                invoke("screen_audio_gain", { peer: c, vol: 1 }).catch(() => { });
            });
        }
        document.querySelectorAll("input[data-vol]").forEach((r) => (r.value = "100"));
        renderGroups();
        log("Volumes par pair réinitialisés.");
    };
    const storedStreams = localStorage.getItem("ghostlink_streams") ?? "4";
    $("#setStreams").value = storedStreams;
    // #14 : le backend repart toujours de NSTREAMS=4 au démarrage — réappliquer la valeur
    // persistée pour que le réglage soit effectivement en vigueur, pas seulement affiché.
    if (Number(storedStreams) !== 4) {
        invoke("set_streams", { n: Number(storedStreams) }).catch(() => { });
    }
    $("#setStreams").onchange = () => {
        const v = $("#setStreams").value;
        localStorage.setItem("ghostlink_streams", v);
        invoke("set_streams", { n: Number(v) }).catch(() => { });
        log("Vitesse de transfert : " + v + " flux parallèles.");
    };
    // Thème visuel (data-skin) : 4 identités, défaut « spectral ». Appliqué sur <html>
    // dès le chargement par un script inline (anti-flash) ; ici on synchronise le select.
    const skinSel = $("#setSkin");
    skinSel.value = localStorage.getItem("ghostlink_skin") ?? "spectral";
    skinSel.onchange = () => {
        const v = skinSel.value;
        localStorage.setItem("ghostlink_skin", v);
        document.documentElement.setAttribute("data-skin", v);
        log("Thème visuel : " + v + ".");
    };
}
// ===== Onglets =====
function initTabs() {
    const nc = document.getElementById("btnNewConn");
    if (nc)
        nc.onclick = () => showTab("connect");
    const ng = document.getElementById("btnNewGroup");
    if (ng)
        ng.onclick = () => {
            renderGroupFriends();
            renderGroups();
            showTab("newgroup");
        };
}
// ===== Thème jour / nuit =====
function initTheme() {
    const KEY = "ghostlink_theme";
    const mq = window.matchMedia ? window.matchMedia("(prefers-color-scheme: dark)") : null;
    const stored = () => localStorage.getItem(KEY);
    const eff = () => {
        // Défaut = sombre (= maquettes « liquid glass »). Un choix utilisateur enregistré prime.
        return stored() || "dark";
    };
    const apply = () => {
        const e = eff();
        document.documentElement.setAttribute("data-theme", e);
        const b = document.getElementById("themeToggle");
        if (b)
            b.textContent = e === "dark" ? "🌙" : "☀️";
    };
    const tb = document.getElementById("themeToggle");
    if (tb)
        tb.onclick = () => {
            localStorage.setItem(KEY, eff() === "dark" ? "light" : "dark");
            apply();
        };
    if (mq && mq.addEventListener)
        mq.addEventListener("change", () => {
            if (!stored())
                apply();
        });
    apply();
}
// ===== Démarrage =====
initIdentity();
initUpdates();
initSettings();
initTabs();
initSession();
initCall();
initFriends();
initTransfer();
initGroups();
showTab("connect");
renderFriends();
renderGroups();
loadFingerprints();
invoke("perm_code")
    .then((code) => {
    S.myCode = code;
})
    .catch(() => { });
invoke("eph_code")
    .then((code) => {
    $("#ephId").value = code;
})
    .catch(() => { });
$("#setName").value = (localStorage.getItem("ghostlink_name") || "").trim();
// Au démarrage : reconnecter le maillage des groupes enregistrés, mais ÉTALÉ dans le temps
// (BUG-4 : éviter la tempête de connexions simultanées qui ralentissait tout au lancement).
setTimeout(() => loadGroups().forEach((g, i) => setTimeout(() => invoke("open_group", { members: friendsOnly(g.members) }).catch(() => { }), i * 500)), 800);
// Tampon de build du FRONTEND. Si « UI » diverge de la version Rust (app_version),
// c'est que la WebView sert un ancien frontend en cache (et non le code compilé).
const UI_BUILD = "0.37.3";
invoke("app_version")
    .then((v) => {
    $("#appVer").textContent = v + " · UI " + UI_BUILD;
})
    .catch(() => {
    $("#appVer").textContent = "? · UI " + UI_BUILD;
});
invoke("get_download_dir")
    .then((d) => {
    $("#setDir").value = d;
})
    .catch(() => { });
(function () {
    const on = localStorage.getItem("ghostlink_onlyfriends") === "1";
    $("#setOnlyFriends").checked = on;
    pushFriendsToBackend();
    invoke("set_only_friends", { on }).catch(() => { });
})();
// Chromium peut créer un AudioContext « suspended » tant qu'aucun geste utilisateur n'a eu
// lieu, et l'app ne joue par ailleurs AUCUN son via la WebView (tout passe par cpal côté
// Rust) : rien ne le déverrouille autrement. On le fait au premier clic réel, une fois.
document.addEventListener("click", () => unlockAudio(), { once: true, capture: true });
// Focus de la fenêtre : pilote les pastilles (un message reçu fenêtre au premier plan ET
// conversation ouverte ne doit rien marquer). Reprendre le focus est AUSSI un point
// d'effacement : sans lui, un message arrivé fenêtre floue sur le groupe DÉJÀ ouvert
// laisserait une pastille que l'utilisateur ne pourrait effacer qu'en naviguant ailleurs
// puis en revenant — alors qu'il est justement en train de le lire.
function setFocused(on) {
    if (S.focused === on)
        return;
    S.focused = on;
    if (!on)
        return;
    // Reprendre le focus n'efface que la conversation RÉELLEMENT affichée : un groupe resté
    // chargé derrière un autre onglet ne doit pas voir sa pastille disparaître juste parce
    // qu'on revient sur la fenêtre (même raison que le bug corrigé dans pushGroupMsg).
    if (clearUnreadIfWatching(S.openGroupId))
        renderGroups();
    const sess = document.querySelector('[data-view="session"]');
    if (sess && !sess.classList.contains("view-hidden") && S.unread1to1) {
        S.unread1to1 = 0;
        paintConvoUnread();
    }
}
// TROIS sources pour le même état, délibérément. Les deux premières sont du DOM Chromium
// pur, donc garanties dans la WebView ; la troisième est l'événement Tauri, que je n'ai pas
// pu vérifier empiriquement — il vient de `emit_to_window` (cible EventTarget::Window /
// WebviewWindow), là où les événements de glisser-déposer, eux, passent explicitement par
// `emit_to(EventTarget::labeled(...))`. Plutôt que de faire reposer les pastilles sur cette
// asymétrie non élucidée, on prend le signal DOM comme source principale.
window.addEventListener("focus", () => setFocused(true));
window.addEventListener("blur", () => setFocused(false));
document.addEventListener("visibilitychange", () => setFocused(document.visibilityState === "visible"));
listen("tauri://focus", () => setFocused(true));
listen("tauri://blur", () => setFocused(false));
S.focused = document.hasFocus(); // état initial réel, et non un `true` optimiste
setTimeout(() => checkUpdate(true), 5000);
setTimeout(refreshPresence, 3000);
setInterval(refreshPresence, 60000); // BUG-4 : 30 s → 60 s (moins de tempête de connexions)
log("Prêt. Affiche ton code, ajoute des amis, et ouvre une session.");
initTheme();
