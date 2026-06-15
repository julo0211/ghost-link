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
function initIdentity(): void {
  // Mon code : caché par défaut, révélé/masqué via le bouton.
  $("#btnId").onclick = async () => {
    if ($("#myId").value) {
      $("#myId").value = "";
      $("#fpBox").classList.add("hidden");
      $("#btnId").textContent = "✨ Afficher mon code";
      return;
    }
    try {
      if (!S.myCode) S.myCode = await invoke("perm_code");
      $("#myId").value = S.myCode;
      showFp(S.myCode);
      $("#btnId").textContent = "🙈 Masquer mon code";
      log("Code ghost affiché.");
    } catch (e) {
      log("Erreur code : " + e);
    }
  };
  $("#btnCopyId").onclick = () => {
    if (S.myCode) {
      navigator.clipboard.writeText(S.myCode);
      log("Code permanent copié ✅");
    } else {
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
    } catch (e) {
      log("Rotation : " + e);
    } finally {
      $("#btnRotateEph").disabled = false;
    }
  };
}

// ===== Mises à jour automatiques =====
async function checkUpdate(silent: boolean): Promise<void> {
  if (!silent) $("#updateStatus").textContent = "Recherche…";
  try {
    const v = await invoke("check_update");
    if (v) {
      $("#updateStatus").textContent = "🎉 Nouvelle version disponible : " + v;
      $("#updateBox").classList.remove("hidden");
      log("Mise à jour disponible : " + v);
    } else if (!silent) {
      $("#updateStatus").textContent = "Tu es à jour ✅";
    }
  } catch (e) {
    if (!silent) $("#updateStatus").textContent = "Vérification impossible : " + e;
  }
}
function initUpdates(): void {
  $("#btnCheckUpdate").onclick = () => checkUpdate(false);
  $("#btnInstallUpdate").onclick = async () => {
    $("#btnInstallUpdate").disabled = true;
    S.dlBytes = 0;
    $("#updateStatus").textContent = "Téléchargement…";
    try {
      await invoke("install_update");
    } catch (e) {
      $("#updateStatus").textContent = "Échec : " + e;
      $("#btnInstallUpdate").disabled = false;
    }
  };
  listen("update-progress", (e) => {
    const { chunk, total } = e.payload || ({} as { chunk?: number; total?: number });
    S.dlBytes += chunk || 0;
    if (total) {
      $("#updateBar").style.width = Math.round((S.dlBytes / total) * 100) + "%";
    }
  });
}

// ===== Réglages (engrenage ⚙️) =====
function initSettings(): void {
  $("#btnSettings").onclick = () => $("#settingsCard").classList.toggle("hidden");
  $("#btnCloseSettings").onclick = () => $("#settingsCard").classList.add("hidden");
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
    } catch (e) {
      log("Dossier : " + e);
    }
  };
  $("#setOnlyFriends").onchange = () => {
    const on = $("#setOnlyFriends").checked;
    localStorage.setItem("ghostlink_onlyfriends", on ? "1" : "0");
    pushFriendsToBackend();
    invoke("set_only_friends", { on }).catch(() => {});
    log(on ? "Mode « amis uniquement » activé." : "Mode « amis uniquement » désactivé.");
  };
}

// ===== Onglets =====
function initTabs(): void {
  document.querySelectorAll<HTMLElement>(".tab").forEach(
    (b) =>
      (b.onclick = () => {
        const t = b.getAttribute("data-tab") || "";
        showTab(t);
        if (t === "groupes") {
          renderGroupFriends();
          renderGroups();
        }
      }),
  );
}

// ===== Thème jour / nuit =====
function initTheme(): void {
  const KEY = "ghostlink_theme";
  const mq = window.matchMedia ? window.matchMedia("(prefers-color-scheme: dark)") : null;
  const stored = (): string | null => localStorage.getItem(KEY);
  const eff = (): string => {
    const s = stored();
    return s ? s : mq && mq.matches ? "dark" : "light";
  };
  const apply = (): void => {
    const e = eff();
    document.documentElement.setAttribute("data-theme", e);
    const b = document.getElementById("themeToggle");
    if (b) b.textContent = e === "dark" ? "🌙" : "☀️";
  };
  const tb = document.getElementById("themeToggle");
  if (tb)
    tb.onclick = () => {
      localStorage.setItem(KEY, eff() === "dark" ? "light" : "dark");
      apply();
    };
  if (mq && mq.addEventListener)
    mq.addEventListener("change", () => {
      if (!stored()) apply();
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

showTab("transfert");
renderFriends();
loadFingerprints();
invoke("perm_code")
  .then((code) => {
    S.myCode = code;
  })
  .catch(() => {});
invoke("eph_code")
  .then((code) => {
    $("#ephId").value = code;
  })
  .catch(() => {});
$("#setName").value = (localStorage.getItem("ghostlink_name") || "").trim();
// Au démarrage : reconnecter le maillage des groupes enregistrés, mais ÉTALÉ dans le temps
// (BUG-4 : éviter la tempête de connexions simultanées qui ralentissait tout au lancement).
setTimeout(
  () =>
    loadGroups().forEach((g, i) =>
      setTimeout(() => invoke("open_group", { members: friendsOnly(g.members) }).catch(() => {}), i * 500),
    ),
  800,
);
invoke("app_version")
  .then((v) => {
    $("#appVer").textContent = v;
  })
  .catch(() => {});
invoke("get_download_dir")
  .then((d) => {
    $("#setDir").value = d;
  })
  .catch(() => {});
(function () {
  const on = localStorage.getItem("ghostlink_onlyfriends") === "1";
  $("#setOnlyFriends").checked = on;
  pushFriendsToBackend();
  invoke("set_only_friends", { on }).catch(() => {});
})();
setTimeout(() => checkUpdate(true), 5000);
setTimeout(refreshPresence, 3000);
setInterval(refreshPresence, 60000); // BUG-4 : 30 s → 60 s (moins de tempête de connexions)
log("Prêt. Affiche ton code, ajoute des amis, et ouvre une session.");

initTheme();
