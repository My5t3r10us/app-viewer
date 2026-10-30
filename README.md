# App Viewer

Application Windows native (Tauri 2 + WebView2) qui regroupe vos mini-apps web locales.
Pour chaque app vous indiquez **dossier**, **commande de lancement** et **port** : App Viewer
lance la commande, attend que le port réponde, puis affiche l'app dans une WebView2 native
intégrée à la fenêtre.

## Fonctionnement

- Chaque app tourne dans sa propre WebView2 : passer d'une app à l'autre conserve l'état des pages.
- La commande est exécutée via `cmd /c` dans le dossier de l'app, avec `PORT=<port>` (option par app :
  à décocher si le `.env` du projet fixe PORT pour un autre serveur, `dotenv` n'écrasant pas une
  variable déjà définie ; CLI : `--set-port=false`) et
  `BROWSER=none` dans l'environnement. Sortie standard / erreur visibles dans le panneau Logs.
- Chaque process est placé dans un *Job Object* Windows : « Arrêter » tue tout l'arbre
  (cmd → node → …) et fermer App Viewer (même en cas de crash) arrête toutes les apps lancées.
  La commande est lancée suspendue et n'est reprise qu'une fois dans le job (aucun sous-process
  ne peut y échapper) ; en quittant, App Viewer attend que chaque job soit vide.
- La barre d'outils affiche le CPU (en % de la machine, comme le Gestionnaire des tâches) et la
  RAM (working set) de l'app, rafraîchis toutes les 1,5 s : tout l'arbre de process pour une app
  lancée par App Viewer, seulement le process qui écoute le port pour une app « Externe ».
- Barre d'adresse : elle suit la page affichée (navigation interne de l'app comprise). On y tape
  un chemin (`/admin`) ou une URL complète puis Entrée (`F6` pour y aller) ; les chemins déjà
  visités sont proposés. Le bouton à droite copie l'URL complète, et « Ouvrir dans le
  navigateur » ouvre la page courante.
- Zoom : `Ctrl` + molette, `Ctrl +` / `Ctrl -` / `Ctrl 0` dans la page (géré par WebView2). Le facteur est
  mémorisé par app (`settings.json`) et réappliqué au prochain lancement ; un indicateur « 125 % » avec
  − / + et remise à 100 % apparaît dans la barre d'outils dès que la page n'est pas à 100 %.
- Téléchargements (`Ctrl+J`, ou l'icône en bas de la sidebar) : liste des fichiers téléchargés depuis les
  apps avec leur statut (en cours, terminé, échec), ouverture du fichier ou de son dossier. Les fichiers vont
  dans le dossier Téléchargements de Windows ; l'historique (100 entrées) est dans `downloads.json`.
- Terminal : le panneau Logs se redimensionne par son bord haut et peut être **détaché** dans une
  fenêtre à part pendant que la commande tourne. Elle se ferme d'elle-même quand la commande se
  termine normalement (délai réglable), et reste ouverte en cas d'erreur.
- Paramètres (`Ctrl+,` ou « Réglages » en bas de la sidebar) : option **« Garder App Viewer actif en
  arrière-plan »** — fermer la fenêtre la masque dans la zone de notification, les apps continuent
  de tourner ; « Quitter » depuis l'icône arrête tout. Relancer l'exe réaffiche l'instance existante.
- Si le port répond déjà au démarrage, l'app est simplement affichée (statut « Externe »),
  sans être gérée par App Viewer. Le bouton **« Reprendre la main »** (barre d'outils) arrête alors le
  process qui écoute sur ce port (il en affiche le nom et le PID, un second clic confirme) puis relance
  l'app depuis App Viewer. Seul ce process (et ses sous-process) est visé ; App Viewer lui-même, les
  process système et le dossier Windows sont refusés. CLI : `appv open <app> --replace`.
- Les apps de type `ai-usage` qui ouvrent le navigateur toutes seules doivent être lancées avec
  leur option équivalente à `--no-open`.

Configuration : `%APPDATA%\com.cbm.appviewer\apps.json` et `settings.json` (modifiables depuis
l'interface).

Fenêtre sans barre de titre : on la déplace par la barre d'outils ou l'en-tête de la sidebar
(double-clic = agrandir), les boutons réduire / agrandir / fermer apparaissent en approchant
la souris du coin supérieur droit. Les bords restent redimensionnables.

Sidebar : largeur réglable par son bord droit (double-clic = largeur par défaut, la glisser
presque à zéro la masque), masquable (`Ctrl+B`). Masquée, elle réapparaît par-dessus l'app au
survol du bord gauche (désactivable dans les paramètres) ; l'app glisse sans être redimensionnée.

Raccourcis : `Ctrl+N` ajouter, `Ctrl+1..9` changer d'app, `Ctrl+L` logs, `Ctrl+R` recharger,
`Ctrl+B` sidebar, `Ctrl+J` téléchargements, `Ctrl+,` paramètres, `Ctrl+=` / `Ctrl+-` / `Ctrl+0` zoom.

## CLI `appv`

L'installeur ajoute aussi `appv.exe` (à côté d'`app-viewer.exe`) et place ce dossier dans le PATH
de l'utilisateur (retiré à la désinstallation ; ouvrir un nouveau terminal après l'installation).

```bash
appv list                                  # apps enregistrées + port en ligne ou non (--json)
appv show mon-app                          # détail (--json)
appv add C:\Code\mon-app                   # nom / commande / port devinés (package.json, .env)
appv add . --name API --command "npm run dev" --port 3000 --autostart
appv add C:\Code\mon-outil --native         # app de bureau (exe ou dev Tauri/tinyjs) : fenêtre incrustée dans App Viewer, pas de port
appv edit mon-app --port 4000              # seuls les champs précisés changent (--path, -n, -c, -p, -u, --autostart=false, --set-port=false)
appv rm mon-app                            # demande confirmation (-y pour s'en passer)
appv open mon-app                          # lance l'app et l'affiche dans App Viewer (démarré si besoin)
appv stop mon-app
appv open mon-app --replace                # port pris par un process hors App Viewer : l'arrête puis lance l'app
appv screenshot mon-app                    # capture de la page, affiche le chemin du PNG (--window : toute la fenêtre, --json)
appv ui snapshot mon-app                   # éléments de la page (réf. e1, e2…) ; puis ui click / type / select / press / scroll / navigate / wait / text
appv config                                # chemin d'apps.json
appv agents install                        # règle + skill pour Claude Code et Codex (uninstall pour retirer)
```

**Captures d'écran.** `appv screenshot [app] [--window] [--json]` enregistre un PNG dans
`%LOCALAPPDATA%\App Viewer\screenshots\` (dossier modifiable via `screenshotsDir` dans `settings.json`) et
n'affiche que son chemin : un agent le lit avec son outil d'image. La page de l'app est capturée par WebView2 ;
`--window` assemble l'interface (sidebar, barre d'outils) et la page, même si une autre fenêtre les recouvre.
Les 50 dernières images de moins de 24 h sont conservées.

**Contrôle d'une app par un agent (`appv ui`).** Pour tester une app locale sans navigateur headless, port de
debug ni script PowerShell (que l'antivirus / EDR signale), l'agent lit la page (`snapshot`), clique, saisit,
envoie des touches, défile, navigue et attend un texte, avec de vrais événements souris / clavier (protocole
DevTools embarqué de WebView2, appelé en processus : aucun port de debug n'est ouvert). Garde-fous :
- **Opt-in par app, depuis l'interface uniquement** : bouton robot de la barre d'outils, désactivé par défaut,
  valable jusqu'à la fermeture d'App Viewer. Aucune commande `appv` ne peut l'activer, et `apps.json` ne
  contient rien à ce sujet.
- Canal : *named pipe* `\\.\pipe\app-viewer-<SID>` réservé à l'utilisateur courant (DACL), clients distants
  refusés, première instance exclusive, requêtes plafonnées à 256 Ko, délai de 10 s. Pas de port TCP : les apps
  hébergées ne peuvent pas l'atteindre. `appv` n'envoie rien si le pipe n'est pas servi par l'`app-viewer.exe`
  installé à côté de lui.
- Actions en liste fermée sur des éléments désignés par référence ; **pas d'exécution de code fourni par
  l'agent** ; `navigate` limité à http(s). Les mots de passe sont masqués dans le snapshot et les logs.
- Visibilité : chaque action est écrite dans les logs de l'app (lignes `🤖`) et un bandeau « Agent en
  contrôle » s'affiche dans la barre d'outils.
- Limite connue : un process tiers du même utilisateur peut parler au pipe ; il reste borné par l'opt-in par
  app et par cette liste d'actions.

**Agents IA.** La règle ([agents/rules/AGENTS.md](agents/rules/AGENTS.md)) et la skill `app-viewer`
([agents/skills/app-viewer/SKILL.md](agents/skills/app-viewer/SKILL.md)) sont embarquées dans
`appv` à la compilation. `appv agents install` écrit la règle entre des marqueurs
`<!-- app-viewer:start/end -->` dans `~/.claude/CLAUDE.md` et `~/.codex/AGENTS.md`, et la skill dans
`~/.claude/skills/app-viewer/` et `~/.codex/skills/app-viewer/`. La commande est idempotente, et
`uninstall` rend les fichiers à l'identique. L'installeur lance l'une, le désinstalleur l'autre. Après
avoir modifié ces sources, recompiler puis relancer `appv agents install`.

Une app se désigne par son nom (ou un début de nom non ambigu), son numéro dans `appv list` ou
son id. `appv` modifie directement `apps.json` ; l'interface ouverte le recharge dans la seconde
(une app supprimée est arrêtée). `open` / `stop` passent par `app-viewer.exe --open|--stop <id>`,
transmis à l'instance en cours par le plugin single-instance ; `open` attend ensuite que le port
réponde (`--no-wait`, `--timeout <s>`).

## Développement

Prérequis : Rust (toolchain MSVC), Node (uniquement pour la CLI Tauri : icônes et installeur).

```bash
npm install
npm run dev          # cargo run (debug)
npm run build        # exe release + installeur NSIS dans src-tauri/target/release/bundle
```

`cargo build --release --manifest-path src-tauri/Cargo.toml` suffit pour obtenir
`src-tauri/target/release/app-viewer.exe` (autonome, l'UI est embarquée).

Structure :

- `ui/` : interface (HTML/CSS/JS sans build) ; `terminal.html` = fenêtre de logs détachée,
  `shared.js` = helpers communs
- `src-tauri/src/main.rs` : commandes, icône de notification, fermeture / sortie, arguments
  `--open` / `--stop` et rechargement d'`apps.json` modifié par le CLI
- `src-tauri/src/bin/appv/` : CLI `appv` (partage `config.rs`) ; `win.rs` = PATH utilisateur
- `src-tauri/windows/hooks.nsh` : hooks de l'installeur NSIS (PATH, règle et skill des agents)
- `agents/` : sources de la règle et de la skill pour les agents IA (embarquées dans `appv`)
- `src-tauri/src/settings.rs` : `settings.json` (page Paramètres)
- `src-tauri/src/stats.rs` : échantillonnage CPU / RAM des apps (job ou process du port)
- `src-tauri/src/runner.rs` : lancement des commandes, logs, détection du port
- `src-tauri/src/ipc.rs`, `ipc_common.rs`, `bin/appv/ipc.rs` : canal CLI → GUI (named pipe sécurisé)
- `src-tauri/src/capture.rs` : captures d'écran ; `agent.rs` + `agent.js` : actions `appv ui` sur la page
- `src-tauri/src/downloads.rs` : historique des téléchargements ; `procs.rs` : reprise d'un port occupé
- `src-tauri/src/views.rs` : une WebView2 enfant par app, positionnée sur la zone de contenu
- `src-tauri/src/job.rs` : Job Object Windows (arrêt de l'arbre de process)
- `src-tauri/src/config.rs` : `apps.json` et détection auto (package.json, port)
