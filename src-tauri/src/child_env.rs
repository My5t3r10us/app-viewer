//! Shared by App Viewer and `appv` (included in both with `#[path]`): keeps the context of an AI
//! agent session (Claude Code, Codex) out of App Viewer and of the apps it launches.
//!
//! When an agent runs `appv open` while App Viewer is closed, App Viewer starts as a child of the
//! agent's shell and inherits its environment, which it then hands to every app it launches. An
//! app would then behave differently depending on which session started App Viewer: a `claude`
//! launched by the app sees `CLAUDECODE` (nested session) and the base URL of a session that may
//! be closed since.

use std::process::Command;

/// Variables an agent session sets for its own tools, never meant for the user's apps.
fn is_agent_var(name: &str, in_claude_session: bool) -> bool {
    let n = name.to_ascii_uppercase();
    n == "CLAUDECODE"
        || n == "CLAUDE_PID"
        || n.starts_with("CLAUDE_CODE_")
        || n.starts_with("CLAUDE_AGENT_SDK_")
        || n.starts_with("CLAUDE_PREVIEW_")
        || n.starts_with("CODEX_SANDBOX")
        || n == "CODEX_THREAD_ID"
        // Set by the desktop app to route its session through a local proxy; a value the user
        // set themselves is kept (it is only dropped when inherited from a Claude Code session).
        || (in_claude_session && n == "ANTHROPIC_BASE_URL")
}

/// Removes the agent session's variables from the environment `cmd` will get.
pub fn scrub_agent_env(cmd: &mut Command) {
    let in_claude_session = std::env::var_os("CLAUDECODE").is_some();
    for (name, _) in std::env::vars_os() {
        if let Some(n) = name.to_str() {
            if is_agent_var(n, in_claude_session) {
                cmd.env_remove(&name);
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::is_agent_var;

    #[test]
    fn agent_vars() {
        for v in ["CLAUDECODE", "CLAUDE_CODE_SESSION_ID", "CLAUDE_AGENT_SDK_VERSION", "CLAUDE_PID", "CODEX_SANDBOX"] {
            assert!(is_agent_var(v, true), "{v}");
        }
        for v in ["PATH", "PORT", "CLAUDE_CONFIG_DIR", "CODEX_HOME", "ANTHROPIC_API_KEY"] {
            assert!(!is_agent_var(v, true), "{v}");
        }
        assert!(is_agent_var("ANTHROPIC_BASE_URL", true));
        assert!(!is_agent_var("ANTHROPIC_BASE_URL", false));
    }
}
