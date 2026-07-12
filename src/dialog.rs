//! Shared formatting of the auth-dialog message, so every backend (zenity,
//! kdialog, pinentry, osascript, Windows MessageBox) presents the same, quickly
//! scannable layout: what will run, and who asked for it.

/// The information block shown above the password field / action buttons.
///
/// `interactive` is whether the caller has a controlling terminal — a reliable
/// "a human at a keyboard launched this" signal, independent of the (best-
/// effort) `actor` name.
pub fn info_block(preview: &str, actor: &str, interactive: Option<bool>) -> String {
    let who = match interactive {
        Some(true) => format!("{actor}  \u{00b7} interactive terminal"),
        Some(false) => format!("\u{26a0} {actor}  \u{00b7} no terminal (automation)"),
        None => actor.to_string(),
    };
    format!("Run this command as root:\n\n    {preview}\n\nRequested by:  {who}")
}

#[cfg(test)]
mod tests {
    use super::info_block;

    #[test]
    fn interactive_shows_terminal() {
        let b = info_block("id", "cosmic-term", Some(true));
        assert!(b.contains("    id"), "command is indented on its own line");
        assert!(b.contains("cosmic-term"));
        assert!(b.contains("interactive terminal"));
    }

    #[test]
    fn automation_is_flagged() {
        let b = info_block("id", "claude", Some(false));
        assert!(b.contains("claude"));
        assert!(b.contains("automation"));
        assert!(
            b.contains('\u{26a0}'),
            "automation is marked with a warning sign"
        );
    }

    #[test]
    fn unknown_interactivity_just_names_actor() {
        let b = info_block("id", "claude", None);
        assert!(b.contains("Requested by:  claude"));
        assert!(!b.contains("terminal"));
    }
}
