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
    // The embedded source may itself be CRLF (git autocrlf): normalize before converting.
    let rule = RULE.trim_end().replace("\r\n", "\n").replace('\n', nl);
    let block = format!("{START}{nl}{rule}{nl}{END}{nl}");
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
    install_in(&home()?)
}

pub fn uninstall() -> Result<(), String> {
    uninstall_in(&home()?)
}

/// `install` in a given home folder (the user profile, or a temporary folder in tests).
fn install_in(home: &Path) -> Result<(), String> {
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

fn uninstall_in(home: &Path) -> Result<(), String> {
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

#[cfg(test)]
mod tests {
    use super::*;

    /// Fresh fake user profile under the system temp folder: tests never touch the real one.
    fn temp_home(tag: &str) -> PathBuf {
        let nanos = std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).unwrap().as_nanos();
        let dir = env::temp_dir().join(format!("appv-test-agents-{tag}-{}-{nanos}", std::process::id()));
        fs::create_dir_all(&dir).unwrap();
        dir
    }

    fn skill_path(home: &Path, agent: &str) -> PathBuf {
        home.join(agent).join("skills").join(SKILL_NAME).join("SKILL.md")
    }

    #[test]
    fn block_round_trip_keeps_text() {
        for rest in ["", "# Règles\n", "# Règles\n\n- un\n", "# Règles\r\n- un\r\n", "sans fin de ligne"] {
            let with = with_block(rest);
            assert!(with.contains(START) && with.contains(END));
            assert_eq!(without_block(&with), rest, "texte d'origine : {rest:?}");
            // Already installed: replacing the block gives the same text.
            assert_eq!(with_block(&without_block(&with)), with);
        }
    }

    #[test]
    fn block_follows_crlf_line_endings() {
        let with = with_block("a\r\nb\r\n");
        assert!(!with.replace("\r\n", "").contains('\n'), "fin de ligne LF dans un fichier CRLF");
        assert!(!with.contains("\r\r"), "retour chariot doublé (règle source en CRLF)");
        assert!(!with_block("a\nb\n").contains('\r'), "fin de ligne CRLF dans un fichier LF");
    }

    #[test]
    fn without_block_ignores_missing_or_reversed_markers() {
        assert_eq!(without_block("rien ici\n"), "rien ici\n");
        let reversed = format!("x\n{END}\ny\n{START}\n");
        assert_eq!(without_block(&reversed), reversed);
    }

    #[test]
    fn install_is_idempotent_and_uninstall_restores_files() {
        let home = temp_home("roundtrip");
        fs::create_dir_all(home.join(".claude")).unwrap();
        fs::create_dir_all(home.join(".codex")).unwrap();
        let claude_md = home.join(".claude").join("CLAUDE.md");
        let original = "# Mes règles\r\n\r\n- garder ceci\r\n";
        fs::write(&claude_md, original).unwrap();

        install_in(&home).unwrap();
        let rules = fs::read_to_string(&claude_md).unwrap();
        assert!(rules.starts_with(original) && rules.contains(START));
        assert!(rules.contains(RULE.lines().next().unwrap()));
        assert_eq!(fs::read_to_string(skill_path(&home, ".claude")).unwrap(), SKILL);
        let codex_md = home.join(".codex").join("AGENTS.md");
        assert!(fs::read_to_string(&codex_md).unwrap().starts_with(START));
        assert_eq!(fs::read_to_string(skill_path(&home, ".codex")).unwrap(), SKILL);

        // A second install changes nothing (one block only).
        install_in(&home).unwrap();
        assert_eq!(fs::read_to_string(&claude_md).unwrap(), rules);
        assert_eq!(fs::read_to_string(&claude_md).unwrap().matches(START).count(), 1);

        uninstall_in(&home).unwrap();
        assert_eq!(fs::read_to_string(&claude_md).unwrap(), original);
        assert_eq!(fs::read_to_string(&codex_md).unwrap(), "");
        assert!(!home.join(".claude").join("skills").join(SKILL_NAME).exists());
        assert!(!home.join(".codex").join("skills").join(SKILL_NAME).exists());

        // Uninstalling again is harmless.
        uninstall_in(&home).unwrap();
        assert_eq!(fs::read_to_string(&claude_md).unwrap(), original);
        fs::remove_dir_all(&home).unwrap();
    }

    #[test]
    fn install_skips_missing_agents_and_uninstall_keeps_other_files() {
        let home = temp_home("partial");
        fs::create_dir_all(home.join(".claude")).unwrap();

        install_in(&home).unwrap();
        assert!(!home.join(".codex").exists(), "~/.codex ne doit pas être créé");
        // Something else the user put in the skill folder survives uninstall.
        let extra = home.join(".claude").join("skills").join(SKILL_NAME).join("notes.md");
        fs::write(&extra, "à moi").unwrap();

        uninstall_in(&home).unwrap();
        assert!(!skill_path(&home, ".claude").exists());
        assert_eq!(fs::read_to_string(&extra).unwrap(), "à moi");
        assert!(!home.join(".codex").exists());
        fs::remove_dir_all(&home).unwrap();
    }
}
