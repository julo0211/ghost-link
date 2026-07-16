# Spec — Médias en chat, statut vocal, qualité de stream, convergence roster, confinement vidéo

- **Date** : 2026-07-16
- **Base** : commit `4faa315` (ghost link v0.33.0)
- **Cible de version** : v0.34.0 (lot de fonctionnalités → bump mineur)
- **Statut** : design validé, en attente de relecture avant plan d'implémentation

## Contexte & objectif

Cinq chantiers demandés sur l'app P2P E2E `ghost link` (Tauri 2 + iroh) : **3 fonctionnalités**
(images/GIF dans le chat, statut vocal séparé, choix de qualité/fps du partage d'écran) et
**2 corrections de bugs** (synchro du roster de groupe pour ajout/kick ; confinement d'un partage
d'écran à un seul groupe). Chaque chantier reste dans le modèle serverless/décentralisé existant :
aucun serveur, messages authentifiés par `from = remote_id`, allocations réseau bornées.

## Vue d'ensemble

| # | Type | Cœur | Fichiers principaux |
|---|------|------|---------------------|
| 1 | Fonctionnalité | Images/GIF **dans** la bulle de chat + envoi (1‑à‑1 & groupe), inline + repli fichier | `net.rs`, `main.rs`, `tauri.ts`, `transfer.ts`, `groups.ts`, `tauri.conf.json` |
| 2 | Fonctionnalité | Statut « dans le vocal » diffusé à tout le groupe, séparé de « parle » | `net.rs`, `tauri.ts`, `groups.ts` |
| 3 | Fonctionnalité | Préréglages qualité/fps du partage natif (Auto/2K60/1080p60/1080p30/720p30) | `video.rs`, `main.rs`, `tauri.ts`, `groups.ts` |
| 4 | **Bug** | Convergence du roster : ajout multi‑saut + tombstones de kick gossipés | `net.rs`, `tauri.ts`, `groups.ts` |
| 5 | **Bug** | Étiqueter le flux vidéo natif par `gid` → confiné à un seul groupe | `groups.ts` (signalisation) |

**Ordre de livraison** (du plus contenu au plus transverse) : **5 → 3 → 1 → 2 → 4**. Chaque item
est indépendant et livrable/vérifiable seul.

## Conventions (rappel projet, s'appliquent à chaque item)

- Toute édition de `ui/src/*.ts` → `npm run build`, et **commit du `ui/js/*.js` compilé** avec le `.ts`
  (le build Tauri sert `ui/js` tel quel, pas de compilation auto).
- Nouveaux chemins réseau : **borne l'allocation AVANT de la faire** (lire une longueur, la valider
  contre un plafond, puis allouer) et **n'accepte que des messages dont `from = remote_id`**
  (inforgeable) est un membre attendu.
- Tags 1‑à‑1 (`KIND_*`) et groupe (`GKIND_*`) : voir `net.rs`. Prochains libres au moment de la spec :
  `KIND_IMG = 9`, `GKIND_VOICE = 10`, `GKIND_GIMG = 11`.
- Tests : `cargo test` (meta.rs, video.rs) ; smoke matériel `-- --ignored` ; le reste se vérifie en
  `cargo tauri dev` avec deux instances/pairs.

---

## Item 1 — Images & GIF dans la conversation (inline + repli fichier)

### État actuel
- Chat = **texte uniquement** : `addMsg` (`transfer.ts:24`) et `addGroupMsgDom` (`groups.ts:383`)
  posent le contenu via `textContent`.
- Les fichiers passent par un flux offre/acceptation séparé et finissent en ligne « ✅ fichier reçu »
  (`transfer.ts:155`), jamais dans la bulle.
- CSP (`src-tauri/tauri.conf.json:23`) : `img-src 'self' data:` → **les data‑URI sont déjà autorisés**.

### Conception
Deux chemins selon la taille, seuil **`MAX_INLINE_IMG = 5 Mo`** (côté UI) :

1. **Inline (≤ 5 Mo)** — nouveau type de message qui transporte les octets. Rendu direct dans la
   bulle via un `data:<mime>;base64,<...>` (GIF animé nativement). Aucune demande d'acceptation.
2. **Repli fichier (> 5 Mo)** — réutilise `send_file` / `send_gfile` (transfert chunké existant).
   À la réception, si le fichier est une image, on l'**affiche dans la bulle** au lieu de la ligne
   « fichier reçu ».

### Déclencheurs d'envoi (UI)
- Bouton **🖼️** à côté du champ de chat (ouvre un sélecteur de fichier image).
- **Coller (Ctrl+V)** une image dans le champ de chat.
- **Glisser‑déposer** une image sur la zone de chat (l'app a déjà `tauri://drag-drop`).
Dans les trois cas : si `taille ≤ 5 Mo` → chemin inline ; sinon → repli fichier.

### Protocole
**1‑à‑1** : `KIND_IMG = 9` sur flux bidi (comme `KIND_CHAT`). Charge utile :
```
u16 author_len | author | u16 mime_len | mime | u16 name_len | name | u32 data_len | data
```
- `data_len` validé contre un plafond dur **`MAX_IMG_WIRE = 8 MiB` AVANT allocation** (rejet + log si dépassé).
- `mime` restreint à une liste blanche (`image/png|jpeg|gif|webp`) ; sinon rejet.
- Émet l'événement `ghost-chat-img { author, name, mime, dataB64 }`.

**Groupe** : `GKIND_GIMG = 11`, envoyé **par pair** sur le mesh (comme `GKIND_GFILE`, mais one‑shot).
Charge utile identique + `gid`. Émet `ghost-gchat-img { group, author, name, mime, dataB64, from }`.
Filtrage réception : n'accepter que si `from` est membre du groupe `gid` connu (sinon ignorer).

**Repli fichier (> 5 Mo)** : nouvelle commande `read_image_bytes(path) -> Vec<u8>` (bornée à
**≤ 32 MiB**, liste blanche de mime par extension). Côté TS :
`URL.createObjectURL(new Blob([bytes], { type }))` → `<img>`. **CSP à étendre** : `img-src 'self'
data: blob:` (ajout de `blob:`). On préfère le blob au protocole d'asset Tauri pour ne pas ouvrir
tout le dossier de téléchargement au WebView (confidentialité).

### Rendu
- Variante image dans `addMsg` / `addGroupMsgDom` : `<img>` avec `max-width:100%`, `loading="lazy"`,
  clic = ouverture en grand (overlay). Le GIF s'anime sans code supplémentaire.
- L'auteur/heure suivent la même mise en forme que les bulles texte.

### Persistance (limite assumée, validée avec l'utilisateur)
- Les images inline restent **en mémoire pour la session** (dans `S.groupMsgs` pour le groupe) mais
  **ne sont pas écrites en `localStorage`** (le base64 exploserait le quota). Au rechargement, elles
  disparaissent (messagerie éphémère). Les métadonnées texte des autres messages ne changent pas.

### Sécurité
- Data‑URI construit en mémoire à partir d'**octets reçus du pair chiffré** (pas d'URL externe) → pas
  de pixel‑traceur, rien à autoriser côté réseau.
- Bornes d'allocation strictes (8 MiB fil inline, 32 MiB repli fichier) — cohérent avec la posture
  net.rs sur les allocations pilotées par le réseau.

### Vérification
- 1‑à‑1 et groupe : envoyer PNG/JPEG/WebP/**GIF animé** ≤ 5 Mo → apparition inline, GIF animé.
- Image > 5 Mo → bascule transfert, rendu inline à la fin.
- Un mime hors liste blanche ou un `data_len` > 8 MiB → rejeté proprement (log, pas de crash).

---

## Item 2 — Statut vocal séparé (visible même hors appel)

### État actuel
- `ghost-voice-activity` porte `{ inCall, speaking }` par membre et bascule `.incall`/`.speaking`
  sur les chips (`groups.ts:1903`), **mais seulement si l'utilisateur est lui‑même dans l'appel**
  (`showHere = S.inGroupCall && S.groupCallId === S.openGroupId`).
- Cette activité vient des datagrammes `CALL_PING` + pics `VOICE_TAG`, qui **ne circulent qu'à
  l'intérieur de l'appel** (`audio.rs`) → un non‑participant n'en reçoit rien.

### Conception — deux statuts distincts
1. **« Dans le vocal » (présence, diffusée)** : chaque client dans un appel de groupe émet un
   **beacon ~1 Hz** sur le mesh du groupe. Les autres membres tiennent un set par groupe avec
   **expiration ~4 s** (pas de beacon = sorti). Rendu : pastille 🔊 persistante sur le chip +
   compteur d'en‑tête « 🔊 N dans le vocal ». **Visible même sans avoir rejoint l'appel.**
2. **« Parle » (activité, dans l'appel)** : on **garde** l'anneau animé `.speaking` existant,
   toujours conditionné à `S.inGroupCall && S.groupCallId === groupe ouvert` (les pics de voix ne
   sont connus que dans l'appel).

### Protocole
- `GKIND_VOICE = 10`, charge `{ gid, inCall: bool }`, `from = remote_id` **authentifié**.
- Émis à ~1 Hz tant que je suis dans l'appel du groupe `gid` (vers tous les membres du groupe, en
  ligne, via le mesh). À l'arrêt de l'appel : un message `inCall:false` immédiat.
- Se greffe sur `group_call_start` / `group_call_stop` (démarre/arrête la boucle de beacon).
- Émet vers l'UI `ghost-voice-presence { group, code, inCall }` (`code` = `from`).

### UI (`groups.ts`)
- `S.voicePresence: Record<gid, Map<code, lastSeenMs>>`, balayage TTL (4 s) via un `setInterval`.
- Rendu de la pastille sur `.mem[data-code]` **indépendamment** de `.speaking` (deux classes
  séparées, ex. `.inbooth` pour présence vs `.speaking` pour parole).
- Compteur d'en‑tête du groupe.

### Sécurité
- Beacon accepté seulement si `from` est membre connu du groupe `gid` (anti‑squat de pastille).

### Vérification
- A et B dans le groupe ; A rejoint l'appel, B **ne rejoint pas** : B voit la pastille 🔊 sur A et le
  compteur « 1 dans le vocal », sans l'anneau « parle ». A quitte → la pastille disparaît sous ~4 s.
- A **parle** : l'anneau « parle » ne s'affiche que pour un participant de l'appel (pas pour B hors appel).

---

## Item 3 — Qualité/FPS du partage natif (préréglages + Auto)

> **Mise à jour 2026-07-16 (livraison) — FPS-ONLY.** La downscale de résolution demande un étage
> de mise à l'échelle absent du pipeline natif (`bgra_to_nv12` rogne/letterboxe 1:1). Sur décision
> utilisateur, la **résolution reste native** et **seul le plafond de fps est livré** dans ce lot ;
> la downscale (2K/1080p/720p) est reportée. Le sélecteur devient donc un choix de **fps** (ex.
> « Auto/60 » ou « 30 »). Le tableau de préréglages de résolution ci-dessous reste la cible d'un
> lot futur.

### État actuel
- `FPS: u32 = 30` constant (`video.rs:24`) ; capture à la **résolution native** ; échelle adaptative
  `LEVELS = [(30,100),(20,66),(12,40),(8,25)]` (`video.rs:34`) qui **part de (30,100 %) et ne fait
  que descendre**. Aucune entrée utilisateur : `video_share_start` ne prend ni fps ni résolution.
- L'étage BGRA→NV12 letterbox/crop déjà dans un **buffer FIXE** (`video.rs`) — l'encodage à une
  résolution cible différente de la capture est donc faisable sans nouveau code de mise à l'échelle
  majeur.

### Conception — préréglages (plafonds)
| Préréglage | Résolution encodée | FPS cible |
|---|---|---|
| **Auto (max)** *(défaut)* | native écran/fenêtre | plafond 60 |
| 2K 60 | 2560×1440 | 60 |
| 1080p 60 | 1920×1080 | 60 |
| 1080p 30 | 1920×1080 | 30 |
| 720p 30 | 1280×720 | 30 |

- Le choix est un **plafond** : l'adaptatif peut toujours descendre sous saturation réseau.
- **Jamais de sur‑échantillonnage** : une cible > résolution native est clampée au natif.
- Le fps est un **plafond réel** : 60 exige un rafraîchissement d'écran ≥ 60 Hz + encodeur matériel
  (NVENC/AMF/QSV, déjà validé par le smoke test).

### Protocole & Rust
- `video_share_start(members, monitor, window, maxFps: u32, maxW: u32|null, maxH: u32|null)`.
- `video.rs` :
  - `FPS` const → `target_fps` runtime ; `frame_interval` et GOP (`fps * KEYFRAME_SECS`) recalculés.
  - `LEVELS` recalibré **relativement** à `target_fps` (niveau 0 = `(target_fps, 100%)`, crans
    inférieurs en proportion).
  - Dimensions d'encodage/NV12 = cible clampée au natif (letterbox conservé) ; capture native inchangée.
  - `bitrate_for(w,h)` gagne un **facteur fps** (≈ ×1.5 à 60 fps) pour ne pas sous‑bitrater du 60.
  - `StartInfo`/retour `{w,h,fps}` = valeurs **réellement encodées**.

### UI (`groups.ts`)
- Dropdown/segmenté « Qualité » en tête du picker natif (`openNativePicker`), **persisté**
  (`localStorage: ghostlink_stream_quality`, défaut « Auto »).
- La vignette d'état et `ghost-video-stats` (déjà `fps/kbps/w/h`) reflètent la qualité choisie en direct.

### Vérification
- Choisir 2K60 sur un écran ≥ 1440p/≥60 Hz → stats montrent ~2560×1440@60.
- Choisir 2K60 sur un écran 1080p → clampé à 1080p (pas d'upscale).
- Sous saturation réseau simulée → l'adaptatif descend **sous** la cible puis remonte.
- `cargo test` (video.rs : NV12/letterbox/keyframe) reste vert ; smoke `-- --ignored` OK.

---

## Item 4 — Convergence du roster (gossip pragmatique)

### État actuel (bugs)
- **Ajout** : `addMembersToGroup` n'envoie `send_gmembers` qu'aux **membres en ligne à l'instant**
  (`groups.ts:300`) ; le rattrapage `ghost-mesh-up` ne renvoie le roster **qu'au pair qui revient**
  (`groups.ts:1838`) → **un seul saut**, pas de re‑gossip → invisible pour tous si le mesh est incomplet.
- **Kick** : `kickedSet` est **local en `localStorage`** (`groups.ts:172`), jamais transmis → un pair
  absent au vote / arrivé plus tard ne l'apprend jamais et peut ré‑injecter l'exclu via l'union
  (`ghost-gmembers`, `groups.ts:1887`).

### Conception — 2P‑Set gossipé (membres ∪, tombstones de kick ∪), multi‑saut
- `send_gmembers` transporte désormais `{ gid, name, members, kicked, unkick }` (CSV).
- **Fusion à la réception** (handler `ghost-gmembers`) :
  ```
  kicked'  = (local.kicked ∪ incoming.kicked) − incoming.unkick
  members' = (local.members ∪ incoming.members) − kicked'
  ```
  (`unkick` = ré‑admissions explicites ; voir plus bas.)
- **Re‑diffusion sur changement effectif** : si la fusion modifie vraiment `members`/`kicked` local,
  re‑broadcast `send_gmembers` aux membres **en ligne** → propagation **multi‑saut** sans mesh complet.
  Garde‑fou anti‑tempête : re‑broadcast **uniquement si ça a changé** (on étend la garde « rien de
  neuf » de `groups.ts:1890` pour couvrir aussi `kicked`).
- **Reconnexion** (`ghost-mesh-up`, `groups.ts:1838`) : envoyer roster **+ kicked** au pair qui revient.
- **Kick** (`castKick`, `groups.ts:245`) : au quorum, `applyKick` ajoute au tombstone local **et** ce
  tombstone est désormais gossipé via `kicked` → vu par les arrivants tardifs.
- **Ré‑admission** (`addMembersToGroup`, `groups.ts:282`) : la ré‑invitation porte les codes en
  `unkick` → lève le tombstone chez les destinataires. Honoré **seulement** si `from` est un membre
  authentifié (même modèle de sécurité que la sync de roster, `groups.ts:1881`).

### Sécurité
- Sync/tombstone/unkick n'ont d'effet que si `from = remote_id` est un membre connu du groupe.
- Le vote‑kick lui‑même reste inchangé et **authentifié** (`from == voter`, `groups.ts:1921`).

### Limite assumée (documentée)
- Le lot reste **advisory** : une course kick ↔ ré‑admission concurrente peut demander une seconde
  ré‑invitation pour converger. Cohérent avec le fait que le vote‑kick est déjà advisory (un client
  malveillant peut s'ignorer). Pas de garantie CRDT formelle (choix explicite de l'utilisateur).

### Protocole
- `net.rs` : `send_gmembers` + champ(s) `kicked` (et `unkick`) ; `GKIND_GMEMBERS = 8` réutilisé
  (charge enrichie, pas de nouveau tag).
- `tauri.ts` : payload `ghost-gmembers` enrichi (`kicked?`, `unkick?`) ; args `send_gmembers` enrichis.

### Vérification (3 instances A, B, C)
- A ajoute C alors que **B est hors ligne** → à son retour, B voit C (via re‑gossip multi‑saut), même
  si B n'était pas directement connecté à A au moment de l'ajout.
- A/B votent l'exclusion de C (quorum) ; **D se connecte ensuite** → D voit C exclu (tombstone gossipé)
  et ne le ré‑injecte pas.
- A ré‑admet C explicitement → C réapparaît chez les membres (unkick authentifié).

---

## Item 5 — Confiner le partage d'écran à un seul groupe (étiquetage `gid`)

### État actuel (bug)
- L'émetteur cible bien `g.members` (`main.rs:294` `group_conns`), mais **le flux vidéo natif ne porte
  aucun `gid`** : le signal de démarrage n'envoie que `{ start, w, h, fps }` (`groups.ts:1588`) et les
  trames sont brutes. À la réception, `nativePeerAllowed` autorise par **simple co‑appartenance**
  (`groups.ts:1262`). Comme il n'y a **qu'une connexion par pair** dans le mesh, un partage destiné au
  groupe A **s'affiche dans l'appel du groupe B** si le pair est co‑membre des deux.

### Conception
- Le signal natif porte le **`gid`** : `sigSend(m, { nativeVideo: { start, w, h, fps, gid } })`
  (`groups.ts:1588`) et le signal d'arrêt aussi.
- `handleNativeSignal` (`groups.ts:1412`) **mémorise le `gid`** du partage par pair.
- `nativePeerAllowed(peer)` (`groups.ts:1262`) devient : autorisé **seulement si** le `gid` du partage
  de ce pair **== `S.groupCallId`** (le groupe de mon appel courant). Les trames d'un partage au `gid`
  non concordant → **aucune vignette** (ni décodage).
- **Trame avant signal** : ne plus créer la vignette sur la 1re trame ; attendre le signal `start`
  (avec `gid`) — sinon on ne peut pas savoir à quel groupe la rattacher. Trames orphelines ignorées
  jusqu'au signal.

### Portée
- Correction **contenue à la signalisation** ; les octets vidéo (flux uni `GKIND_VIDEO`) ne changent pas.

### Vérification
- Pair X co‑membre des groupes A et B. Je partage dans A pendant que X est dans l'appel de **B** →
  X **ne voit pas** ma vignette (gid ≠ groupe d'appel). X rejoint l'appel de A → il la voit.

---

## Notes de release (hors implémentation)
- Version cible **v0.34.0** : bumper les **4** emplacements synchronisés — `package.json`,
  `src-tauri/Cargo.toml`, `src-tauri/tauri.conf.json`, et `UI_BUILD` dans `ui/src/main.ts` — puis
  `.\scripts\release.ps1`. À faire seulement quand les 5 items sont livrés et vérifiés.

## Récapitulatif des changements de contrat (fil)
- `KIND_IMG = 9`, `GKIND_GIMG = 11`, `GKIND_VOICE = 10` (net.rs).
- Nouvelles commandes : `send_img`, `send_gimg`, `read_image_bytes` ; `video_share_start` gagne
  `maxFps`/`maxW`/`maxH` ; `send_gmembers` gagne `kicked`/`unkick`.
- Nouveaux événements : `ghost-chat-img`, `ghost-gchat-img`, `ghost-voice-presence` ; `ghost-gmembers`
  enrichi.
- CSP : `img-src 'self' data: blob:` (ajout `blob:` pour le repli grosse image).
