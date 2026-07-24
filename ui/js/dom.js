// Petits utilitaires DOM + formatage, partagés par toute l'UI.
/** Sélecteur court, typé. Par défaut HTMLElement ; précise le type pour les
 *  éléments spécifiques : `$<HTMLInputElement>("#x").value`. On considère que
 *  l'élément existe (tous les id ciblés sont dans index.html). */
export function $(s) {
    return document.querySelector(s);
}
/** Ajoute une ligne au journal de l'app. */
export function log(m) {
    const d = document.createElement("div");
    d.textContent = "• " + m;
    const box = document.getElementById("log");
    if (box)
        box.prepend(d);
}
/** Formate un nombre d'octets en o/Ko/Mo/Go/To. */
export function fmt(b) {
    let n = Number(b) || 0;
    if (n < 1024)
        return n.toFixed(0) + " o";
    const u = ["Ko", "Mo", "Go", "To"];
    let i = -1;
    do {
        n /= 1024;
        i++;
    } while (n >= 1024 && i < u.length - 1);
    return n.toFixed(1) + " " + u[i];
}
/** Formate une durée en secondes (ETA lisible). */
export function etaStr(sec) {
    if (!isFinite(sec) || sec <= 0)
        return "—";
    sec = Math.round(sec);
    if (sec < 60)
        return sec + " s";
    const m = Math.floor(sec / 60), s = sec % 60;
    if (m < 60)
        return m + " min " + (s < 10 ? "0" : "") + s + " s";
    const h = Math.floor(m / 60);
    return h + " h " + (m % 60 < 10 ? "0" : "") + (m % 60) + " min";
}
/** Nom de fichier depuis un chemin (Windows ou Unix). */
export function baseName(p) {
    return String(p).split(/[\\/]/).pop() || "";
}
/** Tronque un code/identifiant long pour l'affichage. */
export function shortId(id) {
    id = String(id);
    return id.length > 14 ? id.slice(0, 14) + "…" : id;
}
// ----- Visionneuse d'image plein écran (overlay #imgViewWrap défini dans index.html) -----
// Pourquoi un overlay et pas une fenêtre : WebView2 neutralise window.open, et Chromium
// interdit une navigation top-level vers une data: URI — les deux sources d'images du chat
// (data: en réception, blob: à l'envoi) seraient donc impossibles à ouvrir « à côté ».
let viewerWired = false;
function wireImgViewer() {
    if (viewerWired)
        return; // une seule fois : sinon les listeners s'empilent à chaque image
    viewerWired = true;
    const wrap = document.getElementById("imgViewWrap");
    if (!wrap)
        return;
    // Fermer au clic sur le FOND uniquement (un clic sur l'image ne doit pas refermer).
    wrap.addEventListener("click", (e) => {
        if (e.target === wrap)
            closeImgViewer();
    });
    document.getElementById("imgViewClose")?.addEventListener("click", (e) => {
        e.stopPropagation();
        closeImgViewer();
    });
    // Échap : on n'agit QUE si la visionneuse est ouverte, pour ne rien préempter.
    document.addEventListener("keydown", (e) => {
        if (e.key !== "Escape" || wrap.classList.contains("hidden"))
            return;
        e.preventDefault();
        closeImgViewer();
    });
}
function closeImgViewer() {
    document.getElementById("imgViewWrap")?.classList.add("hidden");
    const big = document.getElementById("imgViewImg");
    // Détacher le repli AVANT de vider la source : retirer `src` fait passer l'image à
    // l'état « broken » et déclenche un événement `error`. Sans ce nettoyage, le repli
    // canvas se relancerait à CHAQUE fermeture — ré-encodage PNG pleine résolution sur le
    // thread UI (gel visible sur une grande photo) et rétention mémoire à l'opposé du but.
    if (big)
        big.onerror = null;
    // removeAttribute et non src="" : une chaîne vide relancerait une requête vers l'URL du
    // document. Ça stoppe aussi le décodage d'un GIF animé resté en fond.
    big?.removeAttribute("src");
}
/** Ouvre une image du chat en plein écran. `thumb` (la vignette déjà décodée) sert de
 *  repli si la source est un blob: déjà révoqué : on la recopie via un canvas. */
export function openImgViewer(src, thumb) {
    wireImgViewer();
    const wrap = document.getElementById("imgViewWrap");
    const big = document.getElementById("imgViewImg");
    if (!wrap || !big)
        return;
    // Une vignette vidéo en plein écran NATIF passerait au-dessus de l'overlay : en sortir.
    if (document.fullscreenElement)
        void document.exitFullscreen().catch(() => { });
    big.onerror = () => {
        big.onerror = null;
        if (!thumb || !thumb.naturalWidth)
            return;
        const c = document.createElement("canvas");
        c.width = thumb.naturalWidth;
        c.height = thumb.naturalHeight;
        c.getContext("2d")?.drawImage(thumb, 0, 0);
        try {
            big.src = c.toDataURL("image/png");
        }
        catch {
            /* canvas « tainted » (ne devrait pas arriver : même origine) */
        }
    };
    big.src = src;
    wrap.classList.remove("hidden");
    document.getElementById("imgViewClose")?.focus();
}
// Registre des blob: affichés, PAR conteneur de chat. On ne peut pas les révoquer dès
// l'onload : tant qu'une bulle est à l'écran, la visionneuse plein écran recharge cette
// MÊME url au clic (elle afficherait une image cassée). On les révoque donc au moment où
// le conteneur est vidé — cf. clearImgBlobs. Les images reçues sont des data: (rien à
// révoquer) ; seules celles que l'on envoie soi-même créent un blob.
const imgBlobs = new WeakMap();
/** Libère les blob: des images d'un conteneur de chat. À appeler JUSTE AVANT de le vider. */
export function clearImgBlobs(box) {
    const list = imgBlobs.get(box);
    if (!list)
        return;
    while (list.length)
        URL.revokeObjectURL(list.pop());
}
/** Ajoute une bulle image dans un conteneur de chat (data-URI ou blob-URL).
 *  Mirroir de la structure `.msg`/`.me`/`.them` des bulles texte (addMsg /
 *  addGroupMsgDom), avec un <img> au lieu d'un noeud texte. */
export function addImgBubble(box, src, who, author) {
    const m = document.createElement("div");
    m.className = "msg " + (who === "me" ? "me" : "them");
    if (who !== "me" && author && author.trim()) {
        const au = document.createElement("div");
        au.style.cssText = "font-size:11px;font-weight:700;opacity:.8;margin-bottom:2px";
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
    box.scrollTop = box.scrollHeight;
}
