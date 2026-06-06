# ghost link 👻

Partage de fichiers **pair-à-pair, chiffré de bout en bout**, sans stockage serveur.
Application native (Windows / Linux) construite avec **Tauri 2** + **iroh** (QUIC).

- **De toi à ton ami, directement** : les fichiers ne transitent jamais en clair par un serveur (connexion directe, ou relais aveugle chiffré en dernier recours).
- **Identité = ta clé** : ton « code ami » est ta clé publique ed25519, stable dans le temps.
- **Fonctions** : transfert de fichiers (débit + annulation), chat chiffré, carnet d'amis avec présence en ligne, empreinte d'identité, demandes d'ami mutuelles, **mises à jour automatiques signées**.

---

## Prérequis

- [Rust](https://rustup.rs) (stable)
- La CLI Tauri 2 : `cargo install tauri-cli --version "^2"`
- (Facultatif) la [CLI GitHub `gh`](https://cli.github.com) pour publier une release en une commande
- Sous Linux : les dépendances système de Tauri (WebKitGTK, etc. — voir la doc Tauri)

## Développement

```sh
cd ghost-link-rust
cargo tauri dev
```

## Build (installeurs)

```sh
cargo tauri build
```

Produit, dans `src-tauri/target/release/bundle/` :
- Windows : `nsis/ghost-link_<version>_x64-setup.exe` (+ `.sig`) et `msi/…`
- Linux : `appimage/ghost-link_<version>_amd64.AppImage` (+ `.sig`)

---

## Mises à jour automatiques

L'app interroge au démarrage (et via l'onglet **Identité → Mises à jour**) le lien permanent
de la **dernière release GitHub** :
`https://github.com/julo0211/ghost-link/releases/latest/download/latest.json`.
Tauri **vérifie une signature cryptographique** avant d'installer : personne ne peut pousser
une fausse mise à jour sans ta clé privée.

> Le `latest.json` et les installeurs sont publiés comme **assets d'une release** (pas commités dans le dépôt).

### Clé de signature (déjà faite)

La paire a été générée avec `cargo tauri signer generate -w ~/.tauri/ghostlink.key`,
et la **clé publique** est déjà dans `src-tauri/tauri.conf.json` (`plugins.updater.pubkey`).
La **clé privée** (`~/.tauri/ghostlink.key`) et son mot de passe doivent rester **secrets**
(jamais commités — `.gitignore` exclut `*.key`). Si tu les perds, tu ne peux plus publier de MAJ.

---

## Mettre le projet sur GitHub

Le dépôt cible : `julo0211/ghost-link` (push via SSH).

1. Crée le dépôt **vide et public** sur https://github.com/new
   (nom `ghost-link`, **sans** README / .gitignore / licence).
2. Depuis `ghost-link-rust/`, lance le script (il fait init, commit, remote SSH, push) :

   ```powershell
   .\scripts\setup-github.ps1
   ```

   …ou à la main :

   ```sh
   git init
   git add .
   git commit -m "ghost link v0.11.0"
   git branch -M main
   git remote add origin git@github.com:julo0211/ghost-link.git
   git push -u origin main
   ```

---

## Publier une nouvelle version (runbook)

À chaque release : **bump de version → build signé → latest.json → release GitHub**.

1. **Augmente la version** (strictement supérieure à l'installée) dans :
   - `src-tauri/tauri.conf.json` (`version`)
   - `src-tauri/Cargo.toml` (`version`)
   - `ui/index.html` (pied de page, cosmétique)

2. **Build signé** — exporte la clé privée puis build (PowerShell) :

   ```powershell
   $env:TAURI_SIGNING_PRIVATE_KEY = Get-Content "$HOME\.tauri\ghostlink.key" -Raw
   $env:TAURI_SIGNING_PRIVATE_KEY_PASSWORD = '<ton mot de passe>'
   cargo tauri build
   ```

   Tu obtiens `…_x64-setup.exe` **et** `…_x64-setup.exe.sig`.

3. **Génère `latest.json`** (les URLs pointent vers le lien « latest » de GitHub) :

   ```sh
   node scripts/make-latest-json.mjs --repo julo0211/ghost-link --version <version> `
     --notes "Ce qui change" `
     --win-sig "src-tauri/target/release/bundle/nsis/ghost-link_<version>_x64-setup.exe.sig"
   ```

   (Ajoute `--linux-sig "...AppImage.sig"` si tu builds aussi pour Linux.)

4. **Crée la release GitHub** pour le tag `v<version>` et téléverse **deux fichiers** :
   le `-setup.exe` **et** `latest.json`.

   - Web : https://github.com/julo0211/ghost-link/releases → *Draft a new release* → tag `v<version>` → glisse les fichiers → *Publish*.
   - Ou en une commande avec `gh` :

     ```sh
     gh release create v<version> `
       "src-tauri/target/release/bundle/nsis/ghost-link_<version>_x64-setup.exe" `
       "latest.json" `
       --title "v<version>" --notes "Ce qui change"
     ```

Les apps installées détectent la nouvelle version, téléchargent depuis la release,
**vérifient la signature**, installent et redémarrent.

> La toute première release (v0.11.0) n'aura rien à mettre à jour pour l'app 0.11.0 déjà installée (versions égales) — c'est normal, elle amorce juste le mécanisme. La version suivante (0.11.1, 0.12.0…) déclenchera la MAJ.

---

## Sécurité

- La clé **privée** de signature ne doit JAMAIS être commitée ni partagée (`.gitignore` la protège).
- Le code ami / l'empreinte permettent de vérifier de vive voix qu'un contact est bien le bon.
- Fichiers et messages sont chiffrés de bout en bout par le canal QUIC d'iroh (TLS 1.3),
  liés aux clés ed25519 des deux pairs.

## Structure

```
ghost-link-rust/
├─ ui/index.html               ← interface (onglets Transfert / Amis / Identité)
├─ scripts/
│  ├─ setup-github.ps1         ← init git + push (SSH)
│  └─ make-latest-json.mjs     ← génère le manifeste de mise à jour
└─ src-tauri/
   ├─ Cargo.toml · build.rs · tauri.conf.json · capabilities/
   ├─ icons/
   └─ src/ main.rs · net.rs    ← app Tauri + cœur réseau iroh
```

## Licence

À définir (ex. MIT). Ajoute un fichier `LICENSE` à la racine si tu veux clarifier la réutilisation.
