// Petits utilitaires DOM + formatage, partagés par toute l'UI.
// `$` reste volontairement permissif (retour souple) pour ne pas alourdir
// l'accès au DOM ; la valeur du typage est sur la frontière Tauri (tauri.ts)
// et sur les structures de données de l'app (main.ts).
/** Sélecteur court. Retour souple pour un accès DOM sans friction. */
// eslint-disable-next-line @typescript-eslint/no-explicit-any
export const $ = (s) => document.querySelector(s);
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
