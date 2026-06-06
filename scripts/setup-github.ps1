<#
  setup-github.ps1 — initialise le dépôt git et pousse ghost link sur GitHub (via SSH).
  La clé de signature et la clé publique sont déjà en place ; ce script ne fait QUE le git.

  À lancer depuis le dossier ghost-link-rust\, dans PowerShell :
      .\scripts\setup-github.ps1

  Étape manuelle unique : créer le dépôt VIDE et PUBLIC sur https://github.com/new
  (le script s'arrête pour te le rappeler, puis pousse).
#>
param(
  [string]$Repo  = "julo0211/ghost-link",
  # Identité utilisée pour l'auteur des commits (PUBLIQUE sur GitHub).
  # Par défaut : ton pseudo + un email "noreply" GitHub (n'expose pas ton vrai email).
  # Tu peux passer -Name / -Email pour mettre ce que tu veux.
  [string]$Name  = "julo0211",
  [string]$Email = "julo0211@users.noreply.github.com"
)
$ErrorActionPreference = "Stop"

$root = Split-Path -Parent $PSScriptRoot
Set-Location $root
if (-not (Get-Command git -ErrorAction SilentlyContinue)) { throw "git introuvable. Installe Git pour Windows." }

if (-not (Test-Path ".git")) { git init | Out-Null }
git config user.name  $Name
git config user.email $Email
git add -A
git commit -m "ghost link v0.11.0 — app native Tauri + iroh (P2P E2EE, chat, amis, presence, MAJ auto signees)" | Out-Null
git branch -M main
git remote remove origin 2>$null | Out-Null
git remote add origin "git@github.com:$Repo.git"
Write-Host "remote origin = git@github.com:$Repo.git" -ForegroundColor Green

Write-Host "`nAvant de pousser : cree le depot VIDE et PUBLIC sur https://github.com/new" -ForegroundColor Yellow
Write-Host "  - Repository name = ghost-link"
Write-Host "  - Public"
Write-Host "  - NE coche RIEN (pas de README, .gitignore ni licence)"
Read-Host "`nQuand le depot vide est cree, appuie sur Entree pour pousser"
git push -u origin main

Write-Host "`nTermine. Ton code est en ligne : https://github.com/$Repo" -ForegroundColor Green
Write-Host "Pour publier une version, suis la section 'runbook' du README." -ForegroundColor Cyan
