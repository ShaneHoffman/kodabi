//! The project filter shared by every project-scoped index query.
//!
//! `search_notes`, `list_outstanding_items`, and `get_project_context` all take
//! the same `(project, include_descendants)` pair from
//! `docs/MCP_TOOL_SURFACE.md` and all must mean the same thing by it — including
//! the two details that are easy to get subtly wrong independently: the reserved
//! `Inbox` sentinel maps to *unfiled* (`project IS NULL`), and a subtree match
//! is anchored on a trailing `/` so `Growth` never swallows `Growthx`.
//!
//! Resolving that pair once, here, is what keeps the three tools from drifting.

use rusqlite::types::Value;

/// Which notes a project-scoped query covers.
///
/// Built from a tool's `(project, include_descendants)` arguments by
/// [`ProjectScope::resolve`] and rendered into a `WHERE` fragment over
/// `notes.project` by [`ProjectScope::predicate`]. Every query using it must
/// name the notes table `notes` (not an alias), since the rendered SQL says so.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ProjectScope {
    /// No project filter — every note in the index.
    All,
    /// The reserved `Inbox` sentinel: unfiled notes only.
    Unfiled,
    /// This project only, excluding its sub-projects.
    Exact(String),
    /// This project and everything nested under `<slug>/`.
    Subtree(String),
}

impl ProjectScope {
    /// Resolves a tool's `project` + `include_descendants` arguments.
    ///
    /// `Inbox` (any casing) is reserved — no real project may be named it — so
    /// read tools reuse it as the "unfiled" sentinel, and it ignores
    /// `include_descendants` because unfiled notes have no subtree.
    pub fn resolve(project: Option<&str>, include_descendants: bool) -> Self {
        match project {
            None => ProjectScope::All,
            Some(project) if project.eq_ignore_ascii_case(crate::note::INBOX) => {
                ProjectScope::Unfiled
            }
            Some(project) if include_descendants => ProjectScope::Subtree(project.to_string()),
            Some(project) => ProjectScope::Exact(project.to_string()),
        }
    }

    /// The `WHERE` fragment and its bound values, or `None` for
    /// [`ProjectScope::All`] (which constrains nothing).
    pub(super) fn predicate(&self) -> Option<(String, Vec<Value>)> {
        match self {
            ProjectScope::All => None,
            ProjectScope::Unfiled => Some(("notes.project IS NULL".to_string(), Vec::new())),
            ProjectScope::Exact(project) => Some((
                "notes.project = ?".to_string(),
                vec![Value::Text(project.clone())],
            )),
            // Exact match, or any descendant under `project/`. The trailing
            // slash is what keeps `Growth` from matching `Growthx`, and the
            // pattern escapes the slug's own `%`/`_`/`\` so they stay literal.
            ProjectScope::Subtree(project) => Some((
                "(notes.project = ? OR notes.project LIKE ? ESCAPE '\\')".to_string(),
                vec![
                    Value::Text(project.clone()),
                    Value::Text(format!("{}/%", like_escape(project))),
                ],
            )),
        }
    }
}

/// Escapes a string for use inside a `LIKE … ESCAPE '\'` pattern so its own
/// `%`, `_`, and `\` stay literal.
pub(super) fn like_escape(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    for c in s.chars() {
        if matches!(c, '\\' | '%' | '_') {
            out.push('\\');
        }
        out.push(c);
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    fn text(values: &[Value]) -> Vec<&str> {
        values
            .iter()
            .map(|value| match value {
                Value::Text(text) => text.as_str(),
                other => panic!("expected text, got {other:?}"),
            })
            .collect()
    }

    #[test]
    fn no_project_constrains_nothing() {
        assert_eq!(ProjectScope::resolve(None, true), ProjectScope::All);
        assert_eq!(ProjectScope::resolve(None, false), ProjectScope::All);
        assert!(ProjectScope::All.predicate().is_none());
    }

    #[test]
    fn the_inbox_sentinel_means_unfiled_whatever_its_casing() {
        for spelling in ["Inbox", "inbox", "INBOX", "InBoX"] {
            assert_eq!(
                ProjectScope::resolve(Some(spelling), true),
                ProjectScope::Unfiled,
                "{spelling}"
            );
        }
        let (clause, values) = ProjectScope::Unfiled.predicate().unwrap();
        assert_eq!(clause, "notes.project IS NULL");
        // The NULL branch binds nothing — `project = ?` never matches NULL.
        assert!(values.is_empty());
    }

    #[test]
    fn exact_binds_only_the_slug() {
        assert_eq!(
            ProjectScope::resolve(Some("Growth"), false),
            ProjectScope::Exact("Growth".to_string())
        );
        let (clause, values) = ProjectScope::Exact("Growth".to_string())
            .predicate()
            .unwrap();
        assert_eq!(clause, "notes.project = ?");
        assert_eq!(text(&values), ["Growth"]);
    }

    #[test]
    fn subtree_anchors_the_pattern_on_a_trailing_slash() {
        assert_eq!(
            ProjectScope::resolve(Some("Growth"), true),
            ProjectScope::Subtree("Growth".to_string())
        );
        let (clause, values) = ProjectScope::Subtree("Growth".to_string())
            .predicate()
            .unwrap();
        assert_eq!(
            clause,
            "(notes.project = ? OR notes.project LIKE ? ESCAPE '\\')"
        );
        // `Growth/%` matches `Growth/Q3` but never the sibling `Growthx`.
        assert_eq!(text(&values), ["Growth", "Growth/%"]);
    }

    #[test]
    fn a_slug_carrying_like_metacharacters_stays_literal() {
        let (_, values) = ProjectScope::Subtree("50%_off\\now".to_string())
            .predicate()
            .unwrap();
        assert_eq!(text(&values), ["50%_off\\now", "50\\%\\_off\\\\now/%"]);
    }

    #[test]
    fn like_escape_only_touches_the_three_metacharacters() {
        assert_eq!(like_escape("plain"), "plain");
        assert_eq!(like_escape("a%b"), "a\\%b");
        assert_eq!(like_escape("a_b"), "a\\_b");
        assert_eq!(like_escape("a\\b"), "a\\\\b");
        // Non-ASCII and path separators are not metacharacters.
        assert_eq!(like_escape("Ops/Ünï"), "Ops/Ünï");
    }
}
