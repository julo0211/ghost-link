# Plan — Groupes (chat, appels et vidéo à plusieurs)

But : permettre des **groupes entre amis** — channel texte partagé, appel audio à plusieurs, partage d'écran et webcam. **Petits groupes (~4-6), 100 % P2P, sans serveur.**

## Contrainte d'architecture : le maillage (mesh)

Sans serveur, chaque membre doit se connecter à **chaque** autre membre (N membres → N-1 connexions par personne). Or aujourd'hui ghost link est strictement **1-à-1** : un seul emplacement de connexion (`ConnState`/`slot`). La fondation des groupes, c'est donc passer d'**une** connexion à une **carte de connexions** (`code → Connection`).

Conséquence : ça monte vite en bande passante (le trafic *montant* croît avec la taille du groupe), surtout en vidéo → **on reste sur de petits groupes**. Les grands groupes exigeraient un serveur relais (SFU), ce qui casserait le modèle sans serveur / anonyme.

## Modèle de données

- Un **groupe** = `{ id, nom, membres: [codes permanents] }`. Les membres sont des amis (on utilise leur **code permanent**, stable).
- Stocké côté UI (localStorage) et transmis au backend pour qu'il sache qui mailler.
- « Ouvrir » un groupe → on tente une connexion vers chaque membre **en ligne** (présence déjà gérée pour les amis).

## Ce qui ne change PAS

Le 1-à-1 actuel (fichiers, chat direct, voix directe), l'identité à deux codes, le chiffrement, l'updater : **inchangés**. Le maillage de groupe est ajouté **en couche coexistante** pour ne rien casser de stable.

---

## Phase 1 — Fondation + chat de groupe  (v0.15.0)

- **Backend** : ajout d'une carte `peers: HashMap<String, Connection>` (connexions actives d'un groupe), à côté du `slot` 1-à-1 existant.
- **Protocole** : nouveau type de message `KIND_GCHAT` portant `[group_id][auteur][texte]`, **diffusé** à tous les membres connectés ; relais simple pour que tout le monde reçoive.
- **UI** : section « Groupes » (créer / lister / ouvrir un groupe) + vue *channel* (fil de messages avec auteur, liste des membres en ligne).
- **Test** : possible à 2-3 machines (ou 2 + soi-même).

## Phase 2 — Appel audio de groupe  (v0.16.0)

- On **étend la voix Opus actuelle** : capter le micro une fois, envoyer le datagramme à **chaque** membre ; à la réception, **mixer** les flux de tous les pairs (somme + limiteur anti-saturation) avant lecture.
- **Signalisation** : `KIND_GCALL_JOIN` / `KIND_GCALL_LEAVE`. Consentement à rejoindre (bannière), comme en 1-à-1.
- Fluide à **~4-6** personnes.

## Phase 3 — Webcam + partage d'écran  (v0.17.0)

- La vidéo (capture caméra/écran + encodage VP8/H264 + rendu) serait **énorme** à coder en Rust. Le réaliste : utiliser la **WebRTC intégrée à la webview** :
  - `getUserMedia` (caméra + micro), `getDisplayMedia` (écran), `RTCPeerConnection` (transport média + encodage **natifs** du navigateur).
  - **iroh sert de canal de signalisation** : échange des offres/réponses SDP et des candidats ICE entre membres via les connexions déjà ouvertes.
  - Rendu : balises `<video>` dans l'UI.
- **Limite** : en maillage, chacun envoie sa vidéo à tous → réaliste à **~3-4** personnes (montant ≈ (N-1) × 1-3 Mbps). Au-delà → relais (SFU), hors périmètre.
- **Nuance NAT** : WebRTC fait sa propre traversée (ICE/STUN). Les NAT très stricts pourraient exiger un TURN (à valider ; le prototype web fonctionnait en P2P).

---

## Limites assumées

- **Petits groupes uniquement.** C'est le prix du « sans serveur ».
- **La vidéo est la plus exigeante** (bande passante + NAT) : c'est la dernière couche, et la plus susceptible d'être limitée.
- Chaque phase est **publiée et testée** avant d'attaquer la suivante.
