//! Who is running an agent verb, for the `by` on the ops it writes. It never names a
//! doc's drafter; only `--drafted-by` does.

use super::render;

/// Names only the person reviewing may carry.
const HUMAN_NAMES: [&str; 2] = ["human", "you"];

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Identity {
    /// Lowercase; the UI capitalises it.
    pub name: String,
}

/// The first name found along `--as`, `BIRTA_AS`, `AI_AGENT`, `CLAUDECODE`, then the Codex
/// vars. `env` is injected so tests never touch the process env.
pub fn detect(as_flag: Option<&str>, env: &dyn Fn(&str) -> Option<String>) -> Option<Identity> {
    let set = |key: &str| env(key).filter(|v| !v.trim().is_empty());
    as_flag
        .and_then(agent_name)
        .or_else(|| set("BIRTA_AS").and_then(|v| agent_name(&v)))
        .or_else(|| set("AI_AGENT").and_then(|v| ai_agent_name(&v)))
        .or_else(|| set("CLAUDECODE").map(|_| "claude".to_string()))
        // Unverified: a wrong guess costs a label only.
        .or_else(|| {
            ["CODEX_THREAD_ID", "CODEX_SANDBOX"]
                .iter()
                .any(|key| set(key).is_some())
                .then(|| "codex".to_string())
        })
        .map(|name| Identity { name })
}

/// Checks an `--as` value, as clap would, once the gate has let the verb through.
pub fn parse_as(raw: &str) -> Result<String, String> {
    agent_name(raw).ok_or_else(|| {
        let why = if raw.trim().is_empty() {
            "a name is required"
        } else if raw.chars().any(render::is_hidden) {
            "a name has no control characters"
        } else {
            "that name is the reviewer's"
        };
        format!("invalid value '{raw}' for '--as <NAME>': {why}")
    })
}

fn agent_name(raw: &str) -> Option<String> {
    let name = raw.trim().to_lowercase();
    match name.as_str() {
        "" => None,
        n if n.chars().any(render::is_hidden) => None,
        "claude-code" => Some("claude".to_string()),
        n if HUMAN_NAMES.contains(&n) => None,
        _ => Some(name),
    }
}

/// `AI_AGENT` reads `name_version_role` or `name/version`.
fn ai_agent_name(raw: &str) -> Option<String> {
    agent_name(raw.split(['_', '/']).next().unwrap_or_default())
}
