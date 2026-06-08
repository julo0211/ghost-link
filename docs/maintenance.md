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

## Clé de signature

- Générée une fois avec `cargo tauri signer generate -w ~/.tauri/ghostlink.key`.
- La **clé publique** est dans `src-tauri/tauri.conf.json` (`plugins.updater.pubkey`).
- La **clé privée** (`~/.tauri/ghostlink.key`) + son mot de passe sont **secrets** : jamais commités
  (`.gitignore` exclut `*.key`). Les perdre = ne plus pouvoir publier de mises à jour.

## Publier une nouvelle version

1. **Bumper la version** (strictement supérieure) dans :
   - `src-tauri/tauri.conf.json` (`version`)
   - `src-tauri/Cargo.toml` (`version`)
   - `ui/index.html` (pied de page, cosmétique)

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
