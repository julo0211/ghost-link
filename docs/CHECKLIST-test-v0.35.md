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
- [ ] **Confinement par groupe (v0.34 item 5)** : être co-membre de 2 groupes G1 et G2 avec un ami ;
      partager dans G1 pendant que l'ami est dans l'appel de **G2** → il **ne voit PAS** ton écran. Il
      rejoint l'appel de **G1** → il le voit.
- [ ] **Sélecteur de fluidité (fps)** : choisir 60 vs 30 fps dans le picker → le stat overlay reflète le
      choix ; le réglage est **mémorisé** au prochain partage. (Résolution native — pas de choix de résolution.)
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
- Résolution de partage **native** (choix de résolution reporté ; seul le fps est réglable).
- Destinataires d'un partage figés au **démarrage** du partage (un arrivant tardif : relancer le partage).
- Images de chat **éphémères** (non persistées) ; images de groupe affichées seulement pour le groupe ouvert.
- Transfert de gros fichiers : léger délai au démarrage (calcul du hash) — annulable. (#43 = optimisation reportée.)
