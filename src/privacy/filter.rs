/// Check if a command matches any ignore pattern.
/// Patterns support simple glob-style matching with `*` as wildcard.
pub fn should_ignore(command: &str, patterns: &[String]) -> bool {
    let cmd_upper = command.to_uppercase();
    for pattern in patterns {
        if glob_match(&cmd_upper, &pattern.to_uppercase()) {
            return true;
        }
    }
    false
}

/// Simple glob matching: `*` matches any sequence of characters.
fn glob_match(text: &str, pattern: &str) -> bool {
    let parts: Vec<&str> = pattern.split('*').collect();

    if parts.len() == 1 {
        return text == pattern;
    }

    let mut pos = 0;

    // First part must match at start
    if !parts[0].is_empty() {
        if !text.starts_with(parts[0]) {
            return false;
        }
        pos = parts[0].len();
    }

    // Last part must match at end
    let last = parts[parts.len() - 1];
    if !last.is_empty() && !text.ends_with(last) {
        return false;
    }

    // Middle parts must appear in order
    for &part in &parts[1..parts.len() - 1] {
        if part.is_empty() {
            continue;
        }
        if let Some(idx) = text[pos..].find(part) {
            pos += idx + part.len();
        } else {
            return false;
        }
    }

    true
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_exact_match() {
        assert!(should_ignore(
            "export SECRET_KEY=abc",
            &["export SECRET_KEY=abc".to_string()]
        ));
    }

    #[test]
    fn test_wildcard_match() {
        assert!(should_ignore(
            "export AWS_SECRET_KEY=xyz",
            &["export *SECRET*".to_string()]
        ));
    }

    #[test]
    fn test_no_match() {
        assert!(!should_ignore(
            "git status",
            &["export *SECRET*".to_string()]
        ));
    }

    #[test]
    fn test_case_insensitive() {
        assert!(should_ignore(
            "EXPORT my_key=123",
            &["export *KEY*".to_string()]
        ));
    }
}
