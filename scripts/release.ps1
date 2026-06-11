<#
  release.ps1 — publie une version de ghost link en UNE commande.
  Enchaîne : build signé -> latest.json -> commit+push -> release GitHub.

  Prérequis (sur ta machine) : cargo + tauri-cli, node, git, gh (authentifié),
  et la clé de signature dans $HOME\.tauri\ghostlink.key.

  À lancer depuis ghost-link-rust\ :
      .\scripts\release.ps1
      .\scripts\release.ps1 -Notes "Transfert multi-flux + intégrité SHA-256"

  La version est lue automatiquement dans src-tauri\tauri.conf.json.
#>
param(
  [string]$Repo    = "julo0211/ghost-link",
  [string]$Notes   = "Ameliorations et corrections.",
  [string]$KeyPath = "$HOME\.tauri\ghostlink.key",
  # Identité publique des commits (n'expose pas ton vrai email).
  [string]$Name    = "julo0211",
  [string]$Email   = "julo0211@users.noreply.github.com"
)
$ErrorActionPreference = "Stop"
Set-Location (Split-Path -Parent $PSScriptRoot)

# --- Outils requis ---
foreach ($t in @("cargo","node","git","gh")) {
  if (-not (Get-Command $t -ErrorAction SilentlyContinue)) { throw "$t introuvable dans le PATH." }
}
if (-not (Test-Path $KeyPath)) { throw "Clé privée introuvable : $KeyPath" }

# --- Version lue dans tauri.conf.json ---
$conf    = Get-Content "src-tauri\tauri.conf.json" -Raw | ConvertFrom-Json
$Version = $conf.version
if (-not $Version) { throw "Impossible de lire la version dans tauri.conf.json" }
Write-Host "==> Release ghost link v$Version  (depot: $Repo)" -ForegroundColor Cyan

# Cohérence Cargo.toml / tauri.conf.json
$cargoVer = (Select-String -Path "src-tauri\Cargo.toml" -Pattern '^version\s*=\s*"([^"]+)"').Matches[0].Groups[1].Value
if ($cargoVer -ne $Version) { throw "Versions incohérentes : Cargo.toml=$cargoVer, tauri.conf.json=$Version. Aligne-les avant de publier." }

# --- 1) Build signé ---
Write-Host "`n[1/5] Build signé (cargo tauri build)..." -ForegroundColor Yellow
$env:TAURI_SIGNING_PRIVATE_KEY = Get-Content $KeyPath -Raw
$sec = Read-Host "Mot de passe de la cle de signature" -AsSecureString
$env:TAURI_SIGNING_PRIVATE_KEY_PASSWORD = [System.Net.NetworkCredential]::new("", $sec).Password
cargo tauri build
if ($LASTEXITCODE -ne 0) { throw "cargo tauri build a échoué." }

# --- 2) Localiser l'installeur + sa signature ---
Write-Host "`n[2/5] Recherche de l'installeur..." -ForegroundColor Yellow
$exe = Get-ChildItem "src-tauri\target\release\bundle\nsis\*_x64-setup.exe" -ErrorAction Stop |
       Sort-Object LastWriteTime | Select-Object -Last 1
$sig = "$($exe.FullName).sig"
if (-not (Test-Path $sig)) { throw "Signature .sig introuvable : $sig (le build signé l'a-t-il bien produite ?)" }
Write-Host "    $($exe.Name)"

# --- 3) latest.json (manifeste de l'updater) ---
Write-Host "`n[3/5] Génération de latest.json..." -ForegroundColor Yellow
node scripts\make-latest-json.mjs --repo $Repo --version $Version --notes $Notes --win-sig $sig
if ($LASTEXITCODE -ne 0) { throw "make-latest-json a échoué." }

# --- 4) Commit + push du code ---
Write-Host "`n[4/5] Commit + push..." -ForegroundColor Yellow
git config user.name  $Name
git config user.email $Email
git add -A
git commit -m "ghost link v$Version" 2>$null | Out-Null
git push origin main
if ($LASTEXITCODE -ne 0) { throw "git push a échoué (clé SSH / accès dépôt ?)." }

# --- 5) Release GitHub (crée, ou met à jour les assets si le tag existe déjà) ---
Write-Host "`n[5/5] Release GitHub v$Version..." -ForegroundColor Yellow
gh release view "v$Version" --repo $Repo *> $null
if ($LASTEXITCODE -eq 0) {
  Write-Host "    La release v$Version existe déjà -> mise à jour des assets." -ForegroundColor DarkYellow
  gh release upload "v$Version" "$($exe.FullName)" "latest.json" --repo $Repo --clobber
} else {
  gh release create "v$Version" "$($exe.FullName)" "latest.json" `
    --repo $Repo --title "ghost link v$Version" --notes $Notes
}
if ($LASTEXITCODE -ne 0) { throw "La publication de la release a échoué." }

Write-Host "`n✅ Publié : https://github.com/$Repo/releases/tag/v$Version" -ForegroundColor Green
Write-Host "Les apps en 0.19.0 verront la mise à jour (rappel : le multi-flux exige 0.20.0 des DEUX côtés)." -ForegroundColor Cyan
