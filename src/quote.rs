//! Building the human-readable command preview.

/// Render a command vector as a single preview line, quoting only the
/// arguments that need it. Display-only; not used to construct the actual
/// invocation (that goes through argv arrays, never a shell).
pub fn preview(cmd: &[String]) -> String {
    cmd.iter()
        .map(|s| shell_quote(s))
        .collect::<Vec<_>>()
        .join(" ")
}

/// POSIX-safe single-quoting. Leaves shell-safe tokens bare so the preview
/// stays readable (`pacman -Syu`, not `'pacman' '-Syu'`).
pub fn shell_quote(s: &str) -> String {
    if s.is_empty() {
        return "''".to_string();
    }
    let safe = s.bytes().all(|b| {
        matches!(b,
            b'A'..=b'Z' | b'a'..=b'z' | b'0'..=b'9'
            | b'_' | b'@' | b'%' | b'+' | b'=' | b':' | b',' | b'.' | b'/' | b'-')
    });
    if safe {
        return s.to_string();
    }
    let mut out = String::with_capacity(s.len() + 2);
    out.push('\'');
    for c in s.chars() {
        if c == '\'' {
            out.push_str("'\\''");
        } else {
            out.push(c);
        }
    }
    out.push('\'');
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn leaves_safe_tokens_bare() {
        assert_eq!(shell_quote("pacman"), "pacman");
        assert_eq!(shell_quote("-Syu"), "-Syu");
        assert_eq!(shell_quote("/usr/bin/x"), "/usr/bin/x");
    }

    #[test]
    fn quotes_the_rest() {
        assert_eq!(shell_quote("a b"), "'a b'");
        assert_eq!(shell_quote("it's"), "'it'\\''s'");
        assert_eq!(shell_quote(""), "''");
    }

    #[test]
    fn preview_joins_with_spaces() {
        let cmd = vec!["rm".to_string(), "-rf".to_string(), "a b".to_string()];
        assert_eq!(preview(&cmd), "rm -rf 'a b'");
    }
}
