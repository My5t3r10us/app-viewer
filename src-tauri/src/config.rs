use serde::{Deserialize, Serialize};
use std::{fs, path::Path};

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct AppConfig {
    #[serde(default)]
    pub id: String,
    pub name: String,
    pub path: String,
    pub command: String,
    /// Unused (0) for a native app.
    #[serde(default)]
    pub port: u16,
    #[serde(default)]
    pub kind: AppKind,
    #[serde(default = "default_url_path")]
    pub url_path: String,
    #[serde(default)]
    pub autostart: bool,
    /// Pass PORT=<port> to the command. Off for projects whose own config (.env…) sets PORT
    /// for another of their servers: dotenv does not override a variable that is already set.
    #[serde(default = "default_true")]
    pub set_port_env: bool,
}

/// "web": a local server shown in an embedded WebView2. "native": a desktop program (exe or dev
/// command) that opens its own windows; App Viewer only launches and stops it.
#[derive(Clone, Copy, Debug, Default, Serialize, Deserialize, PartialEq)]
#[serde(rename_all = "lowercase")]
pub enum AppKind {
    #[default]
    Web,
    Native,
}

fn default_true() -> bool {
    true
}

fn default_url_path() -> String {
    "/".into()
}

impl AppConfig {
    pub fn is_native(&self) -> bool {
        self.kind == AppKind::Native
    }

    pub fn url(&self) -> String {
        let path = self.url_path.trim();
        let path = if path.starts_with('/') { path.to_string() } else { format!("/{path}") };
        format!("http://localhost:{}{}", self.port, path)
    }

    /// Trims the fields and checks them. Shared by the UI and the `appv` CLI.
    pub fn validate(&mut self) -> Result<(), String> {
        self.name = self.name.trim().to_string();
        self.path = self.path.trim().to_string();
        self.command = self.command.trim().to_string();
        if self.name.is_empty() || self.command.is_empty() {
            return Err("Le nom et la commande sont obligatoires.".into());
        }
        if !Path::new(&self.path).is_dir() {
            return Err(format!("Le dossier « {} » n'existe pas.", self.path));
        }
        if self.is_native() {
            self.port = 0;
        } else if self.port == 0 {
            return Err("Port invalide.".into());
        }
        Ok(())
    }
}

/// Last modification time of apps.json, to notice edits made outside the UI (the CLI).
pub fn modified(path: &Path) -> Option<std::time::SystemTime> {
    fs::metadata(path).and_then(|m| m.modified()).ok()
}

#[derive(Default, Serialize, Deserialize)]
struct ConfigFile {
    #[serde(default)]
    apps: Vec<AppConfig>,
}

/// Like `load`, but an unreadable file is an error instead of an empty list (for the CLI,
/// which must not overwrite it).
pub fn try_load(path: &Path) -> Result<Vec<AppConfig>, String> {
    let text = match fs::read_to_string(path) {
        Ok(text) => text,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(Vec::new()),
        Err(e) => return Err(format!("{} : {e}", path.display())),
    };
    serde_json::from_str::<ConfigFile>(&text)
        .map(|file| file.apps)
        .map_err(|e| format!("{} illisible : {e}", path.display()))
}

pub fn load(path: &Path) -> Vec<AppConfig> {
    if !path.exists() {
        return Vec::new();
    }
    match try_load(path) {
        Ok(apps) => apps,
        Err(e) => {
            // Keep the unreadable file aside rather than overwriting it on the next save.
            let _ = fs::copy(path, path.with_extension("invalid.json"));
            eprintln!("{e} ; copie dans apps.invalid.json");
            Vec::new()
        }
    }
}

pub fn save(path: &Path, apps: &[AppConfig]) -> Result<(), String> {
    if let Some(dir) = path.parent() {
        fs::create_dir_all(dir).map_err(|e| e.to_string())?;
    }
    let json = serde_json::to_string_pretty(&ConfigFile { apps: apps.to_vec() }).map_err(|e| e.to_string())?;
    // Write to a temp file then rename so a crash never leaves a truncated config.
    let tmp = path.with_extension("json.tmp");
    fs::write(&tmp, json).map_err(|e| e.to_string())?;
    fs::rename(&tmp, path).map_err(|e| e.to_string())
}

pub fn new_id() -> String {
    let nanos = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_nanos())
        .unwrap_or_default();
    format!("{:x}", nanos)
}

/// Suggestions for the "add app" form, guessed from the folder's package.json.
#[derive(Serialize, Default)]
#[serde(rename_all = "camelCase")]
pub struct FolderInfo {
    pub name: Option<String>,
    pub command: Option<String>,
    pub port: Option<u16>,
    /// Command to suggest if the app is declared native.
    pub native_command: Option<String>,
}

pub fn inspect_folder(dir: &Path) -> FolderInfo {
    let mut info = FolderInfo {
        name: dir.file_name().map(|n| n.to_string_lossy().to_string()),
        native_command: guess_native_command(dir),
        ..Default::default()
    };
    let pkg: Option<serde_json::Value> = fs::read_to_string(dir.join("package.json"))
        .ok()
        .and_then(|s| serde_json::from_str(&s).ok());
    let Some(pkg) = pkg else { return info };

    let scripts = &pkg["scripts"];
    let entry = if let Some(start) = scripts["start"].as_str() {
        info.command = Some(start.to_string());
        start.strip_prefix("node ").map(|s| s.split_whitespace().next().unwrap_or("").to_string())
    } else if scripts["dev"].is_string() {
        info.command = Some("npm run dev".into());
        None
    } else if let Some(main) = pkg["main"].as_str() {
        info.command = Some(format!("node {main}"));
        Some(main.to_string())
    } else {
        None
    };

    // Port: PORT=... in .env files, then a 4-5 digit number on a line mentioning "port" in the entry file.
    for env in [".env", ".env.local"] {
        if let Ok(s) = fs::read_to_string(dir.join(env)) {
            if let Some(p) = s.lines().find_map(|l| l.trim().strip_prefix("PORT=")).and_then(|v| v.trim().parse().ok()) {
                info.port = Some(p);
                return info;
            }
        }
    }
    if let Some(src) = entry.and_then(|e| fs::read_to_string(dir.join(e)).ok()) {
        // Apps that open a browser on start usually offer a flag to skip it.
        if src.contains("no-open") {
            if let Some(cmd) = info.command.as_mut().filter(|c| !c.contains("--no-open")) {
                cmd.push_str(" --no-open");
            }
        }
        info.port = src
            .lines()
            .filter(|l| l.to_ascii_lowercase().contains("port"))
            .find_map(first_port_number);
    }
    info
}

/// Command for a native app: an exe named after the folder (in the folder, dist\ or a release
/// build), else the dev script of package.json.
pub fn guess_native_command(dir: &Path) -> Option<String> {
    let name = dir.file_name()?.to_string_lossy().to_string();
    let exe = format!("{name}.exe");
    for sub in ["", "dist", "src-tauri\\target\\release"] {
        let rel = if sub.is_empty() { exe.clone() } else { format!("{sub}\\{exe}") };
        if dir.join(&rel).is_file() {
            return Some(format!("\"{rel}\""));
        }
    }
    let pkg: serde_json::Value = serde_json::from_str(&fs::read_to_string(dir.join("package.json")).ok()?).ok()?;
    ["app:dev", "tauri:dev", "dev"].iter().find(|s| pkg["scripts"][**s].is_string()).map(|s| format!("npm run {s}"))
}

fn first_port_number(line: &str) -> Option<u16> {
    line.split(|c: char| !c.is_ascii_digit())
        .filter(|d| d.len() == 4 || d.len() == 5)
        .find_map(|d| d.parse::<u16>().ok().filter(|p| *p >= 1024))
}

