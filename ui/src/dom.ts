// Petits utilitaires DOM + formatage, partagés par toute l'UI.

/** Sélecteur court, typé. Par défaut HTMLElement ; précise le type pour les
 *  éléments spécifiques : `$<HTMLInputElement>("#x").value`. On considère que
 *  l'élément existe (tous les id ciblés sont dans index.html). */
export function $<T extends Element = HTMLElement>(s: string): T {
  return document.querySelector(s) as unknown as T;
}

/** Ajoute une ligne au journal de l'app. */
/** Lignes conservées dans le Journal. Sans plafond il grandit indéfiniment : plusieurs
 *  événements sont pilotés par le RÉSEAU (demandes d'ami, offres de fichier, avertissements
 *  de métadonnées, exclusions), donc un pair bavard fait enfler le DOM pour toute la
 *  session. 500 lignes couvrent très largement un diagnostic. */
const MAX_LOG_LIGNES = 500;

export function log(m: string): void {
  const d = document.createElement("div");
  d.textContent = "• " + m;
  const box = document.getElementById("log");
  if (!box) return;
  box.prepend(d);
  // prepend() : les plus anciennes sont EN BAS, on coupe donc par la fin.
  while (box.childElementCount > MAX_LOG_LIGNES) box.lastElementChild?.remove();
}

/** Formate un nombre d'octets en o/Ko/Mo/Go/To. */
export function fmt(b: number | string): string {
  let n = Number(b) || 0;
  if (n < 1024) return n.toFixed(0) + " o";
  const u = ["Ko", "Mo", "Go", "To"];
  let i = -1;
  do {
    n /= 1024;
    i++;
  } while (n >= 1024 && i < u.length - 1);
  return n.toFixed(1) + " " + u[i];
}

/** Formate une durée en secondes (ETA lisible). */
export function etaStr(sec: number): string {
  if (!isFinite(sec) || sec <= 0) return "—";
  sec = Math.round(sec);
  if (sec < 60) return sec + " s";
  const m = Math.floor(sec / 60),
    s = sec % 60;
  if (m < 60) return m + " min " + (s < 10 ? "0" : "") + s + " s";
  const h = Math.floor(m / 60);
  return h + " h " + (m % 60 < 10 ? "0" : "") + (m % 60) + " min";
}

/** Nom de fichier depuis un chemin (Windows ou Unix). */
export function baseName(p: string): string {
  return String(p).split(/[\\/]/).pop() || "";
}

/** Tronque un code/identifiant long pour l'affichage. */
export function shortId(id: string): string {
  id = String(id);
  return id.length > 14 ? id.slice(0, 14) + "…" : id;
}

/** Borne un libellé d'origine DISTANTE avant affichage. Les noms auto-déclarés arrivent
 *  via read_lp16, borné seulement par son préfixe u16 (64 Kio) : aucun plafond sémantique,
 *  aucune élision. Sans ça un pseudo de 60 000 caractères casse la mise en page. */
export function clampLabel(s?: string): string {
  const v = String(s ?? "").trim();
  return v.length > 40 ? v.slice(0, 40) + "…" : v;
}

// ----- Son de notification -----
// Oscillateur WebAudio et NON un fichier : le CSP (tauri.conf.json) a
// `media-src 'self' blob:` SANS `data:`, donc une data-URI passée à new Audio() serait
// refusée. Un oscillateur ne demande ni asset embarqué ni dérogation CSP.
//
// ⚠️ L'AUTOPLAY N'EST PAS ACQUIS DANS CETTE APP. Elle ne joue AUCUN son via la WebView :
// getUserMedia est appelé avec audio:false, getDisplayMedia en vidéo seule (c'est
// l'anti-écho, cf. groups.ts), et tout le son réel sort de cpal dans le processus hôte
// (audio.rs). Les <video> autoplay un flux SILENCIEUX — cela ne prouve rien sur
// l'autorisation de PRODUIRE du son. Chromium soumet AudioContext à une politique
// distincte de <video autoplay> : un contexte créé au chargement du module peut démarrer
// `suspended` et le rester indéfiniment. D'où création paresseuse + resume() déclenché par
// le premier geste utilisateur réel (unlockAudio, câblé sur le premier clic dans main.ts).
type PingKind = "msg" | "req" | "call";
let actx: AudioContext | null = null;
let audioUnlocked = false;
const lastPing: Record<string, number> = {};

/** À appeler depuis le premier geste utilisateur réel (n'importe quel clic). */
export function unlockAudio(): void {
  audioUnlocked = true;
  if (actx && actx.state === "suspended") void actx.resume().catch(() => {});
}
function ctx(): AudioContext | null {
  if (!audioUnlocked) return null; // avant tout geste : no-op silencieux assumé
  if (!actx) {
    const C =
      window.AudioContext ||
      (window as unknown as { webkitAudioContext?: typeof AudioContext }).webkitAudioContext;
    if (!C) return null;
    try {
      actx = new C();
    } catch {
      return null;
    }
  }
  if (actx.state === "suspended") void actx.resume().catch(() => {});
  return actx;
}
/** Joue un bip court. Limité à 1 par 2 s PAR TIMBRE — et non globalement : un limiteur
 *  partagé ferait taire un appel entrant arrivé juste après un ping de message, ce qui
 *  contredirait la règle « jamais muet pour un appel entrant ».
 *  Ne JAMAIS brancher sur un événement haute fréquence (ghost-voice-activity ~10 Hz,
 *  ghost-voice-presence 1 Hz × N membres). */
export function playPing(kind: PingKind): void {
  if (localStorage.getItem("ghostlink_sound") === "0") return;
  const now = Date.now();
  if (now - (lastPing[kind] || 0) < 2000) return;
  lastPing[kind] = now;
  const a = ctx();
  if (!a) return;
  const freq = kind === "call" ? 880 : kind === "req" ? 620 : 740;
  const dur = kind === "call" ? 0.22 : 0.12;
  try {
    const o = a.createOscillator();
    const g = a.createGain();
    o.type = "sine";
    o.frequency.value = freq;
    g.gain.setValueAtTime(0, a.currentTime);
    g.gain.linearRampToValueAtTime(0.14, a.currentTime + 0.01);
    g.gain.exponentialRampToValueAtTime(0.0001, a.currentTime + dur);
    o.connect(g);
    g.connect(a.destination);
    o.start();
    o.stop(a.currentTime + dur + 0.02);
  } catch {
    /* pas de son : jamais bloquant */
  }
}

// ----- Visionneuse d'image plein écran (overlay #imgViewWrap défini dans index.html) -----
// Pourquoi un overlay et pas une fenêtre : WebView2 neutralise window.open, et Chromium
// interdit une navigation top-level vers une data: URI — les deux sources d'images du chat
// (data: en réception, blob: à l'envoi) seraient donc impossibles à ouvrir « à côté ».
let viewerWired = false;
function wireImgViewer(): void {
  if (viewerWired) return; // une seule fois : sinon les listeners s'empilent à chaque image
  viewerWired = true;
  const wrap = document.getElementById("imgViewWrap");
  if (!wrap) return;
  // Fermer au clic sur le FOND uniquement (un clic sur l'image ne doit pas refermer).
  wrap.addEventListener("click", (e) => {
    if (e.target === wrap) closeImgViewer();
  });
  document.getElementById("imgViewClose")?.addEventListener("click", (e) => {
    e.stopPropagation();
    closeImgViewer();
  });
  // Échap : on n'agit QUE si la visionneuse est ouverte, pour ne rien préempter.
  document.addEventListener("keydown", (e) => {
    if (e.key !== "Escape" || wrap.classList.contains("hidden")) return;
    e.preventDefault();
    closeImgViewer();
  });
}
function closeImgViewer(): void {
  document.getElementById("imgViewWrap")?.classList.add("hidden");
  const big = document.getElementById("imgViewImg") as HTMLImageElement | null;
  // Détacher le repli AVANT de vider la source : retirer `src` fait passer l'image à
  // l'état « broken » et déclenche un événement `error`. Sans ce nettoyage, le repli
  // canvas se relancerait à CHAQUE fermeture — ré-encodage PNG pleine résolution sur le
  // thread UI (gel visible sur une grande photo) et rétention mémoire à l'opposé du but.
  if (big) big.onerror = null;
  // removeAttribute et non src="" : une chaîne vide relancerait une requête vers l'URL du
  // document. Ça stoppe aussi le décodage d'un GIF animé resté en fond.
  big?.removeAttribute("src");
}
/** Ouvre une image du chat en plein écran. `thumb` (la vignette déjà décodée) sert de
 *  repli si la source est un blob: déjà révoqué : on la recopie via un canvas. */
export function openImgViewer(src: string, thumb?: HTMLImageElement): void {
  wireImgViewer();
  const wrap = document.getElementById("imgViewWrap");
  const big = document.getElementById("imgViewImg") as HTMLImageElement | null;
  if (!wrap || !big) return;
  // Une vignette vidéo en plein écran NATIF passerait au-dessus de l'overlay : en sortir.
  if (document.fullscreenElement) void document.exitFullscreen().catch(() => {});
  big.onerror = () => {
    big.onerror = null;
    if (!thumb || !thumb.naturalWidth) return;
    const c = document.createElement("canvas");
    c.width = thumb.naturalWidth;
    c.height = thumb.naturalHeight;
    c.getContext("2d")?.drawImage(thumb, 0, 0);
    try {
      big.src = c.toDataURL("image/png");
    } catch {
      /* canvas « tainted » (ne devrait pas arriver : même origine) */
    }
  };
  big.src = src;
  wrap.classList.remove("hidden");
  (document.getElementById("imgViewClose") as HTMLButtonElement | null)?.focus();
}

// Registre des blob: affichés, PAR conteneur de chat. On ne peut pas les révoquer dès
// l'onload : tant qu'une bulle est à l'écran, la visionneuse plein écran recharge cette
// MÊME url au clic (elle afficherait une image cassée). On les révoque donc au moment où
// le conteneur est vidé — cf. clearImgBlobs. Les images reçues sont des data: (rien à
// révoquer) ; seules celles que l'on envoie soi-même créent un blob.
const imgBlobs = new WeakMap<HTMLElement, string[]>();
/** Libère les blob: des images d'un conteneur de chat. À appeler JUSTE AVANT de le vider. */
export function clearImgBlobs(box: HTMLElement): void {
  const list = imgBlobs.get(box);
  if (!list) return;
  while (list.length) URL.revokeObjectURL(list.pop() as string);
}

/** Nombre max. de bulles IMAGE conservées dans un conteneur de chat.
 *
 *  Sans plafond, la mémoire du rendu croît sans borne et rien ne la libère : les images
 *  reçues sont des data: (~1,33× les octets du fil, jusqu'à 8 Mio chacun) et Chromium en
 *  garde en plus le bitmap DÉCODÉ, dont la taille dépend des DIMENSIONS et pas du poids —
 *  un PNG de 300 Ko peut déclarer 16000×16000 et coûter ~1 Go. Le tampon texte est déjà
 *  borné à 200 (S.groupMsgs) ; les images n'y sont pas, et le conteneur n'est vidé qu'au
 *  changement de groupe — c'est-à-dire jamais, pour qui reste sur un seul groupe.
 *
 *  Note : élaguer une bulle dont l'image est ouverte en plein écran révoque son blob:.
 *  La visionneuse retombe alors sur son repli canvas (openImgViewer retient la vignette
 *  déjà décodée par clôture) — ne pas supprimer ce repli. */
const MAX_IMG_BUBBLES = 24;

/** Retire les bulles image les plus anciennes au-delà de MAX_IMG_BUBBLES, en libérant
 *  leurs blob:. Ne touche PAS aux bulles texte, qui ont leur propre plafond. */
function trimImgBubbles(box: HTMLElement): void {
  const bulles = box.querySelectorAll<HTMLElement>('[data-img="1"]');
  const trop = bulles.length - MAX_IMG_BUBBLES;
  if (trop <= 0) return;
  const list = imgBlobs.get(box);
  for (let i = 0; i < trop; i++) {
    const vieille = bulles[i];
    const src = vieille.dataset.src || "";
    if (src.startsWith("blob:")) {
      URL.revokeObjectURL(src);
      // Retirer du registre aussi : sinon la liste grossit indéfiniment et clearImgBlobs
      // re-révoquerait des URL mortes (inoffensif, mais c'est une fuite de chaînes).
      if (list) {
        const j = list.indexOf(src);
        if (j >= 0) list.splice(j, 1);
      }
    }
    vieille.remove();
  }
}

/** Bulles TEXTE conservées dans un conteneur de chat. Le tampon de groupe (S.groupMsgs)
 *  est déjà plafonné à 200, mais il ne borne que le REJEU : pendant une session, le DOM
 *  grandit sans limite à chaque message reçu. Le 1-à-1 n'avait aucun plafond du tout. */
const MAX_TEXT_BUBBLES = 300;

/** Élague les bulles TEXTE les plus anciennes. Ignore les bulles image, qui ont leur
 *  propre plafond (elles coûtent bien plus cher et se libèrent différemment). */
export function trimTextBubbles(box: HTMLElement): void {
  const bulles = box.querySelectorAll<HTMLElement>(".msg:not([data-img])");
  const trop = bulles.length - MAX_TEXT_BUBBLES;
  for (let i = 0; i < trop; i++) bulles[i].remove();
}

/** Ajoute une bulle image dans un conteneur de chat (data-URI ou blob-URL).
 *  Mirroir de la structure `.msg`/`.me`/`.them` des bulles texte (addMsg /
 *  addGroupMsgDom), avec un <img> au lieu d'un noeud texte. */
export function addImgBubble(
  box: HTMLElement,
  src: string,
  who: string,
  author?: string,
  from?: string,
  unverified?: boolean,
): void {
  const m = document.createElement("div");
  m.className = "msg " + (who === "me" ? "me" : "them");
  // Marqueur d'élagage porté par un ATTRIBUT DE DONNÉES et non par une classe : le
  // contrat DOM est strict et le CSS cible .msg.me / .msg.them — ajouter une classe
  // risquerait d'accrocher une règle de style existante.
  m.dataset.img = "1";
  m.dataset.src = src;
  if (who !== "me" && author && author.trim()) {
    const au = document.createElement("div");
    au.className = "auth" + (unverified ? " unverified" : "");
    au.style.cssText = "font-size:11px;font-weight:700;opacity:.8;margin-bottom:2px";
    // data-from = code AUTHENTIFIÉ de l'expéditeur : cible du ré-étiquetage en place lors
    // d'un renommage. On ne peut PAS re-rendre le journal pour rafraîchir un libellé —
    // renderGroupMsgs fait clearImgBlobs + innerHTML="" et ne rejoue que le texte, donc
    // toutes les images affichées seraient détruites et leurs blob: révoqués.
    if (from) au.dataset.from = from;
    au.textContent = author.trim();
    m.appendChild(au);
  }
  if (src.startsWith("blob:")) {
    const list = imgBlobs.get(box) ?? [];
    list.push(src);
    imgBlobs.set(box, list);
  }
  const img = document.createElement("img");
  img.src = src;
  img.loading = "lazy";
  img.style.cssText = "max-width:100%;max-height:320px;border-radius:8px;cursor:zoom-in;display:block";
  img.title = "Cliquer pour agrandir";
  img.onclick = () => openImgViewer(src, img);
  m.appendChild(img);
  box.appendChild(m);
  // AVANT le scroll : élaguer après aurait calculé scrollHeight sur une hauteur périmée.
  trimImgBubbles(box);
  box.scrollTop = box.scrollHeight;
}
