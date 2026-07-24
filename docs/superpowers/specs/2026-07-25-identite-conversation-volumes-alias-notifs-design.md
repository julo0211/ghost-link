# Spec — Identité de conversation, volumes persistés, renommage local, son & pastilles

- **Date** : 2026-07-25
- **Base** : commit `ba77388` (ghost link v0.35.5)
- **Cible de version** : v0.36.0 (lot de fonctionnalités → bump mineur)
- **Statut** : design validé, vérifié contre le source, en attente de relecture avant plan d'implémentation

> **Note de révision (2026-07-25)** — toutes les affirmations de « État actuel » de ce document ont
> été vérifiées ligne à ligne contre le commit `ba77388`. Quatorze affirmations de la première
> rédaction étaient fausses ou trompeuses et ont été corrigées ; les cinq qui changeaient une décision
> de conception sont signalées en ligne par **« corrigé »**. Ne pas réintroduire les formulations
> d'origine sans re-vérifier.

## Contexte & objectif

Trois idées utilisateur sur cinq, plus **un refactor dérivé** (l'item 0) sans lequel les deux
dernières coûteraient trois à quatre fois plus cher. Les deux idées restantes sont **volontairement
hors de ce lot** :

- **MP direct entre amis** (réutilisation du `Mesh`, nouveau `GKIND_DM`) → **release C**, seule :
  c'est le seul changement de protocole, et il dégrade en **trou noir silencieux** face à un pair
  v0.35.5.
- **Cam / partage d'écran en PV** → **release D**, seule, après campagne à 2 pairs : elle touche
  simultanément `nativePeerAllowed` (panne v0.34) et le budget de relais par connexion (panne
  v0.35.4). Le calcul qui l'a disqualifiée est en fin de document.

**Nommage des releases** (aucune n'existe avant ce document) : **A = ce lot (v0.36.0)** ;
**B** = notifications système (toast / zone de notification / flash) ; **C** = MP ; **D** = médias en
PV. B, C et D ne sont pas ordonnées entre elles.

Ce lot est, par construction, le sous-ensemble **sans risque réseau** : **zéro Rust, zéro tag
`KIND`/`GKIND`, zéro changement de charge utile QUIC, zéro capability Tauri, zéro modification du
CSP.**

> **Ce que ce lot n'est PAS — corrigé.** La première rédaction affirmait qu'il était « vérifiable sur
> une seule instance ». **C'est faux** : les deux tests que la spec elle-même désigne comme
> *principaux* (l'anti-régression de l'item 0 et le bug corrigé de l'item 1) exigent respectivement
> 3 et 2 instances, et **aucun curseur de volume ne s'affiche hors d'un appel actif**, lequel refuse
> de démarrer sans membre en ligne. La propriété réelle du lot est : **pas de risque réseau**, pas
> « pas de pairs pour tester ». Voir la section « Coût de vérification » avant de planifier.

## Vue d'ensemble

| # | Type | Cœur | Fichiers |
|---|------|------|----------|
| 0 | Refactor dérivé | **Spine « identité de conversation »** : un seul résolveur de libellé ; le message porte le code de son expéditeur ; les libellés affichés sont ré-étiquetables en place | `state.ts`, `dom.ts`, `tauri.ts`, `friends.ts`, `session.ts`, **`transfer.ts`**, `groups.ts`, **`index.html`** |
| 1 | Fonctionnalité + **bug** | Volumes par pair persistés, et re-poussés après l'entrée en appel | `state.ts`, `groups.ts`, `index.html`, `main.ts` |
| 2 | Fonctionnalité | Renommage local (alias) des amis **et** des membres de groupe | `state.ts`, `friends.ts`, `groups.ts`, `index.html` |
| 3 | **Durcissement** + fonctionnalité | 3a : trois failles existantes corrigées. 3b : son de notification. 3c : pastilles de non-lu | `dom.ts`, `state.ts`, `tauri.ts`, `main.ts`, `groups.ts`, `session.ts`, `transfer.ts`, `friends.ts`, `call.ts`, `index.html` |

**Ordre : 0 → 1 → 2 → 3a → 3b → 3c.**

> **Les items ne sont PAS livrables indépendamment**, contrairement à la convention du lot v0.34.
> L'item 0 est la fondation de l'item 2 (fortement) et de l'item 3c (faiblement — seule la
> résolution du nom dans la ligne de remplacement d'image en dépend). **Conséquence pour le plan** :
> un point de contrôle par item, et l'item 0 doit être découpé en commits (voir 0.8) car c'est le
> plus gros porteur de risque du lot et un `git bisect` sur un commit unique serait inutile.
> **Les items 0 et 2 forment une seule unité de risque** : après le spine, l'item 2 se réduit à deux
> puces. Le découpage est conservé pour le bisect, pas parce que ce sont deux chantiers.

## Conventions (rappel projet, s'appliquent à chaque item)

- Toute édition de `ui/src/*.ts` → `npm run build`, et **commit du `ui/js/*.js` compilé** avec le
  `.ts` (le build Tauri sert `ui/js` tel quel, pas de compilation auto).
- **Ne jamais transformer un signal best-effort en précondition d'affichage** (leçon #1). Visé
  nommément ici : le re-push de volume (item 1) et la résolution de libellé (items 0/2) — des
  surcouches, jamais des conditions.
- **Ne pas stocker d'état dans le DOM quand un autre chemin le réécrit.** `renderGroups` vide
  `#groupList` en `innerHTML` et `refreshGroupCounts` le re-rend à chaque `ghost-mesh-up`/`down`.
- **Ne jamais re-rendre un journal de chat pour rafraîchir un libellé** — voir 0.4, c'est la
  régression la plus coûteuse que ce document évite.
- La liste des clés `localStorage` de `CLAUDE.md` est documentée **exhaustive** : ce lot en ajoute
  **cinq**, toutes nommées en fin de document, à ajouter dans le même commit.
- Tests : `cargo test` reste vert sans modification (aucun code Rust n'est touché).

---

## Item 0 — Spine « identité de conversation »

### 0.1 État actuel (vérifié)

- `memberName(code)` (`state.ts:141-144`) résout : nom d'ami, sinon `shortId(code)`.
- **Il n'est importé que par `groups.ts`** (16 sites d'appel : 133, 277, 291, 306, 1059, 1424, 1431,
  1547, 1550, 1571, 1577, 1846, 1864, 2052, 2259, 2280). **Corrigé** : la première rédaction annonçait
  « 13 » puis « 18 » pour la même quantité, et laissait croire qu'il était largement adopté. **Sur ces
  16 appels, 10 sont des lignes de `log()`** ; il ne reste que **6 vraies surfaces d'affichage**
  (puce de membre 133, `confirm()` 306, étiquettes de tuile 1059/1424/1431, bannière d'offre de
  fichier 2280).
- `state.ts` importe **`tauri.ts` et `dom.ts`** — **corrigé** (la première rédaction disait « n'importe
  que `dom.ts` »). Le graphe reste acyclique (`tauri.ts` n'importe rien), et c'est une **bonne**
  nouvelle : `invoke` est déjà en portée dans `state.ts`, donc `ensureFpLabel` peut y vivre sans
  nouvelle arête.
- **Le nom auto-déclaré n'est borné que par le préfixe `u16` du cadre** (64 Kio) — **corrigé** :
  écrire « non borné » est faux, `read_lp16` alloue exactement la longueur lue. Ce qui manque est un
  **plafond sémantique** et une **élision à l'affichage** : un pseudo de 60 000 caractères est accepté
  et rendu tel quel.
- `GroupMsg` stocke pour un message entrant **la chaîne brute déclarée** (`p.author || "?"`) —
  **corrigé** : ce n'est pas un « nom déjà résolu », aucun résolveur ne tourne. `renderGroupMsgs`
  la rejoue verbatim.
- Rust **émet déjà** `from = connection.remote_id()` sur `ghost-gchat` (`net.rs:745`), authentifié
  et inforgeable sur les deux chemins (accept et dial). `tauri.ts:143` est **la seule** déclaration
  d'événement de groupe qui omet `from` (`ghost-gchat-img` et six autres le déclarent).

### 0.2 Le recensement des surfaces — c'est la définition de « fini »

L'item 0 est une campagne de reprise ; sa seule définition d'achèvement est ce tableau. **Les noms de
fonctions sont donnés plutôt que les numéros de ligne : ils ne rancissent pas.**

| Catégorie | Où | Traitement |
|---|---|---|
| **A. Passe déjà par `memberName`** (16 appels, `groups.ts`) | puce de membre, `confirm()` de kick, 3 étiquettes de tuile, bannière d'offre de fichier, 10 `log()` | héritent de l'alias **gratuitement** |
| **B. Lecture directe de `f.name`** (6) | `friends.ts` : item ami (`renderFriends`), log « prêt à te connecter » — `groups.ts` : les 2 sélecteurs d'amis (`renderGroupFriends`, ajout de membres) — `session.ts` : item du rail (`setConnected`), bannière `ghost-incoming` | **router vers `memberName`** |
| **C. Rendu du nom auto-déclaré** (4) | `groups.ts` : auteur de chat de groupe (`addGroupMsgDom`), auteur d'image de groupe — `transfer.ts` : auteur de chat 1-à-1 (`addMsg` via `listen("ghost-chat")`), auteur d'image 1-à-1 | **résoudre + marquer `.unverified` + estampiller `data-from`** (voir 0.4) |
| **D. Échelle d'empreinte dupliquée** (2) | `session.ts` (bannière entrante) et `friends.ts` (bannière de demande d'ami) | **factoriser les 3 barreaux bas** dans `ensureFpLabel` (voir 0.3) |
| **E. En-tête de session & logs PV** | `session.ts` : `#connStatus`, `#peerLabel`, tag `TEMP`, log de connexion (qui imprime le **code brut complet**), log de refus | router vers `memberName` |

> **`transfer.ts` et `index.html` étaient absents de la liste de fichiers de l'item 0 — corrigé, et
> c'était bloquant.** Les deux surfaces de la catégorie C du chat 1-à-1 vivent dans `transfer.ts` ;
> un implémenteur suivant l'ancienne liste aurait livré l'item 0 avec le chat PV affichant toujours
> le nom intégralement usurpable — le trou exact que l'item 0 existe pour fermer. `index.html` porte
> la classe `.unverified` (tout le CSS y est en ligne), sans laquelle la vérification de l'item 0
> n'est pas observable à son propre point de livraison.

### 0.3 L'échelle de résolution

Sa forme est dictée par une régression identifiée en analyse : résoudre un auteur **uniquement**
depuis le code authentifié transformerait en hexadécimal tous les membres arrivés par convergence de
roster (l'union `ghost-gmembers` ajoute des **codes bruts sans nom**). Le nom déclaré n'est donc pas
remplacé — **il devient un échelon** :

```
memberName(code, déclaré?) =
     alias[code]              // ce que J'AI choisi              ← nouveau (item 2)
  ?? ami(code).name           // ce que J'AI enregistré
  ?? clampLabel(déclaré)      // ce qu'IL prétend   ← marqué .unverified
  ?? S.fpCache[code]          // son empreinte
  ?? shortId(code)            // son code
```

**Sémantique du repli : « vide ⇒ on descend d'un cran »**, c'est-à-dire `||` et non `??`. Le nom
déclaré est piloté par le pair : une chaîne vide doit tomber sur l'échelon suivant, pas produire une
étiquette vide. Idem pour un alias enregistré à `""`.

**Deux fonctions, pas deux résolveurs :**

- **`memberName(code, declared?)`** — synchrone. **On garde le nom** : les 16 sites d'appel existants
  héritent sans churn, et `declared` est optionnel donc rétro-compatible site par site.
- **`ensureFpLabel(code)`** — asynchrone, **ne résout rien** : elle remplit `S.fpCache` puis
  redéclenche un rendu.
  - **Elle n'absorbe que les 3 barreaux bas — corrigé.** Les deux échelles ne sont pas identiques :
    celle de `session.ts` part du **nom d'ami** (son événement ne porte aucun nom déclaré), celle de
    `friends.ts` part du **nom déclaré** de la demande. Chaque site garde donc son premier barreau ;
    ce n'est pas un lever-et-supprimer.
  - **Garde d'idempotence obligatoire** : un `Set` des codes en vol + **cache négatif** en cas
    d'échec de l'`invoke`. Sans ça, comme elle est appelée *depuis* le rendu et qu'elle *redéclenche*
    le rendu, un échec produit une boucle d'`invoke`.
  - **Elle ne redéclenche jamais un re-rendu de liste** (`renderFriends`, `renderGroups`) : elle
    applique le libellé **en place** par le mécanisme de 0.4. C'est la convention #2 appliquée à
    elle-même.
  - Deux autres remplisseurs de `S.fpCache` existent déjà (`loadFingerprints` en préchargement,
    `showFp` qui n'écrit pas dans le cache) : les laisser tels quels, ne pas les fusionner dans ce lot.
- **`clampLabel(s)`** dans `dom.ts` : **40 caractères** puis élision. Appliquée à tout libellé
  d'origine distante.

### 0.4 Ré-étiquetage en place — et pourquoi surtout pas un re-rendu

**C'est la découverte la plus importante de la vérification.** Le mécanisme « évident » pour appliquer
un renommage rétroactivement — rappeler `renderGroupMsgs` — **détruit toutes les images du chat** :
cette fonction fait `clearImgBlobs(box)` puis `box.innerHTML = ""` et ne rejoue que `S.groupMsgs`, qui
**ne contient délibérément aucune image**. Les URL `blob:` sont révoquées au passage. Un implémenteur
qui câble « renommage → re-rendu » livre une régression qui mange les images.

Et le chat 1-à-1 n'a **aucun tampon** : `#chatLog` est append-only et n'est vidé qu'à la déconnexion.
Rien ne peut le re-rendre.

**Mécanisme retenu — estampillage :** au moment de l'insertion, toute étiquette d'auteur porte
`data-from="<code>"` (chat de groupe, chat 1-à-1, bulles d'image des deux côtés). Un renommage fait
alors :

```
document.querySelectorAll('[data-from="<code>"]') → réécrire textContent
```

Pas de re-rendu, pas d'image détruite, pas d'URL `blob:` révoquée, **et ça marche en PV et sur les
bulles d'image**, que le tampon ne couvre pas.

**`GroupMsg.from` reste néanmoins nécessaire**, pour un usage distinct : quand on change de groupe,
`renderGroupMsgs` **rejoue** le tampon, et le libellé doit alors être résolu à ce moment-là.

```ts
interface GroupMsg {
  author: string;      // nom déclaré brut, conservé comme échelon 3
  from?: string;       // ← nouveau : code authentifié de l'expéditeur
  text: string;
  who: string;         // INCHANGÉ — ne pas rétrécir en "me" | "them"
}
```

> **`who` reste `string` — corrigé.** La première rédaction le présentait comme `"me" | "them"` en le
> donnant pour l'existant ; c'était un second changement de type non annoncé qui aurait cassé
> `addGroupMsgDom`, `pushGroupMsg` et `renderGroupMsgs`, tous typés `string`.

### 0.5 Lire le `from` déjà émis — ce n'est pas une ligne

> **Corrigé, et c'était bloquant.** Déclarer `from?: string` sur `"ghost-gchat"` dans `tauri.ts`
> **ne suffit pas à compiler** une lecture de `p.from` : le listener re-déclare sa propre forme de
> charge utile en repli (`e.payload || ({} as { group?; author?; text? })`), et TypeScript refuse
> l'accès à une propriété absente d'un membre de l'union.

Sites minimaux : la déclaration dans `tauri.ts`, **le cast en ligne du listener** (idiome local qui
se répète sur quatre listeners voisins), puis les signatures de `pushGroupMsg`, `addGroupMsgDom` et
le rejeu de `renderGroupMsgs`. **≈ 5 sites, pas 1.** En revanche « aucun octet ne change sur le
fil » reste exact.

### 0.6 La règle de clé persistable

```ts
persistKey(code): string | null   // le code s'il est permanent, sinon null
```

Retourne `code` s'il est dans `loadFriends()` **ou** dans les membres d'un `loadGroups()`.

> **Justification corrigée.** La première rédaction disait que `connect` dial depuis l'endpoint
> éphémère « quand la cible n'est pas un ami », donc que `S.currentPeer` serait éphémère. C'est
> inversé : ce choix rend éphémère le code que **le pair distant** voit. Côté local, `S.currentPeer`
> n'est éphémère que si l'utilisateur a collé un code éphémère. La conclusion survit — **`S.currentPeer`
> n'est pas fiablement permanent** — mais quiconque raisonne sur le mécanisme d'origine conclurait à
> tort que « le côté qui accepte est toujours sûr ».

**Statut honnête** : sur les surfaces définies par ce lot, `persistKey` **ne peut jamais retourner
`null`** — on ne renomme que depuis l'item ami et la puce de membre, et on ne persiste de volume que
pour des participants d'appel de groupe ; tous sont déjà dans amis ∪ groupes. C'est donc une **garde
défensive, pas de la logique vivante**. Elle devient porteuse le jour où une surface de renommage
apparaît sur la conversation 1-à-1 — **délibérément hors de ce lot** (voir 2.6).

### 0.7 Limite assumée — le chat 1-à-1

`ghost-chat` ne transporte **aucune** identité d'expéditeur, et le spawn par flux de `run_conn` ne
clone jamais `peer`. L'auteur PV est donc résolu via `S.currentPeer` **à l'insertion** (puis
estampillé `data-from`, donc renommable ensuite).

**Course assumée** : lors d'un échange de session, un message encore en vol du pair sortant
s'afficherait sous l'alias du **nouveau** pair. Fenêtre étroite (le log est vidé à la déconnexion), et
strictement meilleur que l'état actuel où le nom est intégralement usurpable.

**Correctif propre, hors lot** : ~3 lignes de Rust (cloner `peer` dans le spawn `accept_bi`, ajouter
`"from"` aux emits `ghost-chat` / `ghost-chat-img`). C'est un changement de charge utile d'**événement
Tauri**, pas de fil. Il part avec la première release ultérieure qui touche Rust.

### 0.8 Découpage en commits

1. `state.ts` / `dom.ts` : `memberName(code, declared?)`, `clampLabel`, `persistKey`, `ensureFpLabel`
   (avec garde d'idempotence). Aucun appelant changé — **le comportement doit être identique.**
2. Catégorie B : router les 6 lectures de `f.name`.
3. Catégorie C + `index.html` : `from` de bout en bout, estampillage `data-from`, marquage
   `.unverified`, élision.
4. Catégorie D + E : factorisation de l'échelle d'empreinte, en-tête et logs PV.

### 0.9 Sécurité

- Alias et nom d'ami sont **locaux** : aucune donnée distante ne peut les atteindre.
- Le nom déclaré reste affiché mais est **élidé** et **marqué** — les deux manquent aujourd'hui.
- Le rendu passe déjà partout par `textContent` : pas de faille XSS, uniquement de l'usurpation
  visuelle.

### 0.10 Vérification

- **3 instances** — *test anti-régression principal* : un membre de groupe **non-ami** arrivé par
  convergence de roster continue d'afficher son nom déclaré (marqué), **pas** son code hexadécimal.
  *Substitut solo acceptable : forger un roster dans `localStorage` avec un code sans ami associé.*
- **Solo** : un nom déclaré de 500 caractères s'affiche élidé à 40 sans casser la mise en page.
- **Solo** : après le commit 1, l'application se comporte **exactement** comme avant.
- **2 instances** : un `#chatLog` PV contenant des images survit intact à un renommage (aucune image
  ne disparaît) — *c'est le test qui protège de la régression de 0.4.*

---

## Item 1 — Volumes par pair persistés

### 1.1 État actuel (vérifié) — le bug

- Le gain **voix** n'est poussé que depuis **un seul endroit** : le `oninput` du curseur de la puce de
  membre. **Confirmé.**
- Le gain **écran** passe par `applyStreamGain`, rappelée **à chaque création de tuile**. **Confirmé.**
- `S.groupGains` / `S.screenGains` / `S.screenMuted` ne sont **jamais vidées**. **Confirmé** — elles
  survivent donc déjà au raccrochage dans une même session ; seul le redémarrage les perd.
- **Le bug, confirmé** : `GroupCall::start` fait `p.clear()` sur la map du mixeur et
  `receive_group_voice` recrée chaque entrée à `1.0` via `or_insert`. Rien ne re-pousse le gain voix
  ⇒ régler à 130 %, raccrocher, rejoindre laisse **le curseur à 130 % pendant que le backend est à
  1.0**.
- Aucune clé `localStorage` de volume n'existe. *(Faux ami : `ghostlink_streams` est le nombre de flux
  de transfert de fichier.)*
- Le 🔇 est invisible hors survol. **Confirmé.**

### 1.2 Ce qui est stocké

```
ghostlink_gains   : Record<code, number>   // voix,  0..200
ghostlink_sgains  : Record<code, number>   // écran, 0..200
```

Deux clés séparées (même raisonnement que la séparation délibérée `stream_quality` / `stream_res`).

**Le mute n'est pas persisté** : le bouton étant invisible hors survol, un mute durable produit un
pair muet sans cause visible, des semaines plus tard. `S.screenMuted` reste en mémoire.

- **La persistance écrit sur `change`** (un curseur 0-200 pas de 5 produit jusqu'à 40 écritures par
  glissement). **L'`invoke` vers le backend reste sur `input`** — c'est ce qui rend le curseur
  réactif ; ne pas déplacer les deux ensemble.
- **Élagage** à l'enregistrement, aux codes présents dans amis ∪ membres de groupes.
- **Clamp et coercion en TS au chargement** : `set_gain` n'a **aucune borne** côté Rust (confirmé),
  contrairement à `set_screen_gain` qui borne le bas. `localStorage` est éditable par l'utilisateur.

### 1.3 Le re-push — le filtre est nommé

> **Corrigé, et c'était bloquant.** La première rédaction disait « chaque pair **réellement dans
> l'appel** » sans définir l'ensemble. Le seul signal « en appel » du front est `S.voiceAct`, alimenté
> par un événement émis après une temporisation et seulement pour les pairs dont la balise ~1 Hz est
> arrivée : **juste après le `await`, il est vide**. Un implémenteur filtrant dessus aurait livré un
> re-push qui ne pousse rien — un correctif silencieusement inopérant sur le bug titre.

```
await invoke("group_call_start", …)          ← GroupCall::start fait p.clear() ICI
   ↓  et seulement après
cibles = g.members.filter(c => S.meshOnline.has(c))
pour chaque cible dont le gain stocké ≠ 100 → invoke("group_call_volume")
```

`g.members ∩ S.meshOnline` est exactement l'ensemble que `net::group_conns` construit côté Rust, et
celui pour lequel `GroupCall::start` lance une tâche de réception. **Ces entrées existent déjà à 1.0
après le démarrage** — pousser dessus ne crée donc aucune entrée de map ; la crainte de croissance ne
concerne que les pairs *hors* de cet ensemble, que ce filtre exclut précisément.

**Après le `await`, jamais avant** : la commande attend le démarrage bloquant, donc à la résolution
de la promesse le `clear()` a eu lieu. Sûr vis-à-vis des tâches de réception lancées à l'intérieur,
qui utilisent `or_insert` et n'écrasent pas une entrée existante.

**Limite assumée — les arrivants tardifs.** Le re-push n'a lieu **qu'à l'entrée en appel**. Un pair
qui rejoint **après** moi obtient `or_insert` 1.0 et ne reçoit jamais mon gain stocké. Corriger cela
demanderait de re-pousser sur l'événement signalant un nouveau participant ; **hors périmètre de ce
lot**, et explicitement listé ici pour que la vérification 1.6 ne le maquille pas.

### 1.4 Best-effort, non négociable

Le re-push ne conditionne **rien**. Si l'`invoke` échoue, le son sort à 1.0 et la vie continue ; on ne
bloque ni la création d'une tuile, ni l'entrée en appel. `applyStreamGain` avale déjà ses erreurs et
doit continuer.

### 1.5 La soupape

Bouton **« Réinitialiser les volumes par pair »** dans Réglages. **Portée définie** (dans un item dont
le sujet est justement un désync curseur/backend, une définition partielle recréerait le bug) : il
vide les deux clés `localStorage`, **remet `S.groupGains`/`S.screenGains` à vide**, **re-pousse 1.0
au backend pour toutes les cibles de 1.3 si un appel est en cours**, et **rafraîchit les curseurs
affichés**.

### 1.6 Vérification — coût réel

> **Corrigé** : les deux étapes annoncées « solo » sont impossibles. Le curseur de voix n'existe que
> pendant un appel actif, et l'appel refuse de démarrer sans membre en ligne ; le curseur d'écran
> n'existe que sur la tuile d'un pair **distant**.

- **2 instances** — *test du bug corrigé* : régler un pair à 130 %, raccrocher, rejoindre → le son est
  **réellement** à 130 %.
- **2 instances** : couper le son d'un partage, redémarrer → le pair est **audible** (mute non
  persisté, par conception).
- **2 instances** : injecter une valeur aberrante dans `localStorage`, rejoindre → clampée.
- **Solo, partiel** : vérifier après redémarrage que les clés sont relues et clampées (inspection de
  `S`), sans pouvoir observer de curseur.
- **2 instances** : « Réinitialiser » pendant un appel → le son revient à 100 % **et** les curseurs
  se replacent.

---

## Item 2 — Renommage local (alias)

### 2.1 État actuel (vérifié)

- Aucune notion d'alias, de surnom ou de renommage nulle part. **Confirmé.**
- `saveMutual` fait `f.name = name.trim()` : accepter une demande d'ami **écrase le nom stocké
  localement**. **Confirmé.**
- **Aucun contrôle d'unicité** des noms. **Confirmé** — deux pairs peuvent afficher le même pseudo.
- Les noms d'amis sont **ajout seul** : rien ne permet d'éditer un nom après création. **Confirmé.**
- `showTile` / `showCanvasTile` n'écrivent l'étiquette que dans la branche de **création**.
  **Confirmé.**
- `log()` empile des nœuds texte immuables. **Confirmé.**

### 2.2 Une map séparée, et c'est structurel

```
ghostlink_aliases : Record<code, string>
```

**Pas** un champ `Friend.alias` :

1. Les **membres de groupe non-amis** n'ont pas d'entrée `Friend` — et ce sont précisément les gens
   qu'on veut le plus étiqueter.
2. Une suppression d'ami ou un kick détruirait l'étiquette.
3. Le clobber de `saveMutual` la réécrirait.

Avec une map séparée et l'alias au sommet de l'échelle, **le clobber devient inoffensif par
construction**. `saveMutual` n'est donc **pas modifiée** — la map séparée *est* le correctif.

*Effet de bord assumé* : le nom saisi à l'ajout d'un ami reste écrasable par le pair. Réponse
utilisateur : « renomme-le », et l'alias tient.

### 2.3 Durée de vie — les alias ne sont PAS élagués

> **Corrigé : la première rédaction se contredisait trois fois.** Elle donnait « une suppression d'ami
> détruirait l'étiquette » comme *raison* de la map séparée, puis prescrivait un élagage à la
> suppression d'ami, puis posait comme test d'acceptation « supprimer puis re-ajouter un ami →
> l'alias survit ». Les trois sont incompatibles.

**Décision : aucun élagage sur suppression d'ami, sortie de groupe ou kick.** Un alias est une
étiquette humaine, minuscule, et sa valeur est précisément de survivre à ces événements. Le seul
garde-fou est un **plafond de 256 entrées** (les plus anciennes tombent), sur le modèle du plafond à
64 de `ghostlink_pending_freq_out`.

*Les volumes (item 1), eux, sont élagués* : ils n'ont de sens que pour des gens qu'on appelle. **Les
deux items ne partagent pas cette décision** ; ne pas uniformiser.

### 2.4 Où l'on renomme

Un **✏️ révélé au survol**, sur deux surfaces, réutilisant les patterns existants : l'**item ami**
(mécanique du ✕ de suppression) et la **puce de membre de groupe** (à côté du 🚫) — seul chemin pour
étiqueter un non-ami.

**Édition en ligne** : le libellé devient un `<input>`, Entrée valide, Échap annule. Pas de nouvelle
modale, pas de dépendance à `prompt()`.

**Deux points que cela impose :**

- **L'édition en cours vit dans `S`, pas dans le DOM.** La puce de membre est dans `#groupList`, que
  `refreshGroupCounts` re-rend à **chaque** changement de présence : une saisie en cours serait
  détruite. `S.editingAlias: string | null` (le code en cours d'édition) est réappliqué au rendu.
  C'est la convention #2, sur l'item qui l'aurait violée le plus discrètement.
- **Le contrat DOM n'est plus « ajout seul » sur ces deux nœuds** : on y échange un libellé contre un
  `<input>`. Aucun identifiant ni classe existants n'est renommé, mais la nuance est notée ici parce
  que le résumé de fin de document ne peut plus dire « ajout seul » sans réserve.

### 2.5 Ce qui reste après le spine

L'item 0 fait que les catégories A, B et C héritent de l'alias. Il reste :

1. **Les tuiles vidéo vivantes** : l'étiquette n'étant écrite qu'à la création, un renommage en cours
   d'appel laisse l'ancien nom. Passe de ré-étiquetage à ajouter.
2. **Le déclenchement du ré-étiquetage `data-from`** de 0.4 à la validation d'un alias.
3. Le stockage, l'UI ✏️ et le plafond de 2.3.

> **Corrigé** : « il reste exactement deux choses » et « l'alias apparaît partout, y compris sur les
> messages déjà à l'écran » ne tenaient que grâce au mécanisme de re-rendu — celui qui mange les
> images. Avec l'estampillage `data-from` de 0.4, le critère redevient atteignable **y compris en PV
> et sur les bulles d'image**, ce que le tampon `S.groupMsgs` n'aurait jamais permis.

**Limite assumée** : `log()` empile des nœuds texte immuables — les lignes de journal déjà écrites
gardent l'ancien nom. *(Elles ne sont pas estampillées : les estampiller reviendrait à réécrire un
journal, ce qui n'est pas son rôle.)*

### 2.6 Question tranchée — les pairs 1-à-1 non-amis

**Non, ce lot ne permet pas de renommer un pair 1-à-1 non-ami.** Les deux entrées de renommage sont
l'item ami et la puce de membre. C'est la raison pour laquelle `persistKey` (0.6) ne peut jamais
retourner `null` ici, et c'est assumé : renommer un pair `TEMP` dont le code est éphémère produirait
une étiquette qui disparaît à la rotation. Le jour où une entrée de renommage apparaît sur la
conversation 1-à-1, `persistKey` devient porteuse et doit **refuser** avec un message explicite.

### 2.7 Sécurité

C'est le vrai cadrage : **une feature de sécurité, pas de confort.** Les noms distants sont des
chaînes auto-déclarées, sans unicité, bornées seulement par un cadre de 64 Kio, sans élision, et
réécrivables par le pair via `saveMutual`. Un alias local au sommet de l'échelle ferme les quatre au
niveau affichage.

**Contre-risque à ne pas créer** : si l'UI affiche indifféremment « le nom que j'ai choisi » et « le
nom qu'il prétend », l'utilisateur réapprend à faire confiance aux noms. Le marquage `.unverified`
fait partie intégrante de la valeur de sécurité de l'item.

### 2.8 Vérification

- **Solo** : renommer un ami → l'alias apparaît sur toutes les surfaces A/B/C.
- **2 instances** : renommer pendant qu'un chat de groupe **contenant des images** est affiché → les
  auteurs changent, **aucune image ne disparaît**. *Test anti-régression de 0.4.*
- **2 instances** : renommer pendant un appel avec partage d'écran → l'étiquette de la tuile vivante
  change.
- **3 instances** : renommer un membre de groupe **non-ami** depuis sa puce.
- **Solo** : supprimer puis re-ajouter un ami → **l'alias survit** (cohérent avec 2.3).
- **2 instances** : accepter une demande d'ami d'un pair renommé → l'alias survit au `saveMutual`.
- **2 instances** : provoquer un changement de présence pendant une saisie d'alias sur une puce →
  la saisie survit.

---

## Item 3 — Durcissements, son, pastilles

**Livré en trois sous-phases indépendantes.** 3a n'a aucune dépendance sur 3b/3c : ce sont trois
corrections de bugs préexistants avec leurs propres tests multi-instances. Un problème sur le WebAudio
ne doit pas retenir la correction d'une alarme qui sonne toutes les minutes.

### 3a — Trois failles existantes (à corriger AVANT de brancher un son)

Ces failles existent déjà. Le son ne fait que les rendre audibles ; les livrer sans correctif
produirait une app que des inconnus et d'anciens membres peuvent faire sonner.

| Faille (vérifiée) | Effet une fois le son branché | Correctif |
|---|---|---|
| La boucle de retry d'invitation ré-émet **toutes les 60 s indéfiniment** pour un pair resté en ligne (la purge n'a lieu qu'au `ghost-mesh-up`, qui ne refire pas), et le receveur ré-affiche la bannière à chaque fois | **une alarme par minute** jusqu'à action | purger l'entrée après un envoi réussi **et** dédupliquer côté receveur par `(gid, from)` — état **persisté** sous `ghostlink_invseen`, sinon un redémarrage ré-arme l'alarme ; vidé quand le groupe est rejoint ou refusé |
| Le handler `ghost-gchat` ne vérifie que l'existence du **groupe**, pas l'appartenance de l'**expéditeur** — alors que les handlers de roster et de kick, eux, la vérifient ; et `applyKick` ne touche jamais `Settings.friends`, donc le mesh continue d'accepter l'exclu | **un membre exclu fait toujours sonner ton PC** et incrémente ta pastille | ajouter le contrôle d'appartenance déjà en place ailleurs : `from` doit être un membre connu du groupe |
| `ghost-incoming` est émis **avant toute autorisation applicative** : le pair est cryptographiquement authentifié, mais le filtre « amis seulement » est **désactivé par défaut** et l'anti-flood de 2 s est indexé sur l'identité — que faire tourner ses clés suffit à contourner (le code le dit en commentaire) | **un inconnu peut mitrailler le haut-parleur** | ne **sonner** que si le pair est un ami ; la bannière ne change pas de comportement |

> **Corrigé** : la première rédaction disait « avant toute **authentification** ». C'est faux et ça
> affaiblit l'argument en l'exagérant — la poignée de main QUIC/TLS est terminée et `remote_id()` est
> une identité cryptographique. Ce qui manque est l'**autorisation**.

**Vérification 3a** — **3 instances** : exclure un membre par vote, le faire écrire → aucun effet chez
les autres. **2 instances** : laisser une invitation en attente 3 minutes avec le pair en ligne → la
bannière n'apparaît **qu'une fois** ; redémarrer le receveur → toujours une seule.

### 3b — Le son

**Oscillateur WebAudio, zéro fichier.** Le CSP a `media-src 'self' blob:` **sans `data:`** : une
data-URI passée à `new Audio()` serait refusée. Un `.wav` embarqué marcherait, mais un oscillateur ne
demande ni asset, ni réflexion CSP.

> **L'autoplay n'est PAS acquis — corrigé, et c'était bloquant.** La première rédaction affirmait que
> « l'app fait déjà jouer de la vidéo sonore sans geste utilisateur ». **L'app ne joue jamais aucun
> son via la WebView** : `getUserMedia` est appelé avec `audio: false`, `getDisplayMedia` en vidéo
> seule (délibérément, c'est l'anti-écho), et tout le son réel sort de cpal dans le processus hôte.
> Les `<video>` autoplay un flux **silencieux**. De plus Chromium soumet `AudioContext` à une
> politique **distincte** de `<video autoplay>` : un contexte créé au chargement du module peut
> démarrer `suspended` et le rester indéfiniment. **Un implémenteur suivant l'ancienne rédaction
> livrait un `playPing()` sans son et sans erreur.**

**Conception imposée par ce correctif** : `AudioContext` **créé paresseusement** et `resume()` appelé
depuis le **premier geste utilisateur réel** (n'importe quel clic — onglet, ouverture de groupe,
Réglages ; il y en a partout). Tant qu'aucun geste n'a eu lieu, `playPing` est un no-op silencieux
assumé.

- **`playPing(kind)` dans `dom.ts`**, à côté de `log()`.
- **Limiteur de débit PAR TIMBRE, pas global** — corrigé : un limiteur au niveau module serait
  partagé entre les trois timbres, donc un ping de message une seconde avant un appel entrant
  **ferait taire l'appel**, ce qui contredit la règle ci-dessous. 1 ping / 2 s **par `kind`**.
- **Trois timbres** : message / demande d'ami / **appel entrant**.
- **L'appel entrant se répète toutes les ~5 s tant que la bannière est affichée** (≈ 6 pings sur ses
  30 s de vie). Un ping unique à t=0 n'améliorerait quasiment pas le statu quo si l'utilisateur n'est
  pas devant l'écran — or c'est exactement le problème que ce timbre existe pour résoudre.
- **Défaut : muet pendant un appel de groupe** pour les messages ; **jamais muet** pour un appel
  entrant ou une demande d'ami.
- **Interdiction absolue** de brancher un son sur un événement haute fréquence : l'activité vocale est
  poussée à ~10 Hz et la présence vocale à 1 Hz **par membre**.
- Interrupteur **Son de notification** dans Réglages, persisté sous `ghostlink_sound`, sur le modèle
  de la case « partage d'écran natif ».

**Vérification 3b** — **Solo** : **déclencher un ping AVANT tout clic dans la fenêtre** (le
`resume()` doit se faire au premier clic ; le test existe pour attraper le contexte `suspended`).
**Solo** : rafale de 20 messages → au plus un ping par 2 s. **2 instances** : un ping de message une
seconde avant un appel entrant → **l'appel sonne quand même**.

### 3c — Les pastilles

**Les compteurs vivent dans `S`, jamais dans le DOM** : `renderGroups` vide `#groupList` en
`innerHTML` et `refreshGroupCounts` le re-rend à chaque changement de présence.

```ts
S.unread     : Record<string, number>   // par gid
S.unread1to1 : number
S.focused    : boolean                  // init true
```

**Incrément**, au point de passage unique `pushGroupMsg` :

```
who !== "me" && (S.openGroupId !== id || !S.focused)
```

> **Le `who !== "me"` est obligatoire — corrigé.** `pushGroupMsg` est **aussi** le chemin de mes
> propres messages sortants (`who = "me"`). Sans ce terme, mon propre message incrémente ma propre
> pastille dès que la fenêtre a perdu le focus — un invariant qui dépend d'une coïncidence.

**Remise à zéro**, aux points de passage uniques, avec des portées explicites :

- `openGroup(id)` → efface `S.unread[id]` **uniquement**.
- `showTab("session")` → efface `S.unread1to1` **uniquement**. *Entrer dans l'onglet Groupes n'efface
  rien* : effacer tous les compteurs à l'entrée d'onglet viderait les pastilles de groupes jamais
  ouverts, ce qui annule la fonctionnalité.
- **`tauri://focus` → efface le compteur de la conversation actuellement affichée.** Sans ce point,
  un message arrivé fenêtre floue sur le groupe déjà ouvert laisse une pastille que l'utilisateur ne
  peut effacer qu'en naviguant ailleurs puis en revenant — alors qu'il est en train de lire le
  message.

**Focus** : `tauri://focus` / `tauri://blur`. Déjà autorisés par les capabilities actuelles (l'app
écoute déjà les 4 événements de glisser-déposer par le même mécanisme) ; il manque **2 déclarations
de type**.

**Rendu** : réutilisation de la classe `.badge` existante, ajoutée au layout flex de `.item`. CSS
additif.

**Pas de persistance des compteurs** : l'historique de groupe est en mémoire seule ; un compteur
survivant au redémarrage pointerait vers des messages qui n'existent plus.

**Le cas des images — décision tranchée.** Les images de groupe sont **jetées** quand le groupe n'est
pas ouvert. Compter donne une pastille qui s'ouvre sur rien ; ne pas compter fait qu'une image envoyée
seule ne notifie jamais. **Décision : compter, et pousser une ligne de remplacement dans le tampon**
(« 🖼️ *Nom* a envoyé une image »). La pastille devient honnête, et cela rend visible une perte de
données déjà existante. *(C'est le seul point où 3c dépend de l'item 0 : la résolution du nom.)*

**Vérification 3c** — **2 instances** : réduire la fenêtre, se faire écrire → pastille ; ouvrir →
effacée. **Solo** : provoquer un changement de présence pendant qu'une pastille est affichée → **elle
survit** au re-rendu de la liste (*test anti-régression principal* ; un `ghost-mesh-down` suffit,
obtenable en fermant l'autre instance). **2 instances** : recevoir une image dans un groupe fermé →
pastille **et** ligne de remplacement à l'ouverture. **2 instances** : envoyer un message soi-même
fenêtre floue → **aucune** pastille.

### 3d — Hors périmètre : release B

**Toasts Windows, icône de zone de notification, flash de la barre des tâches.** `tauri-plugin-notification`
est absent du `Cargo.toml` **et** du `Cargo.lock` (vérifié) — nouvelle dépendance et nouvel arbre
transitif dans un binaire dont l'interop Media Foundation / COM a déjà été cassée une fois par un
changement de build global. L'icône de zone de notification exige de **changer le jeu de features du
crate `tauri` lui-même** (vérifié : `features = []`). Enfin `release.ps1` **demande interactivement le
mot de passe de la clé de signature** (vérifié) — l'assistant ne peut pas le lancer.

*Hypothèse de plateforme non vérifiable depuis ce dépôt* : un toast Windows exige en général une app
installée avec un raccourci portant l'AppUserModelID, donc ne se valide honnêtement que depuis un
build signé installé. À confirmer au moment de la release B, pas avant.

Quand elle arrivera : **le flash de la barre des tâches d'abord** (l'API existe déjà dans la version
de Tauri utilisée et, pilotée depuis une commande Rust maison, elle ne coûte aucune modification de
capability), et **contenu du toast = nom de l'expéditeur seulement par défaut**, jamais le texte — un
toast apparaît sur l'écran de verrouillage, dans l'historique du Centre de notifications et dans tout
enregistrement d'écran.

---

## Coût de vérification (à lire avant de planifier)

| Item | Solo | 2 instances | 3 instances |
|---|---|---|---|
| 0 | élision, non-régression du commit 1 | images survivant au renommage | **le test principal** (membre non-ami) |
| 1 | inspection de `S` seulement | **le test du bug**, mute, clamp, reset | — |
| 2 | alias sur les surfaces locales, survie à la suppression | images, tuile vivante, `saveMutual`, saisie | renommage d'un non-ami |
| 3a | — | boucle d'invitation | **membre exclu** |
| 3b | ping avant tout clic, limiteur | priorité des timbres | — |
| 3c | survie de la pastille au re-rendu | pastilles, images, message propre | — |

**Aucun de ces tests n'est couvert par `cargo test`** (aucun code Rust n'est touché). Prévoir une
session à 3 instances pour les trois tests principaux.

## Notes de release

- Version cible **v0.36.0** : bumper les **4** emplacements synchronisés — `package.json`,
  `src-tauri/Cargo.toml`, `src-tauri/tauri.conf.json`, `UI_BUILD` dans `ui/src/main.ts` — puis
  `.\scripts\release.ps1`. Seulement quand les 4 items sont livrés et vérifiés.
- **Ne jamais republier un numéro de version déjà distribué.**
- Mettre à jour la liste **exhaustive** des clés `localStorage` dans `CLAUDE.md`.

## Récapitulatif des changements de contrat

- **Fil (QUIC)** : **aucun**. Pas de nouveau `KIND`/`GKIND`, pas de charge utile modifiée, pas de
  nouveau flux. Un pair v0.35.5 et un pair v0.36.0 échangent des octets **identiques** dans les deux
  sens.
- **Commandes Tauri** : **aucune** nouvelle. `group_call_volume` et `screen_audio_gain` existent déjà
  et sont déjà typées.
- **Événements** : **aucun** nouveau. **Trois** déclarations de type ajoutées : `from?` sur
  `ghost-gchat` (déjà émis par Rust), `tauri://focus`, `tauri://blur`.
- **Capabilities Tauri** : **aucun** changement. **CSP** : **aucun** changement.
- **Contrat DOM** : ajouts (nouveaux identifiants ✏️ / réinitialisation / cases de Réglages ;
  classes `.unverified`, `.badge.unread` ; attribut `data-from` sur les étiquettes d'auteur), **plus
  une substitution locale** libellé → `<input>` pendant l'édition d'un alias (voir 2.4). **Aucun
  identifiant ni classe existants renommés.**
- **`localStorage`** — **cinq** clés ajoutées, toutes nommées :
  `ghostlink_aliases`, `ghostlink_gains`, `ghostlink_sgains`, `ghostlink_invseen`, `ghostlink_sound`.

---

## Annexe — pourquoi les médias en PV sont écartés (release D)

Consigné pour que la décision ne soit pas re-litigée sans les chiffres. Le pipeline natif **n'est pas
lié au mesh** (`VideoShare::start` prend un simple `Vec<(String, Connection)>`), ce qui rend l'idée
séduisante. Mais le seau à jetons qui relaie les trames vers la WebView est créé **par connexion**, et
deux flux simultanés sont explicitement autorisés sur une même connexion :

```
débit vidéo max        ≈ 1 875 000 o/s par partage
RELAY_RATE             = 3 670 016 o/s par CONNEXION
2 × 1 875 000          = 3 750 000  >  3 670 016
```

Un pair qui partage à un groupe **et** en PV sur la même connexion dépasse le seau. Une trame perdue
force l'attente de la keyframe suivante — la plus grosse — perdue à son tour : **les deux partages
noirs en permanence, avec des statistiques émetteur parfaites**. C'est exactement la panne v0.35.4, et
le test-garde existant modélise **un seul flux** : il passerait au vert.

La caméra en PV est écartée séparément et durablement : il **n'existe pas** de capture caméra native
(les cibles de partage sont écran ou fenêtre uniquement), donc la caméra force le chemin WebRTC +
STUN + exposition d'IP sur la surface la plus sensible de l'app — celle qui accepte justement les
inconnus.
