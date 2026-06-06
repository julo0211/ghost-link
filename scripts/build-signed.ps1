<#
  build-signed.ps1 — build signé sans retaper les variables à chaque terminal.
  Demande le mot de passe de la clé (saisie masquée), pose les variables pour CETTE session,
  puis lance `cargo tauri build`.

  À lancer depuis ghost-link-rust\ :
      .\scripts\build-signed.ps1
#>
param([string]$KeyPath = "$HOME\.tauri\ghostlink.key")
$ErrorActionPreference = "Stop"

Set-Location (Split-Path -Parent $PSScriptRoot)
if (-not (Test-Path $KeyPath)) { throw "Clé privée introuvable : $KeyPath" }

$env:TAURI_SIGNING_PRIVATE_KEY = Get-Content $KeyPath -Raw
$sec = Read-Host "Mot de passe de la cle de signature" -AsSecureString
$env:TAURI_SIGNING_PRIVATE_KEY_PASSWORD = [System.Net.NetworkCredential]::new("", $sec).Password

cargo tauri build
