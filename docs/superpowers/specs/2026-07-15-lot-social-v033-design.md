# Lot social v0.33.0 — design

Lot de fonctionnalités demandées après le test du partage natif (v0.32.0). Carte
blanche de l'utilisateur ; ce document fige les choix. Architecture cible : mesh P2P
sans serveur, groupes décentralisés (chaque client stocke sa propre liste de membres).

## Ajouts au protocole

- `GKIND_GMEMBERS = 8` (flux de groupe) — sync de roster : `[u16 gid][u16 nom][u32 csv]`.
  Reçu par un membre déjà dans le groupe → **union** des membres (convergence des ajouts).
- `GKIND_KICK = 9` (flux de groupe) — un vote d'exclusion : `[u16 gid][u16 cible][u16 votant]`.
- `CALL_PING = 3` (datagramme, tag après VOICE_TAG=1/SCREEN_TAG=2) — balise « je suis
  dans l'appel », émise ~1 Hz pendant un appel de groupe, indépendante de la parole.

## A. Volume du stream regardé (gain audio d'écran par pair)

Le son d'écran d'un pair est déjà mixé avec un gain (aujourd'hui `set_screen_mute` =
gain 0/1 sur la clé `screen_mix_key(code)`). On généralise :
- `GroupCall::set_screen_gain(code, gain: f32)` + commande `screen_audio_gain(peer, vol)`.
- Curseur 0–200 % sur la vignette du stream. Natif → `screen_audio_gain`. WebRTC →
  `video.volume`. Le bouton 🔇/🔊 reste (raccourci mute = gain 0). État par pair
  `S.screenGains` (session).

## B. Indicateur vocal (en appel + qui parle)

- **En appel** : chaque participant émet `CALL_PING` ~1 Hz vers tous les pairs. Un pair
  est « en appel » si `last_ping` OU `last_voice` < 3 s. Marche même micro coupé.
- **Qui parle** : `receive_group_voice` calcule le pic des trames VOICE_TAG décodées par
  pair (pas SCREEN_TAG) ; le mixeur de capture calcule mon niveau micro local. Au-dessus
  d'un seuil = « parle ».
- État partagé `VoiceActivity: Arc<Mutex<HashMap<code, (level, last_voice, last_ping)>>>`
  + slot « me ». Une tâche émet `ghost-voice-activity` ~10 Hz (map code→{inCall, speaking}).
  `GroupCall::start` reçoit l'`AppHandle` pour l'émission.
- UI : les chips membres portent `data-code` ; le listener bascule une **classe CSS**
  (`speaking`/`incall`) sans re-render. Anneau animé quand ça parle.

## C. Ajouter des membres à un groupe existant

- Bouton « ➕ Membres » dans la vue groupe → liste d'amis (comme à la création) → pour
  chaque nouveau : `send_ginvite` (bannière chez lui) **+** `GKIND_GMEMBERS` aux membres
  déjà présents, qui font l'union de leur roster. Réutilise `addPInv` pour les hors-ligne.
- Limite v1 : convergence best-effort (un hors-ligne rattrape quand quelqu'un rediffuse) ;
  pas de CRDT.

## D. Vote-kick (≥ 60 % des membres en ligne)

- Action « Exclure » sur un chip membre → `GKIND_KICK` (mon vote) à tous les en-ligne.
- Chaque client tallie les votes distincts par (gid, cible). Dénominateur = membres en
  ligne connus localement + moi. Quorum `votes ≥ ⌈0.6 × en_ligne⌉` → retrait local du
  roster + fermeture connexion + **liste noire par groupe** (`ghostlink_kicked`), l'exclu
  ne peut plus se re-dialer. Une ré-invitation lève la liste noire.
- Bannière « Exclure X : n/quorum » pendant le vote. Votes périmés après 5 min.
- Advisory : un client malveillant peut ignorer son propre kick, mais les honnêtes le
  lâchent.

## E. Picker de partage (écran OU fenêtre) + audio de fenêtre

- Clic 🖥️ en mode natif → petite fenêtre-liste (overlay HTML in-app, pas de nouvelle
  fenêtre OS) : **écrans** (déjà énumérés) + **fenêtres** top-level visibles titrées
  (`EnumWindows`). Remplace le déroulant Réglages (qui reste comme défaut/fallback).
- Cible = moniteur (szDevice) ou fenêtre (HWND). `build_capture` : `CreateForWindow` pour
  une fenêtre. `video_share_start` prend `target: {kind, id}`.
- **Audio de fenêtre** : le loopback WASAPI passe en mode **INCLUDE le process de la
  fenêtre** (PID via `GetWindowThreadProcessId`) au lieu de « tout le système sauf nous ».
  `screen_audio_start` prend un PID cible optionnel.
- **Redimensionnement d'une fenêtre partagée** : reconstruction **debouncée** (~700 ms de
  stabilité) de la capture + l'encodeur à la nouvelle taille, keyframe + bit newSession →
  le récepteur reset son décodeur (le canvas s'adapte déjà à `frame.displayWidth/Height`).
  Choisi plutôt que « arrêt + relance » pour l'UX. Le teardown NVENC sans fuite déjà en
  place rend la reconstruction sûre.

## Hors périmètre / déjà fait

- Confirmation sur la croix de suppression de groupe : **existe déjà**
  (`confirm("Supprimer / quitter…")`). Rien à faire.

## Rituel qualité

Expérience scratchpad pour toute brique risquée (audio d'inclusion PID, CreateForWindow),
build + tests, revue multi-agents adversariale du diff, `cargo test`/`clippy` + `npm run
build`, bump 0.33.0, commit/push. Release manuelle (`release.ps1`).
