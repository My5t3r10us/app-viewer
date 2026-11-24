# Feuille de route

État d'App Viewer : ce qui est livré, ce qui est en cours, et des pistes pour la suite. Le détail du
fonctionnement est dans le [README](README.md).

## Fait (jusqu'à la v0.7.2)

**Application**
- Mini-apps web locales regroupées dans une fenêtre Tauri 2, une WebView2 par app : l'état des pages
  est conservé quand on passe d'une app à l'autre.
- Lancement via `cmd /c` dans le dossier de l'app, avec `PORT=<port>` (désactivable par app) et
  `BROWSER=none` ; attente du port avant affichage.
- *Job Object* Windows par app : « Arrêter » tue tout l'arbre de process, et fermer App Viewer (même
  en cas de crash) arrête toutes les apps lancées.
- Apps « Externe » (port déjà occupé au démarrage) et bouton « Reprendre la main ».
- Apps natives (exe ou commande de dev) dont la fenêtre est incrustée dans App Viewer.
- CPU et RAM par app dans la barre d'outils.
- Barre d'adresse, zoom mémorisé par app, téléchargements (`Ctrl+J`), panneau Logs détachable.
- Paramètres, dont « Garder App Viewer actif en arrière-plan » (zone de notification).
- Fenêtre sans barre de titre, sidebar redimensionnable et masquable, raccourcis clavier.

**CLI `appv`**
- `list`, `show`, `add` (nom, commande et port devinés depuis `package.json` et `.env`), `edit`,
  `rm`, `open` (avec `--replace`), `stop`, `config`.
- `screenshot` : capture de la page d'une app ou de toute la fenêtre.
- `ui` : lecture et pilotage de la page d'une app par un agent (snapshot, clic, saisie, touches,
  défilement, navigation, attente), avec opt-in par app depuis l'interface et canal *named pipe*
  réservé à l'utilisateur.
- `agents install|uninstall` : règle et skill pour Claude Code et Codex.
- Installé avec App Viewer et ajouté au PATH de l'utilisateur.
- v0.7.2 : `appv` lance App Viewer détaché de la session de l'agent qui l'appelle.

## En cours : v0.7.3 (publication et outillage)

- Dépendances mises à jour (compatibles semver) et `cargo audit`.
- Tests du CLI `appv` et de `config.rs` (détection d'`appv add`, résolution d'une app,
  `appv agents install|uninstall` dans un dossier temporaire).
- `cargo clippy --all-targets -- -D warnings` sans avertissement.
- Cette feuille de route.
- CI GitHub Actions sur Windows (tests, clippy, audit, build de l'installeur) et GitHub Release sur
  les tags `v*`.
- Passage en 0.7.3, puis tags `v0.7.2` et `v0.7.3` et release publiée par la CI.

## Pistes pour la suite

Rien n'est planifié : ce sont des sujets ouverts, à trier.

- Étendre les tests aux parties de l'interface qui n'en ont pas encore (lancement des commandes,
  rechargement d'`apps.json`, vues WebView2).
- Signature de code de l'installeur, aujourd'hui non signé.
- Limite connue du canal `appv ui` : un process tiers du même utilisateur peut parler au pipe (voir
  le README).
- Idées et demandes à recueillir à l'usage, puis à reporter ici.
