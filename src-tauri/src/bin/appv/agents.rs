//! `appv agents install|uninstall` : gives Claude Code and Codex the rule and the skill to use appv.
//! Sources: agents/ at the repo root, embedded at build time.

use std::{
    env, fs,
    path::{Path, PathBuf},
};

const RULE: &str = include_str!("../../../../agents/rules/AGENTS.md");
const SKILL: &str = include_str!("../../../../agents/skills/app-viewer/SKILL.md");
const SKILL_NAME: &str = "app-viewer";
const START: &str = "<!-- app-viewer:start -->";
const END: &str = "<!-- app-viewer:end -->";

struct Agent {
    name: &'static str,
    /// Config folder under the user profile; the agent is skipped if it doesn't exist.
    dir: &'static str,
    /// Global instructions file, inside `dir`.
    rules: &'static str,
}

const AGENTS: [Agent; 2] = [
    Agent { name: "Claude Code", dir: ".claude", rules: "CLAUDE.md" },
    Agent { name: "Codex", dir: ".codex", rules: "AGENTS.md" },
];

fn home() -> Result<PathBuf, String> {
    env::var_os("USERPROFILE").map(PathBuf::from).ok_or_else(|| "variable USERPROFILE absente".into())
}

fn read(path: &Path) -> Result<String, String> {
    match fs::read_to_string(path) {
        Ok(s) => Ok(s),
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(String::new()),
        Err(e) => Err(format!("{} : {e}", path.display())),
    }
}

fn write(path: &Path, text: &str) -> Result<(), String> {
    if let Some(dir) = path.parent() {
        fs::create_dir_all(dir).map_err(|e| format!("{} : {e}", dir.display()))?;
    }
    fs::write(path, text).map_err(|e| format!("{} : {e}", path.display()))
}

/// Our block, appended to `rest`: one line break before it (so a blank line when `rest` already
/// ends with one) and one after. `without_block` removes exactly these, so uninstalling gives
/// back the file as it was.
fn with_block(rest: &str) -> String {
    let nl = if rest.contains("\r\n") { "\r\n" } else { "\n" };
    let block = format!("{START}{nl}{}{nl}{END}{nl}", RULE.trim_end().replace('\n', nl));
    if rest.is_empty() {
        block
    } else {
        format!("{rest}{nl}{block}")
    }
}

fn without_block(text: &str) -> String {
    let (Some(start), Some(end)) = (text.find(START), text.find(END)) else { return text.to_string() };
    if end < start {
        return text.to_string();
    }
    let before = &text[..start];
    let before = before.strip_suffix("\r\n").or_else(|| before.strip_suffix('\n')).unwrap_or(before);
    let after = &text[end + END.len()..];
    let after = after.strip_prefix("\r\n").or_else(|| after.strip_prefix('\n')).unwrap_or(after);
    format!("{before}{after}")
}

pub fn install() -> Result<(), String> {
    let home = home()?;
    let mut done = 0;
    for agent in &AGENTS {
        let dir = home.join(agent.dir);
        if !dir.is_dir() {
            println!("- {} : ~/{} absent, ignoré.", agent.name, agent.dir);
            continue;
        }
        let rules = dir.join(agent.rules);
        write(&rules, &with_block(&without_block(&read(&rules)?)))?;
        let skill = dir.join("skills").join(SKILL_NAME).join("SKILL.md");
        write(&skill, SKILL)?;
        println!("✔ {} : règle dans {}, skill dans {}", agent.name, rules.display(), skill.display());
        done += 1;
    }
    if done == 0 {
        println!("Aucun agent trouvé (~/.claude ou ~/.codex).");
    }
    Ok(())
}

pub fn uninstall() -> Result<(), String> {
    let home = home()?;
    for agent in &AGENTS {
        let dir = home.join(agent.dir);
        let rules = dir.join(agent.rules);
        let text = read(&rules)?;
        if text.contains(START) {
            write(&rules, &without_block(&text))?;
            println!("✔ {} : règle retirée de {}", agent.name, rules.display());
        }
        let skill_dir = dir.join("skills").join(SKILL_NAME);
        if skill_dir.join("SKILL.md").is_file() {
            fs::remove_file(skill_dir.join("SKILL.md")).map_err(|e| e.to_string())?;
            let _ = fs::remove_dir(&skill_dir); // only if nothing else was put there
            println!("✔ {} : skill retirée", agent.name);
        }
    }
    Ok(())
}
