#!/usr/bin/env node
// Génère `latest.json` (le manifeste lu par l'updater Tauri) pour une release GitHub.
// Les URLs utilisent le lien permanent « dernière release » de GitHub
// (https://github.com/<repo>/releases/latest/download/<fichier>), stable d'une version à l'autre.
//
// Exemple :
//   node scripts/make-latest-json.mjs --repo julo0211/ghost-link --version 0.12.0 \
//     --notes "Corrections et améliorations" \
//     --win-sig "src-tauri/target/release/bundle/nsis/ghost-link_0.12.0_x64-setup.exe.sig"
//
// Ajoute --linux-sig "...AppImage.sig" si tu publies aussi pour Linux.
// Ensuite : téléverse le(s) installeur(s) ET ce latest.json comme « assets » de la release vX.

import fs from 'node:fs';
import path from 'node:path';

function arg(name) {
  const i = process.argv.indexOf('--' + name);
  return i >= 0 ? process.argv[i + 1] : undefined;
}

const repo = arg('repo');
const version = arg('version');
if (!repo || !version) {
  console.error('Requis : --repo <proprietaire/depot> et --version <x.y.z>');
  process.exit(1);
}
const notes = arg('notes') || '';
const base = `https://github.com/${repo}/releases/latest/download`;
const platforms = {};

function add(key, sigPath, urlOverride) {
  if (!sigPath) return;
  if (!fs.existsSync(sigPath)) {
    console.error('Signature introuvable : ' + sigPath);
    process.exit(1);
  }
  const signature = fs.readFileSync(sigPath, 'utf8').trim();
  const file = path.basename(sigPath).replace(/\.sig$/, '');
  platforms[key] = { signature, url: urlOverride || `${base}/${file}` };
}

add('windows-x86_64', arg('win-sig'), arg('win-url'));
add('linux-x86_64', arg('linux-sig'), arg('linux-url'));

if (Object.keys(platforms).length === 0) {
  console.error('Fournis au moins --win-sig (et/ou --linux-sig).');
  process.exit(1);
}

const manifest = { version, notes, pub_date: new Date().toISOString(), platforms };
fs.writeFileSync('latest.json', JSON.stringify(manifest, null, 2) + '\n');
console.log(`latest.json généré (v${version}) — plateformes : ${Object.keys(platforms).join(', ')}`);
console.log(`Téléverse le(s) installeur(s) + ce latest.json comme assets de la release v${version} sur GitHub.`);
