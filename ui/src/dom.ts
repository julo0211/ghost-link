// Petits utilitaires DOM + formatage, partagés par toute l'UI.

/** Sélecteur court, typé. Par défaut HTMLElement ; précise le type pour les
 *  éléments spécifiques : `$<HTMLInputElement>("#x").value`. On considère que
 *  l'élément existe (tous les id ciblés sont dans index.html). */
export function $<T extends Element = HTMLElement>(s: string): T {
  return document.querySelector(s) as unknown as T;
}

/** Ajoute une ligne au journal de l'app. */
export function log(m: string): void {
  const d = document.createElement("div");
  d.textContent = "• " + m;
  const box = document.getElementById("log");
  if (box) box.prepend(d);
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
