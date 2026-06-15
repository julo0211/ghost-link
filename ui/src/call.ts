// Vocal 1-à-1 : test micro local + appel duplex (datagrammes iroh) + périphériques audio.

import { invoke, listen } from "./tauri.js";
import { $, log } from "./dom.js";
import { S } from "./state.js";

export function setCallUI(on: boolean): void {
  S.inCall = on;
  const b = $("#btnCall");
  if (b) b.textContent = on ? "📵 Raccrocher" : "📞 Appeler";
  const s = $("#callStatus");
  if (s) s.textContent = on ? "🔊 En appel — parle ! (casque conseillé pour éviter l'écho)" : "";
  const m = $("#btnMute");
  if (m) {
    m.classList.toggle("hidden", !on);
    S.muted = false;
    m.textContent = "🔇 Couper le micro";
  }
}
export function hideCallOffer(): void {
  const b = $("#callOfferBanner");
  if (b) b.classList.add("hidden");
  if (S.callOfferTimer) {
    clearTimeout(S.callOfferTimer);
    S.callOfferTimer = null;
  }
}

function fillSel(sel: HTMLSelectElement, items: string[], chosen: string): void {
  sel.innerHTML = "";
  const def = document.createElement("option");
  def.value = "";
  def.textContent = "(par défaut)";
  sel.appendChild(def);
  for (const n of items) {
    const o = document.createElement("option");
    o.value = n;
    o.textContent = n;
    sel.appendChild(o);
  }
  sel.value = chosen;
}
async function loadAudioDevices(): Promise<void> {
  try {
    const r = await invoke("list_audio_devices");
    const ins = (r && r[0]) || [];
    const outs = (r && r[1]) || [];
    fillSel($("#selMic"), ins, localStorage.getItem("ghostlink_mic") || "");
    fillSel($("#selSpk"), outs, localStorage.getItem("ghostlink_spk") || "");
    invoke("set_audio_input", { name: localStorage.getItem("ghostlink_mic") || null }).catch(() => {});
    invoke("set_audio_output", { name: localStorage.getItem("ghostlink_spk") || null }).catch(() => {});
  } catch {
    /* ignore */
  }
}

export function initCall(): void {
  // Vocal — V1 : test local du micro (boucle micro → haut-parleur)
  $("#btnVoiceTest").onclick = async () => {
    if (!S.voiceTesting) {
      $("#btnVoiceTest").disabled = true;
      $("#voiceStatus").textContent = "démarrage…";
      try {
        await invoke("voice_test_start");
        S.voiceTesting = true;
        $("#btnVoiceTest").textContent = "⏹ Arrêter";
        $("#voiceStatus").textContent =
          "tu devrais t'entendre (petit délai) — mets un casque pour éviter l'écho.";
        log("🎙️ Test micro démarré.");
      } catch (e) {
        $("#voiceStatus").textContent = "erreur : " + e;
        log("Test micro : " + e);
      } finally {
        $("#btnVoiceTest").disabled = false;
      }
    } else {
      try {
        await invoke("voice_test_stop");
      } catch {
        /* ignore */
      }
      S.voiceTesting = false;
      $("#btnVoiceTest").textContent = "🎙️ Tester le micro";
      $("#voiceStatus").textContent = "";
      log("Test micro arrêté.");
    }
  };

  // Appel vocal (V3) — duplex via les datagrammes iroh
  $("#btnMute").onclick = () => {
    S.muted = !S.muted;
    invoke("call_set_mute", { on: S.muted }).catch(() => {});
    $("#btnMute").textContent = S.muted ? "🎙️ Réactiver le micro" : "🔇 Couper le micro";
    $("#callStatus").textContent = S.muted ? "🔇 Micro coupé" : "🔊 En appel — parle !";
  };
  $("#btnCall").onclick = async () => {
    if (!S.inCall) {
      $("#btnCall").disabled = true;
      try {
        await invoke("call_start", { signal: true });
        setCallUI(true);
        log("📞 Appel démarré.");
      } catch (e) {
        log("Appel : " + e);
        $("#callStatus").textContent = "erreur : " + e;
      } finally {
        $("#btnCall").disabled = false;
      }
    } else {
      try {
        await invoke("call_stop", { signal: true });
      } catch {
        /* ignore */
      }
      setCallUI(false);
      log("Appel terminé.");
    }
  };
  // Appel entrant : NE PAS ouvrir le micro automatiquement — demander le consentement.
  listen("ghost-call-start", () => {
    if (S.inCall) return;
    $("#callOfferBanner").classList.remove("hidden");
    if (S.callOfferTimer) clearTimeout(S.callOfferTimer);
    S.callOfferTimer = setTimeout(() => {
      hideCallOffer();
      invoke("call_stop", { signal: false }).catch(() => {});
    }, 30000);
    log("📞 Appel entrant — accepte pour activer ton micro.");
  });
  $("#btnCallAccept").onclick = async () => {
    hideCallOffer();
    if (S.inCall) return;
    $("#btnCallAccept").disabled = true;
    try {
      await invoke("call_start", { signal: false });
      setCallUI(true);
      log("📞 Appel accepté.");
    } catch (e) {
      log("Appel : " + e);
    } finally {
      $("#btnCallAccept").disabled = false;
    }
  };
  $("#btnCallRefuse").onclick = () => {
    hideCallOffer();
    invoke("call_stop", { signal: true }).catch(() => {}); // prévient l'appelant du refus
    log("Appel refusé.");
  };
  listen("ghost-call-stop", async () => {
    hideCallOffer(); // l'appelant a annulé pendant la sonnerie
    if (!S.inCall) return;
    try {
      await invoke("call_stop", { signal: false });
    } catch {
      /* ignore */
    }
    setCallUI(false);
    log("Le pair a raccroché.");
  });

  // Choix des périphériques audio (micro / haut-parleur)
  $("#selMic").onchange = () => {
    const v = $("#selMic").value;
    localStorage.setItem("ghostlink_mic", v);
    invoke("set_audio_input", { name: v || null }).catch(() => {});
  };
  $("#selSpk").onchange = () => {
    const v = $("#selSpk").value;
    localStorage.setItem("ghostlink_spk", v);
    invoke("set_audio_output", { name: v || null }).catch(() => {});
  };
  loadAudioDevices();
}
