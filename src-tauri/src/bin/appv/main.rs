//! `appv` : gère en ligne de commande les applications d'App Viewer.
//!
//! Lit et écrit le même `apps.json` que l'interface (qui recharge le fichier dès qu'il change).
//! Lancer ou arrêter une app passe par `app-viewer.exe --open|--stop <id>` : le plugin
//! single-instance transmet la demande à l'instance en cours, ou en démarre une.

#[path = "../../config.rs"]
#[allow(dead_code)] // shared with the GUI, which uses the rest
mod config;
mod agents;
#[path = "../../ipc_common.rs"]
mod ipc_common;
mod ipc;
mod win;

use clap::{Args, Parser, Subcommand};
use config::{AppConfig, AppKind};
use std::{
    env,
    io::{self, BufRead, IsTerminal, Write},
    net::{Ipv4Addr, Ipv6Addr, SocketAddr, TcpStream},
    path::{Path, PathBuf},
    process::{Command, ExitCode},
    thread,
    time::{Duration, Instant},
};

/// Same as `identifier` in tauri.conf.json: Tauri's app_config_dir is %APPDATA%\<identifier>.
/// A test build of both binaries (TAURI_CONFIG + APPV_IDENTIFIER set at compile time) uses its own
/// identifier, hence its own config folder, and leaves the real one alone.
const IDENTIFIER: &str = match option_env!("APPV_IDENTIFIER") {
    Some(id) => id,
    None => "com.cbm.appviewer",
};
const GUI_EXE: &str = "app-viewer.exe";

#[derive(Parser)]
#[command(
    name = "appv",
    version,
    about = "Gère les applications d'App Viewer : lister, ajouter, modifier, supprimer, lancer.",
    after_help = "<APP> : nom (ou début du nom), numéro affiché par `appv list`, ou identifiant.\n\
                  Exemples :\n  appv add C:\\Code\\mon-app --port 3000\n  appv open mon-app\n  appv edit 2 --port 4000"
)]
struct Cli {
    #[command(subcommand)]
    cmd: Cmd,
}

#[derive(Subcommand)]
enum Cmd {
    /// Liste les applications enregistrées et indique si leur port répond
    #[command(visible_alias = "ls")]
    List {
        /// Sortie JSON
        #[arg(long)]
        json: bool,
    },
    /// Affiche le détail d'une application
    Show {
        app: String,
        /// Sortie JSON
        #[arg(long)]
        json: bool,
    },
    /// Enregistre une application. Nom, commande et port sont devinés depuis le dossier
    /// (package.json, .env) quand ils ne sont pas précisés
    Add {
        /// Dossier de l'application (dossier courant par défaut)
        dir: Option<PathBuf>,
        #[command(flatten)]
        fields: Fields,
    },
    /// Modifie une application (seuls les champs précisés changent)
    Edit {
        app: String,
        /// Nouveau dossier
        #[arg(long)]
        path: Option<PathBuf>,
        #[command(flatten)]
        fields: Fields,
    },
    /// Supprime une application (l'arrête si elle tourne)
    #[command(visible_alias = "rm")]
    Remove {
        app: String,
        /// Ne pas demander de confirmation
        #[arg(short, long)]
        yes: bool,
    },
    /// Lance une application et l'affiche dans App Viewer (démarre App Viewer si besoin)
    #[command(visible_aliases = ["start", "run"])]
    Open {
        app: String,
        /// Rendre la main tout de suite, sans attendre que le port réponde
        #[arg(long)]
        no_wait: bool,
        /// Attente maximale du port, en secondes
        #[arg(long, default_value_t = 120)]
        timeout: u64,
        /// Si le port est déjà occupé par un process lancé hors d'App Viewer, l'arrête (lui et ses
        /// sous-process) avant de lancer l'app. Jamais par défaut.
        #[arg(long)]
        replace: bool,
    },
    /// Arrête une application lancée par App Viewer
    Stop { app: String },
    /// Capture d'écran : enregistre un PNG et affiche son chemin (à lire avec l'outil d'image de l'agent)
    Screenshot {
        /// Application à capturer (facultative s'il n'y en a qu'une en ligne, ou avec --window)
        app: Option<String>,
        /// Capture toute la fenêtre d'App Viewer (sidebar et barre d'outils comprises)
        #[arg(long)]
        window: bool,
        /// Sortie JSON : chemin, dimensions, app, URL
        #[arg(long)]
        json: bool,
    },
    /// Affiche le chemin du fichier de configuration (apps.json)
    Config,
    /// Règle et skill pour que Claude Code et Codex utilisent appv
    Agents {
        #[command(subcommand)]
        action: AgentsAction,
    },
    /// Pilote la page d'une app (lire, cliquer, saisir) sans navigateur headless ni script PowerShell.
    /// L'utilisateur doit d'abord autoriser l'app : bouton « robot » de la barre d'outils d'App Viewer
    Ui {
        #[command(subcommand)]
        action: UiAction,
    },
    /// Vérifie que le canal de contrôle d'App Viewer répond
    #[command(hide = true)]
    Ping,
    /// Débogage (builds debug d'App Viewer uniquement) : exécute du JS dans l'interface d'App Viewer
    #[command(hide = true)]
    DebugUi { script: String },
    /// Débogage (builds debug uniquement) : un appel du protocole DevTools sur la page d'une app
    #[command(hide = true)]
    DebugCdp { app: String, method: String, params: String },
    /// Utilisé par l'installeur : ajoute le dossier d'appv au PATH de l'utilisateur
    #[command(hide = true)]
    InstallPath,
    /// Utilisé par le désinstalleur : retire le dossier d'appv du PATH de l'utilisateur
    #[command(hide = true)]
    UninstallPath,
}

#[derive(Subcommand)]
enum UiAction {
    /// Liste les éléments de la page (boutons, champs, liens, textes) avec des références e1, e2…
    Snapshot { app: String },
    /// Texte brut de la page
    Text { app: String },
    /// Clique sur un élément du dernier snapshot
    Click {
        app: String,
        #[arg(value_name = "REF")]
        element: String,
    },
    /// Saisit du texte dans un champ (remplace le contenu, sauf --append)
    Type {
        app: String,
        #[arg(value_name = "REF")]
        element: String,
        text: String,
        /// Valide ensuite avec Entrée (soumet le formulaire)
        #[arg(long)]
        submit: bool,
        /// Ajoute au contenu au lieu de le remplacer
        #[arg(long)]
        append: bool,
    },
    /// Choisit une option d'une liste déroulante (par sa valeur ou son libellé)
    Select {
        app: String,
        #[arg(value_name = "REF")]
        element: String,
        value: String,
    },
    /// Envoie une touche (Enter, Escape, Tab, Shift+Tab, ArrowDown, Ctrl+a…)
    Press { app: String, key: String },
    /// Fait défiler la page (--to top|bottom|REF, ou --by <pixels> ; sinon 80 % d'écran vers le bas)
    Scroll {
        app: String,
        #[arg(long)]
        to: Option<String>,
        #[arg(long, allow_negative_numbers = true)]
        by: Option<i64>,
    },
    /// Va à un chemin de l'app (/admin) ou à une URL http(s)
    Navigate { app: String, target: String },
    /// Attend qu'un texte apparaisse (ou disparaisse avec --gone)
    Wait {
        app: String,
        #[arg(long)]
        text: String,
        #[arg(long)]
        gone: bool,
        /// Attente maximale, en secondes (60 au plus)
        #[arg(long, default_value_t = 10)]
        timeout: u64,
    },
}

#[derive(Subcommand)]
enum AgentsAction {
    /// Écrit la règle (~/.claude/CLAUDE.md, ~/.codex/AGENTS.md) et la skill `app-viewer`
    Install,
    /// Retire la règle et la skill
    Uninstall,
}

#[derive(Args)]
struct Fields {
    /// Nom affiché
    #[arg(short, long)]
    name: Option<String>,
    /// Commande de lancement (exécutée via cmd /c dans le dossier, avec PORT=<port> sauf --set-port=false)
    #[arg(short, long)]
    command: Option<String>,
    /// Port sur lequel l'app répond
    #[arg(short, long)]
    port: Option<u16>,
    /// Chemin de la page à afficher (par défaut /)
    #[arg(short, long)]
    url_path: Option<String>,
    /// Lancer l'app au démarrage d'App Viewer (--autostart, ou --autostart=false)
    #[arg(long, num_args = 0..=1, default_missing_value = "true", value_name = "BOOL")]
    autostart: Option<bool>,
    /// Transmettre PORT=<port> à la commande (par défaut oui ; --set-port=false si le .env du
    /// projet fixe PORT pour un autre de ses serveurs)
    #[arg(long, num_args = 0..=1, default_missing_value = "true", value_name = "BOOL")]
    set_port: Option<bool>,
    /// App de bureau (exe ou commande de dev) : sa fenêtre est incrustée dans App Viewer, pas de port. App
    /// Viewer la lance et l'arrête seulement (--native, ou --native=false)
    #[arg(long, num_args = 0..=1, default_missing_value = "true", value_name = "BOOL")]
    native: Option<bool>,
}

type Res<T = ()> = Result<T, String>;

fn main() -> ExitCode {
    let cli = Cli::parse();
    match run(cli.cmd) {
        Ok(()) => ExitCode::SUCCESS,
        Err(e) => {
            eprintln!("Erreur : {e}");
            ExitCode::FAILURE
        }
    }
}

fn run(cmd: Cmd) -> Res {
    match cmd {
        Cmd::List { json } => list(json),
        Cmd::Show { app, json } => show(&app, json),
        Cmd::Add { dir, fields } => add(dir, fields),
        Cmd::Edit { app, path, fields } => edit(&app, path, fields),
        Cmd::Remove { app, yes } => remove(&app, yes),
        Cmd::Open { app, no_wait, timeout, replace } => open(&app, no_wait, timeout, replace),
        Cmd::Stop { app } => stop(&app),
        Cmd::Screenshot { app, window, json } => screenshot(app.as_deref(), window, json),
        Cmd::Config => {
            println!("{}", config_path()?.display());
            Ok(())
        }
        Cmd::Agents { action: AgentsAction::Install } => agents::install(),
        Cmd::Agents { action: AgentsAction::Uninstall } => agents::uninstall(),
        Cmd::Ui { action } => ui(action),
        Cmd::DebugCdp { app, method, params } => {
            let (_, apps) = load()?;
            let id = apps[find(&apps, &app)?].id.clone();
            let params: serde_json::Value = serde_json::from_str(&params).map_err(|e| e.to_string())?;
            println!("{}", ipc::request("debug-cdp", serde_json::json!({ "app": id, "method": method, "params": params }))?);
            Ok(())
        }
        Cmd::DebugUi { script } => {
            let reply = ipc::request("debug-eval-ui", serde_json::json!({ "script": script }))?;
            println!("{}", reply["value"]);
            Ok(())
        }
        Cmd::Ping => {
            let reply = ipc::request("ping", serde_json::json!({}))?;
            println!("{reply}");
            Ok(())
        }
        Cmd::InstallPath => {
            let dir = exe_dir()?;
            if win::add_to_user_path(&dir)? {
                println!("{} ajouté au PATH (nouveaux terminaux).", dir.display());
            }
            Ok(())
        }
        Cmd::UninstallPath => {
            let dir = exe_dir()?;
            if win::remove_from_user_path(&dir)? {
                println!("{} retiré du PATH.", dir.display());
            }
            Ok(())
        }
    }
}

// ---------------------------------------------------------------- config

fn config_path() -> Res<PathBuf> {
    let appdata = env::var_os("APPDATA").ok_or("variable APPDATA absente")?;
    Ok(PathBuf::from(appdata).join(IDENTIFIER).join("apps.json"))
}

fn load() -> Res<(PathBuf, Vec<AppConfig>)> {
    let path = config_path()?;
    let apps = config::try_load(&path)?;
    Ok((path, apps))
}

/// Finds an app by id, list number, exact name, then unique name prefix / substring.
fn find(apps: &[AppConfig], key: &str) -> Res<usize> {
    let key = key.trim();
    if let Some(i) = apps.iter().position(|a| a.id == key) {
        return Ok(i);
    }
    if let Ok(n) = key.parse::<usize>() {
        if (1..=apps.len()).contains(&n) {
            return Ok(n - 1);
        }
    }
    let low = key.to_lowercase();
    if let Some(i) = apps.iter().position(|a| a.name.to_lowercase() == low) {
        return Ok(i);
    }
    let matchers: [fn(&str, &str) -> bool; 2] = [|name, k| name.starts_with(k), |name, k| name.contains(k)];
    for matches in matchers {
        let found: Vec<usize> = (0..apps.len()).filter(|&i| matches(&apps[i].name.to_lowercase(), &low)).collect();
        match found.as_slice() {
            [i] => return Ok(*i),
            [] => {}
            many => {
                let names: Vec<&str> = many.iter().map(|&i| apps[i].name.as_str()).collect();
                return Err(format!("« {key} » est ambigu : {}", names.join(", ")));
            }
        }
    }
    Err(format!("aucune application « {key} » (voir `appv list`)"))
}

fn check_unique_name(apps: &[AppConfig], cfg: &AppConfig) -> Res {
    if apps.iter().any(|a| a.id != cfg.id && a.name.eq_ignore_ascii_case(&cfg.name)) {
        return Err(format!("une application s'appelle déjà « {} » (choisissez un autre --name)", cfg.name));
    }
    Ok(())
}

fn warn_port(apps: &[AppConfig], cfg: &AppConfig) {
    if cfg.is_native() {
        return;
    }
    if let Some(other) = apps.iter().find(|a| a.id != cfg.id && !a.is_native() && a.port == cfg.port) {
        eprintln!("Attention : le port {} est aussi celui de « {} ».", cfg.port, other.name);
    }
}

fn absolute(dir: &Path) -> Res<String> {
    let abs = std::path::absolute(dir).map_err(|e| format!("{} : {e}", dir.display()))?;
    Ok(abs.display().to_string())
}

// ---------------------------------------------------------------- commands

fn list(json: bool) -> Res {
    let (_, apps) = load()?;
    if json {
        let rows: Vec<_> = apps
            .iter()
            .map(|a| {
                let mut v = serde_json::to_value(a).unwrap();
                v["url"] = a.url().into();
                v["up"] = is_up(a).into();
                v
            })
            .collect();
        println!("{}", serde_json::to_string_pretty(&rows).unwrap());
        return Ok(());
    }
    if apps.is_empty() {
        println!("Aucune application. Ajoutez-en une : appv add <dossier>");
        return Ok(());
    }
    let rows: Vec<[String; 6]> = apps
        .iter()
        .enumerate()
        .map(|(i, a)| {
            let state = if is_up(a) { "● en ligne" } else { "○ arrêtée" };
            let name = if a.autostart { format!("{} ↻", a.name) } else { a.name.clone() };
            [(i + 1).to_string(), name, port_label(a), state.into(), a.command.clone(), a.path.clone()]
        })
        .collect();
    print_table(&["#", "NOM", "PORT", "ÉTAT", "COMMANDE", "DOSSIER"], &rows);
    if apps.iter().any(|a| a.autostart) {
        println!("\n↻ = lancée au démarrage d'App Viewer");
    }
    Ok(())
}

fn print_table<const N: usize>(header: &[&str; N], rows: &[[String; N]]) {
    let width = |s: &str| s.chars().count();
    let mut widths = header.map(width);
    for row in rows {
        for (w, cell) in widths.iter_mut().zip(row) {
            *w = (*w).max(width(cell));
        }
    }
    let line = |cells: Vec<&str>| {
        let mut out = String::new();
        for (i, cell) in cells.iter().enumerate() {
            out.push_str(cell);
            if i + 1 < N {
                out.push_str(&" ".repeat(widths[i] - width(cell) + 2));
            }
        }
        println!("{}", out.trim_end());
    };
    line(header.to_vec());
    for row in rows {
        line(row.iter().map(String::as_str).collect());
    }
}

fn describe(a: &AppConfig) {
    let state = if is_up(a) { "en ligne" } else { "arrêtée" };
    println!("{}", a.name);
    println!("  id         {}", a.id);
    println!("  dossier    {}", a.path);
    println!("  commande   {}", a.command);
    println!("  type       {}", if a.is_native() { "native (fenêtre incrustée)" } else { "web" });
    if !a.is_native() {
        println!("  port       {}", a.port);
        println!("  url        {}", a.url());
        println!("  env PORT   {}", if a.set_port_env { "transmis" } else { "non transmis" });
    }
    println!("  autostart  {}", if a.autostart { "oui" } else { "non" });
    println!("  état       {state}");
}

fn show(key: &str, json: bool) -> Res {
    let (_, apps) = load()?;
    let a = &apps[find(&apps, key)?];
    if json {
        let mut v = serde_json::to_value(a).unwrap();
        v["url"] = a.url().into();
        v["up"] = is_up(a).into();
        println!("{}", serde_json::to_string_pretty(&v).unwrap());
    } else {
        describe(a);
    }
    Ok(())
}

fn add(dir: Option<PathBuf>, f: Fields) -> Res {
    let (path, mut apps) = load()?;
    let dir = match dir {
        Some(d) => d,
        None => env::current_dir().map_err(|e| e.to_string())?,
    };
    let dir = absolute(&dir)?;
    let native = f.native.unwrap_or(false);
    let guess = config::inspect_folder(Path::new(&dir));
    let command = if native {
        f.command.or_else(|| config::guess_native_command(Path::new(&dir)))
    } else {
        f.command.or(guess.command)
    };
    let mut cfg = AppConfig {
        id: config::new_id(),
        name: f.name.or(guess.name).unwrap_or_default(),
        path: dir,
        command: command.ok_or("commande introuvable dans le dossier : précisez --command")?,
        port: if native { 0 } else { f.port.or(guess.port).ok_or("port non détecté : précisez --port")? },
        kind: if native { AppKind::Native } else { AppKind::Web },
        url_path: f.url_path.unwrap_or_else(|| "/".into()),
        autostart: f.autostart.unwrap_or(false),
        set_port_env: f.set_port.unwrap_or(true),
    };
    cfg.validate()?;
    check_unique_name(&apps, &cfg)?;
    warn_port(&apps, &cfg);
    apps.push(cfg.clone());
    config::save(&path, &apps)?;
    println!("✔ Application ajoutée (n° {}) :\n", apps.len());
    describe(&cfg);
    println!("\nLancer : appv open \"{}\"", cfg.name);
    Ok(())
}

fn edit(key: &str, dir: Option<PathBuf>, f: Fields) -> Res {
    let (path, mut apps) = load()?;
    let i = find(&apps, key)?;
    let mut cfg = apps[i].clone();
    let before = cfg.clone();
    if let Some(d) = dir {
        cfg.path = absolute(&d)?;
    }
    if let Some(v) = f.name {
        cfg.name = v;
    }
    if let Some(v) = f.command {
        cfg.command = v;
    }
    if let Some(v) = f.port {
        cfg.port = v;
    }
    if let Some(v) = f.url_path {
        cfg.url_path = v;
    }
    if let Some(v) = f.set_port {
        cfg.set_port_env = v;
    }
    if let Some(v) = f.autostart {
        cfg.autostart = v;
    }
    if let Some(v) = f.native {
        cfg.kind = if v { AppKind::Native } else { AppKind::Web };
    }
    cfg.validate()?;
    if cfg == before {
        println!("Rien à modifier (précisez --name, --command, --port, --url-path, --path, --autostart ou --set-port).");
        return Ok(());
    }
    check_unique_name(&apps, &cfg)?;
    warn_port(&apps, &cfg);
    apps[i] = cfg.clone();
    config::save(&path, &apps)?;
    println!("✔ Application modifiée :\n");
    describe(&cfg);
    if (cfg.command != before.command || cfg.path != before.path || cfg.port != before.port || cfg.kind != before.kind) && is_up(&before) {
        println!("\nL'app tourne : relancez-la dans App Viewer pour appliquer la nouvelle commande.");
    }
    Ok(())
}

fn remove(key: &str, yes: bool) -> Res {
    let (path, mut apps) = load()?;
    let i = find(&apps, key)?;
    let name = apps[i].name.clone();
    if !yes {
        if !io::stdin().is_terminal() {
            return Err("confirmation impossible hors terminal : ajoutez --yes".into());
        }
        print!("Supprimer « {name} » ? [o/N] ");
        io::stdout().flush().ok();
        let mut answer = String::new();
        io::stdin().lock().read_line(&mut answer).map_err(|e| e.to_string())?;
        if !matches!(answer.trim().to_lowercase().as_str(), "o" | "oui" | "y" | "yes") {
            println!("Annulé.");
            return Ok(());
        }
    }
    apps.remove(i);
    config::save(&path, &apps)?;
    println!("✔ « {name} » supprimée.");
    Ok(())
}

fn open(key: &str, no_wait: bool, timeout: u64, replace: bool) -> Res {
    let (_, apps) = load()?;
    let app = &apps[find(&apps, key)?];
    if app.is_native() {
        send_to_gui("--open", &app.id)?;
        if no_wait {
            println!("Lancement de « {} » (app native).", app.name);
            return Ok(());
        }
        let started = Instant::now();
        while started.elapsed() < Duration::from_secs(timeout.min(15)) {
            thread::sleep(Duration::from_millis(400));
            match gui_status(&app.id).as_deref() {
                Some("running") => {
                    println!("✔ « {} » lancée, sa fenêtre est affichée dans App Viewer.", app.name);
                    return Ok(());
                }
                Some("error") => break,
                _ => {}
            }
        }
        return Err(format!("« {} » ne s'est pas lancée : consultez ses logs dans App Viewer (Ctrl+L)", app.name));
    }
    if replace && port_open(app.port) {
        ensure_gui()?;
        let reply = ipc::request("takeover", serde_json::json!({ "app": app.id }))?;
        println!("■ Process externe arrêté : {}", reply["killed"].as_str().unwrap_or("?"));
    }
    send_to_gui("--open", &app.id)?;
    if no_wait {
        println!("Ouverture de « {} » dans App Viewer.", app.name);
        return Ok(());
    }
    if port_open(app.port) {
        println!("✔ « {} » est affichée dans App Viewer ({}).", app.name, app.url());
        return Ok(());
    }
    print!("Démarrage de « {} »…", app.name);
    io::stdout().flush().ok();
    let started = Instant::now();
    while started.elapsed() < Duration::from_secs(timeout) {
        thread::sleep(Duration::from_millis(400));
        if port_open(app.port) {
            println!(" prête ({:.1}s).", started.elapsed().as_secs_f32());
            println!("✔ Affichée dans App Viewer : {}", app.url());
            return Ok(());
        }
    }
    println!();
    Err(format!(
        "le port {} ne répond pas après {timeout}s : consultez les logs de l'app dans App Viewer (Ctrl+L)",
        app.port
    ))
}

fn stop(key: &str) -> Res {
    let (_, apps) = load()?;
    let app = &apps[find(&apps, key)?];
    if !win::process_running(GUI_EXE) {
        println!("App Viewer n'est pas lancé : « {} » ne tourne pas sous App Viewer.", app.name);
        return Ok(());
    }
    send_to_gui("--stop", &app.id)?;
    let started = Instant::now();
    while started.elapsed() < Duration::from_secs(10) {
        if !is_up(app) {
            println!("✔ « {} » arrêtée.", app.name);
            return Ok(());
        }
        thread::sleep(Duration::from_millis(300));
    }
    println!("Arrêt demandé, mais le port {} répond encore (app lancée hors d'App Viewer ?).", app.port);
    Ok(())
}

/// One action on the page of an app, through App Viewer's control pipe.
fn ui_request(key: &str, action: &str, extra: serde_json::Value) -> Res<serde_json::Value> {
    let (_, apps) = load()?;
    let id = apps[find(&apps, key)?].id.clone();
    let mut msg = match extra {
        serde_json::Value::Object(map) => map,
        _ => serde_json::Map::new(),
    };
    msg.insert("app".into(), id.into());
    msg.insert("action".into(), action.into());
    ipc::request("ui", serde_json::Value::Object(msg))
}

/// "Page : <url> — <title>" when the reply says where the page is now.
fn page_line(reply: &serde_json::Value) -> Option<String> {
    let page = reply.get("page")?;
    let url = page.get("url")?.as_str()?;
    Some(format!("Page : {url} — {}", page.get("title").and_then(|t| t.as_str()).unwrap_or("")))
}

fn ui(action: UiAction) -> Res {
    use serde_json::json;
    let s = |r: &serde_json::Value, k: &str| r.get(k).and_then(|v| v.as_str()).unwrap_or_default().to_string();
    let reply;
    match action {
        UiAction::Snapshot { app } => {
            reply = ui_request(&app, "snapshot", json!({}))?;
            println!("{}", s(&reply, "text"));
        }
        UiAction::Text { app } => {
            reply = ui_request(&app, "text", json!({}))?;
            println!("{}", s(&reply, "text"));
        }
        UiAction::Click { app, element } => {
            reply = ui_request(&app, "click", json!({ "ref": element }))?;
            println!("✔ clic sur {}", s(&reply, "desc"));
            if let Some(over) = reply.get("covered").and_then(|c| c.as_str()) {
                println!("⚠ l'élément est recouvert par : {over}");
            }
        }
        UiAction::Type { app, element, text, submit, append } => {
            reply = ui_request(&app, "type", json!({ "ref": element, "text": text, "submit": submit, "append": append }))?;
            println!("✔ saisie dans {}{}", s(&reply, "desc"), if submit { " + Entrée" } else { "" });
        }
        UiAction::Select { app, element, value } => {
            reply = ui_request(&app, "select", json!({ "ref": element, "value": value }))?;
            println!("✔ {} → {}", s(&reply, "desc"), s(&reply, "value"));
        }
        UiAction::Press { app, key } => {
            reply = ui_request(&app, "press", json!({ "key": key }))?;
            println!("✔ touche {}", s(&reply, "desc"));
        }
        UiAction::Scroll { app, to, by } => {
            reply = ui_request(&app, "scroll", json!({ "to": to, "by": by }))?;
            println!("✔ défilement : {} / {} px", reply["y"], reply["max"]);
        }
        UiAction::Navigate { app, target } => {
            reply = ui_request(&app, "navigate", json!({ "target": target }))?;
            println!("✔ navigation demandée");
        }
        UiAction::Wait { app, text, gone, timeout } => {
            reply = ui_request(&app, "wait", json!({ "text": text, "gone": gone, "timeout": timeout }))?;
            println!("✔ {} après {} ms", if gone { "disparu" } else { "présent" }, reply["waited"]);
        }
    }
    if let Some(line) = page_line(&reply) {
        println!("{line}");
    }
    Ok(())
}

fn screenshot(key: Option<&str>, window: bool, json: bool) -> Res {
    let (_, apps) = load()?;
    let id = match key {
        Some(k) => Some(apps[find(&apps, k)?].id.clone()),
        None if window => None,
        None => {
            let online: Vec<&AppConfig> = apps.iter().filter(|a| port_open(a.port)).collect();
            match online.as_slice() {
                [only] => Some(only.id.clone()),
                [] => return Err("aucune application en ligne : précisez laquelle (appv list) ou lancez-en une (appv open)".into()),
                many => {
                    let names: Vec<&str> = many.iter().map(|a| a.name.as_str()).collect();
                    return Err(format!("plusieurs applications en ligne ({}) : précisez laquelle", names.join(", ")));
                }
            }
        }
    };
    let reply = ipc::request("screenshot", serde_json::json!({ "app": id, "window": window }))?;
    if json {
        println!("{}", serde_json::to_string_pretty(&reply).unwrap());
    } else {
        println!("{}", reply["path"].as_str().ok_or("réponse sans chemin")?);
    }
    Ok(())
}

// ---------------------------------------------------------------- helpers

/// Makes sure App Viewer is running and its control pipe answers (starts it if needed).
fn ensure_gui() -> Res {
    match ipc::request("ping", serde_json::json!({})) {
        Err(e) if e == ipc::NOT_RUNNING => {}
        other => return other.map(|_| ()),
    }
    let gui = exe_dir()?.join(GUI_EXE);
    if !gui.is_file() {
        return Err(format!("{} introuvable (appv doit être à côté d'App Viewer)", gui.display()));
    }
    win::allow_foreground();
    Command::new(&gui).spawn().map_err(|e| format!("lancement d'App Viewer impossible : {e}"))?;
    let started = Instant::now();
    while started.elapsed() < Duration::from_secs(20) {
        thread::sleep(Duration::from_millis(300));
        if ipc::request("ping", serde_json::json!({})).is_ok() {
            return Ok(());
        }
    }
    Err("App Viewer n'a pas démarré à temps".into())
}

fn exe_dir() -> Res<PathBuf> {
    let exe = env::current_exe().map_err(|e| e.to_string())?;
    Ok(exe.parent().ok_or("dossier d'appv introuvable")?.to_path_buf())
}

/// Hands a request to App Viewer: the running instance gets it, or a new one starts with it.
fn send_to_gui(action: &str, id: &str) -> Res {
    let gui = exe_dir()?.join(GUI_EXE);
    if !gui.is_file() {
        return Err(format!("{} introuvable (appv doit être à côté d'App Viewer)", gui.display()));
    }
    win::allow_foreground();
    Command::new(&gui)
        .args([action, id])
        .spawn()
        .map_err(|e| format!("lancement d'App Viewer impossible : {e}"))?;
    Ok(())
}

/// Status of an app as App Viewer sees it ("running", "stopped"…), if it is running.
fn gui_status(id: &str) -> Option<String> {
    if !win::process_running(GUI_EXE) {
        return None;
    }
    let reply = ipc::request("status", serde_json::json!({ "app": id })).ok()?;
    reply["status"].as_str().map(str::to_string)
}

/// A web app is up when its port answers; a native one when App Viewer runs it.
fn is_up(a: &AppConfig) -> bool {
    if a.is_native() {
        gui_status(&a.id).as_deref() == Some("running")
    } else {
        port_open(a.port)
    }
}

fn port_label(a: &AppConfig) -> String {
    if a.is_native() { "native".into() } else { a.port.to_string() }
}

fn port_open(port: u16) -> bool {
    let timeout = Duration::from_millis(200);
    [SocketAddr::from((Ipv4Addr::LOCALHOST, port)), SocketAddr::from((Ipv6Addr::LOCALHOST, port))]
        .iter()
        .any(|addr| TcpStream::connect_timeout(addr, timeout).is_ok())
}
