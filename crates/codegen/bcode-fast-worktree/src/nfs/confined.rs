//! Fd-relative helpers + the single owned deleter used by daemon-down `rm`
//! and `clean-artifacts`. Never a weaker sibling of `grove_git::delete_owned`.

/// Whether `id` may be used as a path component and a git ref component.
///
/// An allowlist, not a denylist: the id reaches `git update-ref`, a backing
/// directory name and a mount table, so anything outside a small alphabet --
/// whitespace, newlines, shell metacharacters, non-ASCII -- is refused rather
/// than escaped. Upstream's published snapshot only excluded separators and
/// NUL, which let a newline through into a ref name.
pub fn is_safe_worktree_id(id: &str) -> bool {
    !id.is_empty()
        && !id.starts_with('.')
        && id.len() <= 128
        && id
            .chars()
            .all(|c| c.is_ascii_alphanumeric() || matches!(c, '.' | '_' | '-'))
}

#[cfg(test)]
mod tests {
    use super::is_safe_worktree_id;

    #[test]
    fn ids_are_an_allowlist_of_path_and_ref_safe_characters() {
        assert!(is_safe_worktree_id("wt-abc_123.4"));
        assert!(!is_safe_worktree_id(""));
        assert!(!is_safe_worktree_id(".hidden"));
        assert!(!is_safe_worktree_id("wt/nested"));
        assert!(!is_safe_worktree_id("wt\\nested"));
        assert!(!is_safe_worktree_id("wt\0nul"));
        assert!(!is_safe_worktree_id("wt name"));
        assert!(!is_safe_worktree_id("wt\nnewline"));
        assert!(!is_safe_worktree_id("wt;rm -rf /"));
        assert!(!is_safe_worktree_id("wt-é"));
        assert!(!is_safe_worktree_id(&"w".repeat(129)));
    }
}
