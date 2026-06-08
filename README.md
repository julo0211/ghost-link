# ghost link 👻

**Partage de fichiers et messagerie, directement d'un appareil à l'autre — chiffré de bout en bout, sans serveur.**

Tes fichiers ne passent jamais en clair par un serveur : la connexion va **de toi à ton ami, directement** (ou via un relais aveugle chiffré si la connexion directe est impossible). Ton identité, c'est une clé qui reste sur ton appareil.

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

- **Chiffrement de bout en bout** : fichiers et messages sont chiffrés par le canal sécurisé (QUIC / TLS 1.3), liés aux clés des deux pairs. Personne au milieu ne peut lire.
- **Aucun stockage serveur** : rien de ce que tu envoies n'est conservé ailleurs que chez ton destinataire.
- **Ton identité reste chez toi** : ta clé privée ne quitte jamais ton appareil.
- **Vérifie un contact** : compare son **empreinte** (onglet Identité) avec ce qu'il t'annonce, pour être sûr de parler à la bonne personne.

---

## À venir

- **Chat vocal** en direct (en préparation).

---

## À propos

ghost link est une application native (Windows / Linux) construite avec Tauri et iroh.
Le code de ce dépôt est public. *(Pour compiler le projet ou publier une version, voir [`docs/maintenance.md`](docs/maintenance.md).)*

Licence : à définir.
