// Frontière typée avec le backend Rust (Tauri, mode withGlobalTauri).
// Tout passe par `invoke` (commandes) et `listen` (événements) ci-dessous :
// les noms de commandes/événements ET la forme de leurs données sont vérifiés
// à la compilation. Une faute de frappe = erreur de build.
/** Appelle une commande Rust. Le nom et la forme des arguments sont typés. */
export function invoke(...[cmd, args]) {
    return window.__TAURI__.core.invoke(cmd, args);
}
/** Écoute un événement Rust. Le payload est typé selon l'événement. */
export function listen(event, handler) {
    void window.__TAURI__.event.listen(event, handler);
}
