# Plan — Chat vocal pour ghost link

Objectif : une **conversation vocale en direct, chiffrée de bout en bout**, entre deux pairs déjà
connectés (la session iroh existante), sans serveur, dans l'esprit du reste de l'app.

> Statut : plan validé à écrire en code par étapes. Je ne peux pas compiler/tester ici → on avance
> par jalons compilés/testés chez toi, comme pour le reste.

---

## 1. Principe

On réutilise la **connexion iroh déjà ouverte** (celle qui sert aux fichiers et au chat texte).
La voix passe par les **datagrammes QUIC** d'iroh : des paquets **non fiables** (un paquet perdu
est ignoré, pas réémis) — c'est ce qu'on veut pour la voix, où un retard est pire qu'une perte.
Le tout est chiffré par le canal iroh (TLS 1.3), comme le reste.

Chaîne audio (chaque sens, en parallèle) :

```
Micro ──cpal──> PCM 48kHz mono ──Opus encode──> [paquets ~20 ms] ──iroh datagram──>
        ... réseau ...
──iroh datagram──> Opus decode ──> tampon anti-gigue ──cpal──> Haut-parleur
```

---

## 2. Choix techniques

| Brique | Choix | Pourquoi |
|---|---|---|
| Capture / lecture audio | **`cpal`** | I/O audio multiplateforme (Windows WASAPI, Linux ALSA/Pulse), sans serveur audio externe. |
| Codec | **`audiopus`** (libopus intégré) | Opus = le standard voix : très bonne qualité à ~24 kbit/s, résilient aux pertes. `audiopus` embarque libopus → pas de dépendance système à installer. |
| Transport | **datagrammes iroh** (`Connection::send_datagram` / `read_datagram`) | Non fiable + faible latence, déjà chiffré, sur la connexion existante. |
| Tampon anti-gigue | maison (petite file ordonnée par n° de séquence) | Lisse les variations de délai réseau. |
| Ré-échantillonnage (si besoin) | **`rubato`** | Si le périphérique n'est pas en 48 kHz. |

Format audio cible : **48 kHz, mono, trames de 20 ms** (960 échantillons). Une trame Opus fait
quelques centaines d'octets → tient largement dans **un** datagramme QUIC.

Paquet voix (dans un datagramme) : `[u8 type=VOICE][u16 seq][octets Opus]`.
Le `seq` sert au tampon anti-gigue (réordonner, détecter les pertes).

Signalisation (début/fin d'appel, mute) : sur le **flux fiable existant**, via de nouveaux types de
message (`KIND_CALL_START`, `KIND_CALL_STOP`) — comme le chat/les demandes d'ami.

---

## 3. Interface

- Dans la session : bouton **📞 Appeler** / **Raccrocher**, un voyant « en appel », un bouton **🔇 Muet**.
- Plus tard : choix du micro / haut-parleur (réglages ⚙️), indicateur de niveau (VU-mètre).

---

## 4. Jalons (compilés/testés à chaque étape)

- **V1 — Boucle locale audio** : `cpal` capture → lecture en local (s'entendre avec un délai).
  Valide que cpal voit le micro et le HP sur ta machine. *Aucun réseau.*
- **V2 — Opus en local** : insérer encode/decode Opus dans la boucle V1. Valide le codec.
- **V3 — Appel brut entre pairs** : envoyer les trames Opus en datagrammes iroh, décoder et jouer
  côté pair. Pas encore de tampon anti-gigue → ça peut « hacher », mais on s'entend à distance.
- **V4 — Anti-gigue + pertes** : petit tampon (~60–100 ms) trié par `seq`, gestion des paquets
  perdus/en retard (Opus sait masquer une perte). Son fluide.
- **V5 — Confort** : bouton muet, signalisation appel propre (sonnerie/accept), choix des
  périphériques, VU-mètre, raccrochage propre des deux côtés.

---

## 5. Risques & points d'attention (honnêtes)

- **Écho** : sans annulation d'écho (AEC), si l'autre est en haut-parleurs, sa voix repart dans son
  micro. Parade simple au début : **casque/écouteurs** (recommandé), ou *push-to-talk*. Une vraie AEC
  (ex. `webrtc-audio-processing`) est lourde et délicate → à envisager plus tard seulement.
- **Latence** : viser < 150 ms bout-à-bout. Trames 20 ms + tampon court. Le direct iroh aide ; via
  relais ce sera un peu plus haut.
- **Périphériques** : taux d'échantillonnage variables selon le matériel → prévoir le
  ré-échantillonnage (rubato) si le périphérique n'offre pas 48 kHz.
- **Code temps réel délicat** : les callbacks audio cpal tournent dans un thread temps réel — pas de
  blocage, pas d'alloc lourde dedans ; on communique avec le réseau via des files (ring buffers).
- **Build** : nouvelles dépendances natives (`cpal`, `audiopus`). `audiopus` compile libopus tout seul,
  mais le 1er build sera plus long. Je ne peux pas compiler ici → on corrigera les détails d'API au fur
  et à mesure (datagrammes : confirmer la signature exacte `send_datagram(Bytes)` / `read_datagram()`).
- **Permissions micro** : sous Windows, l'accès micro de l'app native devra être autorisé (réglage
  Confidentialité de Windows).

---

## 6. Dépendances à ajouter (au moment de coder)

```toml
cpal = "0.15"
audiopus = "0.3"      # libopus intégré
# (optionnel selon besoin)
rubato = "0.15"       # ré-échantillonnage
```

Pas de serveur, pas de nouveau service : tout reste P2P sur la connexion iroh existante.

---

## 7. Alternative écartée

**Micro via la WebView** (`getUserMedia` + Web Audio, audio relayé par le pont JS↔Rust) : rejeté —
le pont IPC n'est pas fait pour de l'audio temps réel continu (latence, à-coups), et il faudrait
quand même Opus en WASM. La voie **native (cpal + Opus + datagrammes)** est la bonne.

---

## 8. Décision demandée

Si ce plan te convient, on démarre par **V1** (boucle audio locale) : petit, sûr, et ça prouve que
cpal voit ton micro/HP avant d'investir dans le reste. On enchaîne jalon par jalon.
