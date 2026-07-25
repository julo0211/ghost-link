# Maintenance — compiler et publier ghost link

Document destiné au mainteneur du projet (pas aux utilisateurs).

## Prérequis

- [Rust](https://rustup.rs) (stable)
- CLI Tauri 2 : `cargo install tauri-cli --version "^2"`
- [Node.js](https://nodejs.org) (pour générer le manifeste de mise à jour)
- [GitHub CLI `gh`](https://cli.github.com) (pour pousser/publier facilement)

## Développer

```sh
cd ghost-link-rust
cargo tauri dev
```

## Build signé (release)

Les artefacts de mise à jour doivent être **signés**. Le plus simple :

```powershell
.\scripts\build-signed.ps1
```

Le script demande le mot de passe de la clé (saisie masquée), pose les variables d'environnement
de signature pour la session, puis lance `cargo tauri build`. Résultat dans
`src-tauri/target/release/bundle/` : le `…_x64-setup.exe` **et** son `…_x64-setup.exe.sig`.

> Équivalent manuel (PowerShell) :
> ```powershell
> $env:TAURI_SIGNING_PRIVATE_KEY = Get-Content "$HOME\.tauri\ghostlink.key" -Raw
> $env:TAURI_SIGNING_PRIVATE_KEY_PASSWORD = '<mot de passe>'
> cargo tauri build
> ```

## Clé de signature — POINT DE DÉFAILLANCE UNIQUE

- Générée une fois avec `cargo tauri signer generate -w ~/.tauri/ghostlink.key`.
- La **clé publique** est dans `src-tauri/tauri.conf.json` (`plugins.updater.pubkey`), et elle est
  **compilée dans chaque binaire distribué**. Un client installé n'accepte QUE des mises à jour
  signées par cette clé-là.
- La **clé privée** (`~/.tauri/ghostlink.key`) + son mot de passe sont **secrets** : jamais commités
  (`.gitignore` exclut `*.key`).

⚠️ **Conséquence à mesurer** : perdre la clé privée ou son mot de passe, c'est perdre définitivement
la capacité de mettre à jour le parc déjà installé — y compris pour un correctif de sécurité. Il n'y
a **aucune procédure de rotation** : les clients existants n'accepteraient pas une nouvelle clé. La
seule issue serait de demander à chaque utilisateur de réinstaller à la main.

**À faire une fois, maintenant** :

1. Copier `~/.tauri/ghostlink.key` sur **deux supports hors ligne distincts** (clé USB chiffrée,
   coffre-fort de mots de passe qui accepte les pièces jointes…). Pas dans un dossier synchronisé
   en clair.
2. Déposer le mot de passe dans un gestionnaire de mots de passe — il ne doit pas vivre uniquement
   dans la tête du mainteneur.
3. **Vérifier la sauvegarde en conditions réelles** (une sauvegarde non testée n'en est pas une) :
   `.\scripts\build-signed.ps1` en pointant la copie doit produire un `.sig` à côté de l'installeur.

## Publier une nouvelle version

1. **Bumper la version** (strictement supérieure) dans les **4** emplacements — `scripts/release.ps1`
   vérifie leur cohérence et refuse de publier s'ils divergent :
   - `src-tauri/tauri.conf.json` (`version`)
   - `src-tauri/Cargo.toml` (`version`)
   - `package.json` (`version`)
   - `ui/src/main.ts` (constante `UI_BUILD`)

   Puis `npm install --package-lock-only` pour reporter la version dans `package-lock.json`, et
   `npm run build` (le TypeScript **n'est pas** compilé automatiquement par Tauri).

   ⚠️ **Ne jamais republier un numéro déjà distribué** : l'updater compare les versions, pas les
   binaires. Une seconde `0.35.0` ne pourrait pas atteindre les clients déjà en `0.35.0`.

2. **Build signé** : `.\scripts\build-signed.ps1`

3. **Générer `latest.json`** (les URLs pointent vers la dernière release GitHub) :

   ```powershell
   node scripts/make-latest-json.mjs --repo julo0211/ghost-link --version <version> `
     --notes "Ce qui change" `
     --win-sig "src-tauri/target/release/bundle/nsis/ghost-link_<version>_x64-setup.exe.sig"
   ```

4. **Créer la release** pour le tag `v<version>`, en y joignant le `-setup.exe` **et** `latest.json` :

   ```powershell
   gh release create v<version> `
     "src-tauri/target/release/bundle/nsis/ghost-link_<version>_x64-setup.exe" `
     "latest.json" `
     --title "v<version>" --notes "Ce qui change"
   ```

Les apps installées détectent la nouvelle version, téléchargent depuis la release, **vérifient la
signature**, installent et redémarrent. (Sur GitHub, `latest.json` et l'installeur sont des *assets*
de release, pas des fichiers commités.)

## Pousser le code

```powershell
.\scripts\setup-github.ps1
```

Initialise le dépôt si besoin et pousse sur `git@github.com:julo0211/ghost-link.git` (via SSH).
Identité de commit neutre par défaut (pseudo + email `noreply`).

## Structure

```
ghost-link-rust/
├─ ui/index.html                ← interface (onglets Transfert / Amis / Identité, réglages)
├─ docs/
│  ├─ maintenance.md            ← ce fichier
│  └─ plan-chat-vocal.md        ← plan du futur chat vocal
├─ scripts/
│  ├─ build-signed.ps1          ← build signé (demande le mot de passe)
│  ├─ make-latest-json.mjs      ← génère le manifeste de mise à jour
│  └─ setup-github.ps1          ← init git + push (SSH)
└─ src-tauri/
   ├─ Cargo.toml · build.rs · tauri.conf.json · capabilities/
   ├─ icons/
   └─ src/ main.rs · net.rs     ← app Tauri + cœur réseau iroh
```
