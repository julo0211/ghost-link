# Checklist de test ghost link v0.35.0 (avec des amis)

> ⚠️ **Rien de ce lot (v0.34.0 features + correctifs de revue + durcissement v0.35.0) n'a été testé
> en runtime.** Tout est « compile + revu ». Cette liste couvre ce qui doit être vérifié en vrai.
>
> **Prérequis :** tout le monde sur la **même version 0.35.0** (`.\scripts\release.ps1` d'abord, puis
> chacun met à jour). Idéalement tester à **3** pour les groupes/roster. Note la version affichée dans
> Réglages → « ghost link X · UI 0.35.0 » : le X et l'UI doivent **coïncider (0.35.0)**.

## 0. Base P2P (2 personnes)
- [ ] A se connecte à B via son code → les deux passent « Connecté ».
- [ ] Présence des amis : ajouter un ami, il apparaît en ligne/hors ligne correctement.
- [ ] **Demande d'ami (#48 corrigé)** : A envoie une demande d'ami à B pendant une session ; B accepte
      → **A voit bien « Ami ajouté »** (le correctif ne doit PAS rejeter une acceptation légitime).
- [ ] Un `FACCEPT` non sollicité (difficile à provoquer manuellement) ne doit jamais ajouter un ami
      sans qu'une demande sortante ait été envoyée — comportement attendu : message « ignorée ».
- [ ] Fermeture de l'app d'un pair → l'autre passe « déconnecté » **rapidement** (pas après un long timeout).

## 1. Transfert de fichiers (2 personnes)
- [ ] Envoi d'un petit fichier A→B : accepté, reçu intègre.
- [ ] Envoi d'un **gros fichier** (≥ quelques Go) : barre de progression, débit, **annulation** en cours
      (des 2 côtés) fonctionne — y compris **pendant la phase de hash** (annulation réactive).
- [ ] **Glisser-déposer (#15)** : déposer **plusieurs** fichiers d'un coup → message clair « un seul à la
      fois, X sélectionné, N ignorés » (pas d'ignorance silencieuse). Déposer un **dossier** → refus propre.
- [ ] Le fichier reçu porte bien le **nom d'origine** et passe la vérif d'intégrité (pas de « corrompu »).

## 2. Métadonnées (privacy) — dont #12 sous-processus PDF
- [ ] Envoyer une **photo JPEG avec GPS/EXIF** → à l'arrivée, EXIF/GPS **retiré** (bannière « nettoyé »).
- [ ] Envoyer un **PDF avec auteur/métadonnées** → nettoyé (bannière « nettoyé »), le PDF s'ouvre correctement.
- [ ] **#12 — l'app ne crashe pas sur un PDF** : envoyer plusieurs PDF (dont un gros/complexe) → aucun
      plantage de l'app ; au pire un PDF « sauté » (bannière visible), jamais un crash silencieux.
- [ ] Envoyer un **MP4 (caméra/téléphone) avec métadonnées** → nettoyé ou averti (#13 : XMP/uuid).
- [ ] Un fichier **sans extension** ou à extension inhabituelle (`.jpe`, renommé) contenant de l'EXIF →
      nettoyé OU bannière visible (#46 : plus d'échec silencieux).

## 3. Chat + images/GIF (2 personnes)
- [ ] Chat texte 1-à-1 et de groupe : envoi/réception OK, nom d'auteur correct.
- [ ] **Image/GIF inline** : coller (Ctrl+V) ou bouton 🖼️ une image ≤ 5 Mo → s'affiche **dans la bulle**,
      le **GIF s'anime**. Clic → ouverture en grand.
- [ ] **#20** : envoyer une image dans le groupe A, puis **changer de groupe pendant l'envoi** → l'image
      NE doit PAS apparaître dans le chat du mauvais groupe.
- [ ] Image > 5 Mo via bouton/coller → message clair « glisse-la sur la fenêtre pour l'envoyer en fichier ».
- [ ] Session longue avec beaucoup d'images → pas de fuite mémoire visible (#44, blobs révoqués).
- [ ] **Image en plein écran** : cliquer sur une image du chat l'ouvre en grand (fond noir).
      - [ ] fonctionne pour une image **reçue** ET pour une image **que j'ai envoyée** (les deux sources) ;
      - [ ] en **groupe** ET en **1-à-1** ;
      - [ ] se ferme au clic sur le fond, par le bouton ✕, **et** par la touche Échap ;
      - [ ] un clic **sur l'image** ne la referme pas ;
      - [ ] après avoir changé de groupe puis être revenu, les images encore affichées s'ouvrent toujours
            (blob non révoqué trop tôt) ;
      - [ ] un **GIF animé** s'anime en plein écran, et se fige bien à la fermeture.

## 4. Appels vocaux / vidéo — les 3 bugs HIGH (2–3 personnes)
- [ ] Appel de groupe : audio dans les deux sens, **mute** fonctionne, `CALL_PING` (indicateur « en appel »).
- [ ] **#1 (caméra fantôme)** : **double-cliquer vite** sur 📹 (caméra) → puis cliquer « arrêter » → la
      caméra doit **réellement s'éteindre** (LED off) et ne plus être diffusée. (C'était LE risque : une
      caméra que l'UI dit éteinte mais qui diffuse encore.)
- [ ] **#2 (micro non raccroché)** : rejoindre l'appel du groupe A, **ouvrir le groupe B** (l'appel de A
      continue), puis **supprimer le groupe A** via la ✕ → le micro doit être **coupé** (personne ne
      t'entend plus dans A) et l'appel proprement terminé.
- [ ] **#3 (signal WebRTC)** : un ami **hors du groupe** ne doit jamais faire apparaître une vignette
      vidéo chez toi ni déclencher une connexion — rien ne s'affiche hors de l'appel de ton groupe.
- [ ] **#6/#7 (bascule d'appel)** : en appel dans A, rejoindre l'appel de B → l'appel/partage de A est
      **bien arrêté** (pas de partage orphelin qui continue d'émettre vers A). « Fermer » le groupe B ne
      doit PAS couper la vidéo d'un appel actif d'un autre groupe.
- [ ] **#5 (pair qui revient)** : pendant que A partage sa caméra, B redémarre son app puis revient →
      B **reçoit à nouveau** le flux (pas besoin d'arrêter/relancer le partage).

## 5. Partage d'écran natif + qualité (2–3 personnes)
- [ ] Partage d'écran natif (Réglages → partage natif) : les membres **dans l'appel** voient l'écran ;
      son système/fenêtre optionnel fonctionne.
- [ ] 🔴 **ÉCHO CORRIGÉ (le bug rapporté)** — partager l'**écran entier** en acceptant « Partager aussi
      le SON ? », pendant que 2 autres personnes parlent dans l'appel :
      - [ ] **personne ne s'entend en écho** (c'était le symptôme : les voix repartaient dans le flux) ;
      - [ ] le son des autres applis (vidéo, musique, jeu) est bien transmis, lui ;
      - [ ] même vérification en partageant **une fenêtre** (doit rester sans écho, comme avant) ;
      - [ ] le sélecteur Windows **ne propose plus** de case « Partager l'audio » : c'est normal et voulu,
            le son passe désormais toujours par la capture native anti-écho.
      - [ ] Si un écho persiste : es-tu en **haut-parleurs** ? (le micro peut capter les enceintes — teste
            au casque) et as-tu une **sortie audio virtuelle** (VoiceMeeter, SteelSeries Sonar, NVIDIA
            Broadcast) ? Ces deux cas ont des causes différentes : signale-le, ne relance pas le test.
- [ ] **Confinement par groupe** : être co-membre de 2 groupes G1 et G2 avec un ami ; partager dans G1
      pendant que l'ami est dans l'appel de **G2** → il **ne voit PAS** ton écran. Il rejoint l'appel
      de **G1** → il le voit.
      ⚠️ Depuis la v0.35.5 ce confinement repose uniquement sur « être dans l'appel du groupe dont
      l'émetteur est membre » (comportement v0.33). Le verrouillage plus strict par identifiant de
      groupe a été RETIRÉ : il rendait le partage définitivement invisible dès que son signal
      d'annonce était manqué. Si l'ami est membre des DEUX groupes, il peut donc voir le partage
      depuis l'appel de G2 — limite assumée, à re-durcir seulement après un vrai test à 2 pairs.
- [ ] **Sélecteur de fluidité (fps)** : choisir 60 vs 30 fps dans le picker → le stat overlay reflète le
      choix ; le réglage est **mémorisé** au prochain partage.
- [ ] **Sélecteur de résolution** (720p / 1080p / native), depuis un écran 1440p ou 4K :
      - [ ] en **720p** : le log au lancement annonce bien `1280×720`, la vignette d'état affiche la même
            chose, et le débit (stats) **chute nettement** vs natif ;
      - [ ] chez le récepteur l'image est **nette et non déformée** (pas de bandes, pas de bouillie) ;
      - [ ] en **1080p** depuis un écran **1080p** → reste 1080p (**aucun sur-échantillonnage**) ;
      - [ ] redimensionner une **fenêtre** partagée en 720p → bandes noires, jamais de gel ni d'image cassée ;
      - [ ] le réglage est **mémorisé** au partage suivant.
- [ ] **720p à 60 fps** s'affiche bien chez le récepteur (piège du niveau H.264 : une vignette qui
      resterait **noire** signalerait une régression du codec annoncé).
- [ ] **#47** : un ami **hors du groupe** ne doit pas pouvoir faire apparaître une vignette « X (écran) »
      chez toi.

## 6. Statut vocal « dans le vocal » (v0.34 item 2) (3 personnes)
- [ ] A rejoint l'appel d'un groupe, **B ne rejoint pas** : B voit la **pastille 🔊 « dans le vocal »**
      sur A et un compteur, **sans** avoir rejoint. A quitte → la pastille disparaît en quelques secondes.
- [ ] Quand A **parle**, l'anneau « parle » n'apparaît que pour un participant de l'appel (distinct de 🔊).

## 7. Groupes : roster (ajout/kick) — v0.34 item 4 (3 personnes)
- [ ] **Ajout propagé** : A ajoute C au groupe pendant que **B est hors ligne** ; B revient → B voit C
      (l'ajout converge, même si B n'était pas connecté à A au moment de l'ajout).
- [ ] **Kick vu par les arrivants (gossip de votes)** : A et B votent l'exclusion de C (quorum atteint) ;
      un pair **D se connecte ensuite** → D voit C exclu (le kick converge via re-gossip des votes).
- [ ] **Pas d'exclusion unilatérale** : un seul membre ne doit **pas** pouvoir exclure quelqu'un sans que
      le quorum de votes soit atteint. (Le correctif de sécurité : plus de gossip de « tombstones ».)
- [ ] Ré-admettre un membre exclu → il réapparaît (peut demander une 2ᵉ ré-invitation — limite advisory assumée).

## 8. Mise à jour auto
- [ ] Depuis une 0.34.0 installée, la **mise à jour vers 0.35.0** est proposée et s'installe.
- [ ] **#21** : si le téléchargement de la MàJ échoue (couper le réseau), un 2ᵉ essai « installer » doit
      re-proposer la MàJ (pas de « aucune mise à jour en attente » trompeur).

## Notes / limites connues (normales, ne pas signaler comme bugs)
- Vote-kick **advisory** (un mesh sans serveur ne peut pas forcer une exclusion).
- Le son du partage d'écran passe **toujours** par la capture native (le navigateur ne fournit plus
  l'audio) : une seule question « Partager aussi le SON ? » après le lancement, écran comme fenêtre.
- Destinataires d'un partage figés au **démarrage** du partage (un arrivant tardif : relancer le partage).
- Images de chat **éphémères** (non persistées) ; images de groupe affichées seulement pour le groupe ouvert.
- Transfert de gros fichiers : léger délai au démarrage (calcul du hash) — annulable. (#43 = optimisation reportée.)
