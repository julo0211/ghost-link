# Checklist de test ghost link v0.38.0 (avec des amis)

> Lot issu de l'audit du 2026-09-22 (`../../AUDIT-2026-09-22.md`). Tout est **compilé + testé
> unitairement** (67 tests) et quatre constats ont été reproduits puis corrigés sur banc
> (`../../experiences-audit-2026-09-22/`) — mais **rien n'a encore tourné entre deux vraies
> instances**. À faire AVANT `.\scripts\release.ps1`.
>
> **Prérequis :** deux (idéalement trois) PC différents — deux instances sur le même PC partagent
> la même identité (`identity.key`). Tout le monde sur la même build 0.38.0 : Réglages →
> « ghost link 0.38.0 · UI 0.38.0 ».

## 1. Appel de groupe — le plus important (3 personnes : A, B, C)
- [ ] A et B démarrent un appel de groupe. **C lance l'appli APRÈS**, puis rejoint l'appel →
      les trois s'entendent (avant : C n'entendait personne et personne ne l'entendait).
      Le Journal de A et B affiche « 🔊 C rattaché à l'appel en cours ».
- [ ] En plein appel, **B ferme puis relance l'appli**, rejoint → A et B s'entendent à nouveau sans
      que A ait raccroché.
- [ ] Régler le volume de B à 150 % chez A, puis B se reconnecte → le curseur ET le son restent à 150 %.
- [ ] A partage son écran **avec le son** ; C arrive ensuite dans l'appel → C entend le son du partage.
      (La VIDÉO native, elle, reste figée au démarrage du partage : relancer ⏹️ puis 🖥️ — limite connue.)
- [ ] **Débrancher le casque** en plein appel de groupe → message « 🎧 Appel de groupe coupé… »,
      l'appel se termine proprement, la pastille « dans le vocal » s'éteint chez les autres.
- [ ] Même test en appel **1-à-1** → message « 🎧 Appel coupé… », le pair voit l'appel se terminer.

## 2. Session 1-à-1 et code éphémère (2 personnes)
- [ ] B donne son **code éphémère** à A ; A s'y connecte ; B clique « 🔄 Nouveau code » →
      confirmation « Ta session en cours passe par ton code éphémère… ». Si B confirme : **les deux**
      voient « Déconnecté » tout de suite (avant : session morte en silence, messages perdus).
- [ ] A (non-ami de B) se connecte au code **permanent** de B, puis A tourne SON code éphémère →
      même comportement côté A.
- [ ] Entre deux **amis** (code permanent), tourner le code éphémère → aucune question, la session
      n'est PAS coupée.
- [ ] En appel 1-à-1 avec A, B accepte une connexion entrante de C → l'appel avec A se termine
      (micro coupé), l'historique de A disparaît, la conversation avec C s'ouvre vide.
- [ ] Deux demandes de connexion entrantes d'affilée : la première expire (45 s) → la bannière de la
      seconde reste affichée et acceptable.

## 3. Présence « amis uniquement » (2 personnes)
- [ ] A coche « amis uniquement » et retire B de ses amis → dans la liste d'amis de B, A apparaît
      **hors ligne** (avant : toujours « en ligne »).
- [ ] A décoche → B revoit A en ligne au rafraîchissement suivant.

## 4. Maillage de groupe (2 personnes)
- [ ] Lancer les deux applis **au même moment** (compter « 3, 2, 1 »), plusieurs fois → le groupe
      finit toujours avec l'autre « en ligne » (avant : 9 fois sur 10 les deux connexions mouraient).

## 5. Fichiers et images (2–3 personnes)
- [ ] Glisser une image de **200 Ko** verrouillée ou sur un lecteur réseau inaccessible → message avec
      la VRAIE raison, fichier « prêt » mais pas envoyé (avant : « dépasse 5 Mo »).
- [ ] Glisser une photo de **20 Mo** → la question de repli apparaît sans gel de la fenêtre.
- [ ] Glisser une image de 20 Mo **dans un groupe**, confirmer → les membres ayant ce groupe ouvert
      voient l'image dans la conversation ; tous voient le CHEMIN dans le Journal.
- [ ] Fichier de groupe refusé par un membre → l'expéditeur lit « ⛔ … non reçu par X : refusé »
      (avant : « Fichier envoyé au groupe » quoi qu'il arrive).
- [ ] Deux membres envoient un fichier en même temps → les DEUX offres s'affichent l'une après
      l'autre (« +1 en attente »), aucune n'est refusée en silence.
- [ ] Recevoir deux fois « photo.jpg » → « photo (1).jpg » puis « photo (2).jpg », l'original intact.
- [ ] Débrancher la clé USB choisie comme dossier de réception, accepter un fichier → message
      « accepté mais impossible à enregistrer ».
- [ ] La bannière d'offre affiche le dossier de destination (« → C:\…\Downloads »).
- [ ] Réglages → dossier de réception = dossier Démarrage (`shell:startup`) → **refusé**.
- [ ] Coller un `.bmp` ou `.svg` copié depuis l'Explorateur → « non prise en charge », rien n'est envoyé.
- [ ] Envoyer une photo **HEIF `.HIF`** (ou sans extension) → avertissement « métadonnées non
      nettoyables », et la photo s'ouvre chez le destinataire (avant : détruite).

## 6. Groupes : exclusion et roster (3 personnes)
- [ ] Après exclusion de C par vote, C lance un appel dans le groupe → A et B ne reçoivent **ni
      bannière ni sonnerie**.
- [ ] C (exclu, toujours ami) propose un fichier « de groupe » → refusé avec un message, sans bannière.
- [ ] B envoie une demande d'ami à A en déclarant le code d'un ami existant de A → le nom de cet ami
      n'est PAS modifié chez A.

## 7. Release
- [ ] `.\scripts\release.ps1` sur une version déjà publiée (sans monter la version) → refus immédiat,
      AVANT la compilation signée.
- [ ] Depuis une 0.37.3 installée, la mise à jour vers 0.38.0 est proposée et s'installe.
