# Images et GIF glissés → affichage inline dans la conversation

**Date** : 2026-07-25 · **Cible** : v0.37.1 (non encore publiée)

## Problème

Le Ctrl+V et le bouton 🖼️ envoient une image **inline** : elle s'affiche dans la conversation
et n'est jamais écrite sur le disque du destinataire. Le glisser-déposer, lui, passe par le
transfert de fichier : le destinataire doit accepter une offre, et l'image atterrit dans ses
Téléchargements. Deux gestes équivalents pour l'utilisateur, deux résultats opposés.

Comportement voulu : **une image ou un GIF glissé se comporte comme un Ctrl+V**.

## La contrainte qui structure tout

Le presse-papiers fournit un objet `File` — donc des octets directement lisibles par le JS.
Le glisser-déposer ne fournit qu'un **chemin**. Or la WebView n'a aucun accès disque : les
capabilities se limitent à `core:default` + `updater:default` (ni plugin `fs`, ni `shell`,
ni protocole d'assets), et `read_image_bytes` a été confiné au dossier de réception lors de
l'audit du 25/07 — précisément pour supprimer une primitive de lecture arbitraire.

Faire « comme le Ctrl+V » impose donc de rouvrir une porte de lecture. Toute la conception
consiste à ne l'ouvrir que sur les fichiers que l'utilisateur a **physiquement déposés**.

### Précédent dans Tauri

Tauri fait déjà exactement cela en interne (`tauri-2.11.2/src/manager/window.rs:232-240`) :
à chaque `DragDropEvent::Drop`, il ajoute les chemins déposés au `Scopes` de l'application,
*avant* d'émettre `tauri://drag-drop` vers le JS. Le principe « un dépôt physique vaut
autorisation » est donc un motif éprouvé et non une invention.

Ce scope n'est consommé que par `tauri-plugin-fs` et le protocole d'assets, que l'app ne
déclare pas — c'est ce qui lui donne sa surface IPC minimale. On reproduit donc le motif
localement, en ~30 lignes, plutôt que d'importer un plugin entier.

Vérifié également : l'émission vers le JS (l. 248) a lieu dans le gestionnaire **interne** de
Tauri, indépendamment de tout `on_window_event` applicatif. Ajouter un observateur ne peut
donc pas casser le glisser-déposer.

## Architecture

### Backend (`src-tauri/src/main.rs`)

**`DroppedPaths`** — état managé par Tauri : ensemble de `PathBuf` canonicalisés, borné à 32
entrées (purge complète au dépassement). Alimenté **uniquement** par
`Builder::on_window_event` sur `WindowEvent::DragDrop(DragDropEvent::Drop { paths, .. })`.
Aucune commande ne permet d'y écrire : le JS ne peut pas forger d'entrée.

**`chemin_autorise(dossier, path, deposes)`** remplace `confiner_dans`. Accepte si le chemin
canonicalisé est :
1. présent dans `DroppedPaths` (déposé par l'utilisateur), **ou**
2. sous le dossier de réception (cas historique : aperçu d'un fichier reçu).

Sinon : erreur. `read_image_bytes` conserve ses bornes existantes (fichier régulier, 32 Mio).

### Frontend (`ui/src/transfer.ts`)

Dans le gestionnaire `tauri://drag-drop`, après le routage par vue déjà en place :

- **Le fichier n'est pas une image** (`guessImageMime` → `null`) : comportement actuel,
  inchangé (remplissage de la zone, envoi au clic).

> **INVARIANT** — un dépôt ne déclenche un envoi de FICHIER que dans un seul cas : une image
> trop lourde dont l'utilisateur vient de confirmer le repli. Partout ailleurs, déposer
> **prépare** l'envoi ; c'est le clic sur « Envoyer » qui l'ordonne. Une image ≤ 5 Mo part
> d'elle-même parce qu'elle s'affiche dans la conversation au lieu d'atterrir chez le
> destinataire — ce n'est pas la même chose.
>
> Violé une première fois à l'implémentation : la queue « envoi fichier » était partagée
> entre le cas confirmé et le cas ordinaire, si bien que déposer un PDF l'envoyait
> immédiatement. Les deux branches doivent rester **séparées**, pas factorisées.
- **Image ou GIF ≤ 5 Mo** : lecture des octets via `read_image_bytes`, puis `send_img`
  (1-à-1) ou `send_gimg` (groupe), et bulle locale. Les métadonnées sont nettoyées par
  `clean_inline_img`, déjà en place — aucun travail supplémentaire.
- **Image > 5 Mo, ou illisible** : `confirm()` nommant explicitement la conséquence
  (« elle atterrira dans les Téléchargements de ton correspondant »). Si accepté, envoi
  fichier immédiat — le `confirm` remplace le clic sur « Envoyer ». Si refusé, rien, et une
  ligne dans le Journal.

La limite de 5 Mo est celle du protocole inline (`MAX_INLINE_IMG` côté UI, `MAX_IMG_WIRE`
= 8 Mio sur le fil).

## Gestion des erreurs

Aucun `catch` muet. Chaque échec — lecture impossible, envoi refusé, annulation — produit une
ligne de Journal. C'est la leçon #7 du projet : un dépôt qui ne peut pas aboutir doit le dire.

## Tests

**Automatiques (Rust)** :
- un chemin déposé hors du dossier de réception est accepté ;
- le même chemin, non déposé, est refusé ;
- un fichier du dossier de réception reste accepté (non-régression de l'aperçu) ;
- la traversée `..` reste refusée ;
- le registre reste borné au-delà de 32 entrées.

**Manuels, à 2 pairs** (`docs/CHECKLIST-test-v0.35.md`) :
- JPEG géolocalisé glissé → bulle inline des deux côtés, « 🧹 Métadonnées retirées » au
  Journal, aucun fichier dans les Téléchargements du destinataire ;
- GIF animé glissé → inline, **et l'animation boucle** (vérifie `NETSCAPE2.0`) ;
- GIF de 12 Mo glissé → confirmation, puis fichier ;
- PDF glissé → comportement inchangé ;
- même série depuis un groupe.

## Conséquence assumée

Une image inline n'est **jamais** écrite sur le disque du destinataire : il la voit, peut
l'ouvrir en plein écran, mais ne peut pas la conserver. C'est le revers direct de l'objectif
« pas de fichier chez les gens », et c'est un choix explicite.

## Hors périmètre

- Fermer le finding `send_file` de l'audit avec ce même registre. Le mécanisme s'y prête
  (c'est même sa vocation), mais `send_gfile` n'a aujourd'hui aucun autre moyen de désigner
  un fichier que la saisie du chemin : le gater tuerait les envois de groupe. À traiter avec
  l'ajout d'un sélecteur natif, dans un lot dédié.
- Permettre au destinataire d'enregistrer une image inline reçue.
