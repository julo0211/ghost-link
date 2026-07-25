# ghost link 👻

**Partage de fichiers et messagerie, directement d'un appareil à l'autre — chiffré de bout en bout, sans serveur.**

Tes fichiers ne passent jamais en clair par un serveur : la connexion va **de toi à ton ami, directement**. Quand une connexion directe est impossible, le trafic transite par un **relais public opéré par [number0](https://n0.computer)** : il ne peut **pas** lire ce qui passe (chiffré de bout en bout), mais il voit ton adresse IP et ton identité publique. Ton identité, c'est une clé qui reste sur ton appareil.

---

## Installation

### Windows
1. Va dans **[Releases](https://github.com/julo0211/ghost-link/releases/latest)**.
2. Télécharge `ghost-link_x.y.z_x64-setup.exe`.
3. Lance-le et suis l'installation.

L'application se **met à jour toute seule** : quand une nouvelle version sort, elle te le propose (onglet **Identité → Mises à jour**, ou au démarrage).

*(Linux : une version AppImage pourra être proposée plus tard.)*

---

## Premiers pas

1. **Ton code ghost.** Onglet **🪪 Identité → « Afficher mon code »**. C'est ton identité, courte et stable : partage-la à tes amis pour qu'ils puissent te joindre. L'**empreinte** affichée juste en dessous sert à vérifier de vive voix que c'est bien toi.

2. **Ajoute un ami.** Onglet **👥 Amis** : colle son code + un nom, puis **Ajouter**. Le **point vert** indique qu'il est en ligne (bouton **⟳ Statut** pour rafraîchir).

3. **Connecte-toi.** Clique **🔌 Connecter** sur un ami (ou colle un code dans l'onglet **Transfert**). Une fois connectés, vous pouvez tout faire, dans les deux sens.

4. **Envoie un fichier.** Onglet **📤 Transfert** : **glisse-dépose** un fichier dans la zone prévue (ou colle son chemin), puis **Envoyer**. Tu vois le débit et tu peux annuler. Les fichiers reçus arrivent dans ton dossier **Téléchargements** (modifiable dans les réglages).

5. **Discute.** La section **💬 Discussion** apparaît quand tu es connecté : messages chiffrés, en direct.

6. **Demande d'ami mutuelle.** Pendant une session, **➕ Demander en ami** : l'autre accepte ou refuse. Les amis confirmés des deux côtés affichent un **✓ mutuel**.

---

## Réglages ⚙️

Bouton **engrenage** en haut à droite :

- **Nom d'affichage** — le nom que tes pairs voient dans le chat et les demandes d'ami.
- **Dossier de réception** — où sont enregistrés les fichiers reçus (par défaut : Téléchargements).
- **N'accepter que les amis** — refuse les connexions de pairs qui ne sont pas dans ton carnet d'amis.

Tu peux aussi basculer entre **thème clair et sombre** avec le bouton 🌙 / ☀️.

---

## Confidentialité & sécurité

### Ce qui est garanti

- **Chiffrement de bout en bout** : fichiers, messages, voix et vidéo sont chiffrés par le canal sécurisé (QUIC / TLS 1.3), liés aux clés des deux pairs. L'identité du pair est vérifiée cryptographiquement à chaque connexion : **personne au milieu ne peut lire ni se faire passer pour ton ami**.
- **Aucun contenu stocké ailleurs** : rien de ce que tu envoies n'est conservé sur un serveur — ni tes fichiers, ni tes messages, ni ton historique.
- **Ton identité reste chez toi** : ta clé privée ne quitte jamais ton appareil (chiffrée au repos sous Windows).
- **Métadonnées retirées avant envoi** : photos (EXIF/GPS), documents et vidéos sont **nettoyés automatiquement** avant de partir — l'original sur ton disque n'est jamais modifié. Ce qui n'a pas pu être nettoyé est **signalé** dans le Journal, jamais passé sous silence.
- **Vérifie un contact** : compare son **empreinte** (onglet Identité) avec ce qu'il t'annonce de vive voix. C'est la seule façon d'être certain que le code que tu enregistres est bien le sien.

### Ce qu'il faut savoir (les limites, dites franchement)

- **Ton adresse IP est visible de ton pair.** C'est inhérent au pair-à-pair : la connexion est directe.
- **Des relais tiers voient des métadonnées.** L'app utilise les relais et l'annuaire publics de number0. Ils ne voient **jamais le contenu**, mais ils apprennent ton adresse IP, ton identité publique et le moment où tu es en ligne. Ce n'est pas configurable pour l'instant.
- **La vidéo par caméra ouvre une connexion WebRTC** qui contacte un serveur STUN public (Google) et expose ton IP aux membres du groupe. L'app **te le demande explicitement** avant. Le **partage d'écran natif**, lui, ne contacte aucun serveur tiers — c'est le chemin recommandé.
- **Ton code permanent ne peut pas être révoqué.** Qui l'a peut savoir quand tu es en ligne (sauf si tu actives « N'accepter que les amis »). Utilise le code éphémère pour un échange ponctuel.
- **Le vote d'exclusion d'un groupe est indicatif.** Sans serveur, personne ne peut forcer un client malveillant à se retirer : les clients honnêtes l'ignorent, c'est tout.
- **Perdre ton appareil, c'est perdre ton identité.** Il n'existe pas encore d'export de la clé.

---

## Fonctionnalités

- **Transfert de fichiers** volumineux, multi-flux, avec débit, annulation et vérification d'intégrité.
- **Chat** texte et **images/GIF** en ligne (glisser-déposer ou Ctrl+V).
- **Appel vocal** 1-à-1 et **en groupe**.
- **Partage d'écran ou de fenêtre**, en natif (sans WebRTC ni STUN), avec choix du nombre d'images par seconde et de la résolution.
- **Groupes** décentralisés : chat, appel, invitations, exclusion par vote.
- **Thèmes** clair/sombre et quatre identités visuelles.

---

## À propos

ghost link est une application native (Windows / Linux) construite avec Tauri et iroh.
Le code de ce dépôt est public. *(Pour compiler le projet ou publier une version, voir [`docs/maintenance.md`](docs/maintenance.md).)*

Licence : à définir.
