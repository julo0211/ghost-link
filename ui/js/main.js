// ghost link — point d'entrée de l'UI. Orchestre les modules de domaine :
// identité + réglages + mises à jour + thème ici, le reste délégué aux init*().
import { invoke, listen } from "./tauri.js";
import { $, log } from "./dom.js";
import { S, loadGroups, friendsOnly, pushFriendsToBackend } from "./state.js";
import { showTab, initSession } from "./session.js";
import { initCall } from "./call.js";
import { initFriends, renderFriends, loadFingerprints, showFp, refreshPresence } from "./friends.js";
import { initTransfer } from "./transfer.js";
import { initGroups, renderGroups, renderGroupFriends } from "./groups.js";
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
const UI_BUILD = "0.35.1";
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
setTimeout(() => checkUpdate(true), 5000);
setTimeout(refreshPresence, 3000);
setInterval(refreshPresence, 60000); // BUG-4 : 30 s → 60 s (moins de tempête de connexions)
log("Prêt. Affiche ton code, ajoute des amis, et ouvre une session.");
initTheme();
