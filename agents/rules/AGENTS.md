## App Viewer : apps web locales (règle globale)
- Les mini-apps web locales de l'utilisateur sont enregistrées dans App Viewer et se gèrent
  avec le CLI `appv` (dans le PATH ; sinon `%LOCALAPPDATA%\App Viewer\appv.exe`).
- Lancer / afficher une app enregistrée → `appv open <app>` (démarre App Viewer si besoin,
  attend que le port réponde). Arrêter → `appv stop <app>`. État → `appv list --json`.
- Avant de lancer à la main le serveur d'un projet (`npm run dev`, `node server.js`…), vérifie
  avec `appv list --json` s'il est enregistré : si oui, passe par `appv open` / `appv stop`
  au lieu d'ouvrir un second process sur le même port.
- Voir ou tester une app locale → `appv screenshot <app>` (affiche le chemin d'un PNG : lis-le avec ton
  outil d'image) et `appv ui snapshot|click|type|press|scroll|navigate|wait <app> …` (l'utilisateur doit
  d'abord autoriser l'app avec le bouton robot d'App Viewer ; s'il est refusé, demande-le-lui, ne
  contourne pas). Pas de navigateur headless, de port de debug, de Playwright/Puppeteer ni de script
  PowerShell pour ça : l'antivirus (AMSI / EDR) les bloque.
- Port déjà pris par un process hors App Viewer (statut « Externe ») : `appv open <app> --replace`
  l'arrête (lui seul, jamais un autre) puis lance l'app ; seulement si l'utilisateur est d'accord.
- Enregistrer un projet → `appv add <dossier>` (nom, commande et port devinés ; sinon
  `--name`, `--command`, `--port`). Modifier → `appv edit <app> --champ valeur`.
- Interdit : éditer `apps.json` à la main. Toujours passer par `appv`.
- `appv rm <app> -y` seulement sur demande explicite de l'utilisateur.
- Détails : skill `app-viewer`.
