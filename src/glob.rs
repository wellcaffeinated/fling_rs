//! Minimal shell-style glob matching used by the server's access rules.
//!
//! Supported wildcards:
//! - `*` matches any sequence of characters (including an empty sequence).
//! - `?` matches exactly one character.
//!
//! All other characters match literally. `*` matches across any character,
//! including spaces and `/`, so a pattern like `list *` matches `list /tmp/x`.

/// Returns true if `text` matches the glob `pattern`.
///
/// Uses the classic linear-time backtracking algorithm with a single
/// remembered `*` position, which is sufficient for the simple patterns used
/// in access rules.
pub fn glob_match(pattern: &str, text: &str) -> bool {
    let p: Vec<char> = pattern.chars().collect();
    let t: Vec<char> = text.chars().collect();

    let mut pi = 0; // index into pattern
    let mut ti = 0; // index into text
    let mut star_p: Option<usize> = None; // pattern index just after the last `*`
    let mut star_t = 0; // text index when the last `*` was seen

    while ti < t.len() {
        if pi < p.len() && (p[pi] == '?' || p[pi] == t[ti]) {
            pi += 1;
            ti += 1;
        } else if pi < p.len() && p[pi] == '*' {
            star_p = Some(pi + 1);
            star_t = ti;
            pi += 1;
        } else if let Some(sp) = star_p {
            // Backtrack: let the last `*` swallow one more character.
            pi = sp;
            star_t += 1;
            ti = star_t;
        } else {
            return false;
        }
    }

    // Consume any trailing `*` in the pattern.
    while pi < p.len() && p[pi] == '*' {
        pi += 1;
    }

    pi == p.len()
}

#[cfg(test)]
mod tests {
    use super::glob_match;

    #[test]
    fn literal() {
        assert!(glob_match("list", "list"));
        assert!(!glob_match("list", "create"));
        assert!(!glob_match("list", "lists"));
    }

    #[test]
    fn star_suffix() {
        assert!(glob_match("list*", "list"));
        assert!(glob_match("list*", "list all"));
        assert!(glob_match("list*", "list --verbose things"));
        assert!(!glob_match("list*", "create things"));
    }

    #[test]
    fn star_matches_empty() {
        assert!(glob_match("*", ""));
        assert!(glob_match("list*", "list"));
    }

    #[test]
    fn star_in_middle_crosses_spaces_and_slashes() {
        assert!(glob_match("status *", "status /tmp/file"));
        assert!(glob_match("get */config", "get a/b/config"));
        assert!(!glob_match("status *", "log /tmp/file"));
    }

    #[test]
    fn question_mark() {
        assert!(glob_match("v?", "v1"));
        assert!(!glob_match("v?", "v"));
        assert!(!glob_match("v?", "v12"));
    }

    #[test]
    fn empty_pattern_matches_only_empty() {
        assert!(glob_match("", ""));
        assert!(!glob_match("", "anything"));
    }
}
