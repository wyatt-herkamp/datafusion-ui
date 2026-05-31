//! Detection of `EXPLAIN` queries. Works on the SQL *text*, so it applies
//! uniformly to both the local DataFusion session and external FlightSQL — both
//! return the same `(plan_type, plan)` result shape for an EXPLAIN.

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ExplainKind {
    /// `EXPLAIN` — logical + physical plan, no execution.
    Plan,
    /// `EXPLAIN ANALYZE` — runs the query and annotates with runtime metrics.
    Analyze,
    /// `EXPLAIN VERBOSE` — extra plan detail.
    Verbose,
    /// `EXPLAIN ANALYZE VERBOSE`.
    AnalyzeVerbose,
}

impl ExplainKind {
    /// Whether the plan carries runtime metrics (i.e. ANALYZE was used).
    pub fn is_analyze(self) -> bool {
        matches!(self, ExplainKind::Analyze | ExplainKind::AnalyzeVerbose)
    }

    /// Human label for the formatted-view heading / toggles.
    pub fn label(self) -> &'static str {
        match self {
            ExplainKind::Plan => "EXPLAIN",
            ExplainKind::Analyze => "EXPLAIN ANALYZE",
            ExplainKind::Verbose => "EXPLAIN VERBOSE",
            ExplainKind::AnalyzeVerbose => "EXPLAIN ANALYZE VERBOSE",
        }
    }

    /// The SQL prefix that produces this kind, e.g. `"EXPLAIN ANALYZE "`.
    pub fn prefix(self) -> &'static str {
        match self {
            ExplainKind::Plan => "EXPLAIN ",
            ExplainKind::Analyze => "EXPLAIN ANALYZE ",
            ExplainKind::Verbose => "EXPLAIN VERBOSE ",
            ExplainKind::AnalyzeVerbose => "EXPLAIN ANALYZE VERBOSE ",
        }
    }
}

/// Detect an `EXPLAIN [ANALYZE] [VERBOSE]` prefix, ignoring leading comments and
/// whitespace. Returns `None` for ordinary queries.
pub fn detect(sql: &str) -> Option<ExplainKind> {
    let stripped = strip_leading(sql);
    let mut tokens = stripped.split_whitespace();
    if !tokens.next()?.eq_ignore_ascii_case("explain") {
        return None;
    }
    let mut analyze = false;
    let mut verbose = false;
    for t in tokens {
        if t.eq_ignore_ascii_case("analyze") {
            analyze = true;
        } else if t.eq_ignore_ascii_case("verbose") {
            verbose = true;
        } else {
            // First non-modifier token starts the inner statement.
            break;
        }
    }
    Some(match (analyze, verbose) {
        (true, true) => ExplainKind::AnalyzeVerbose,
        (true, false) => ExplainKind::Analyze,
        (false, true) => ExplainKind::Verbose,
        (false, false) => ExplainKind::Plan,
    })
}

/// Strip any `EXPLAIN [ANALYZE] [VERBOSE]` prefix, returning the inner query.
/// Used by the toolbar buttons so re-explaining doesn't stack prefixes.
pub fn strip_prefix(sql: &str) -> &str {
    if detect(sql).is_none() {
        return sql;
    }
    let stripped = strip_leading(sql);
    let mut rest = stripped;
    // Drop "explain" and any analyze/verbose modifier words.
    for word in ["explain", "analyze", "verbose"] {
        let trimmed = rest.trim_start();
        if trimmed.len() >= word.len() && trimmed[..word.len()].eq_ignore_ascii_case(word) {
            rest = &trimmed[word.len()..];
        }
    }
    rest.trim_start()
}

/// Remove leading line (`--`) and block (`/* */`) comments and whitespace.
fn strip_leading(sql: &str) -> &str {
    let mut s = sql.trim_start();
    loop {
        if let Some(rest) = s.strip_prefix("--") {
            match rest.find('\n') {
                Some(i) => s = rest[i + 1..].trim_start(),
                None => return "",
            }
        } else if let Some(rest) = s.strip_prefix("/*") {
            match rest.find("*/") {
                Some(i) => s = rest[i + 2..].trim_start(),
                None => return "",
            }
        } else {
            return s;
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn detects_kinds() {
        assert_eq!(detect("SELECT 1"), None);
        assert_eq!(detect("explain select 1"), Some(ExplainKind::Plan));
        assert_eq!(
            detect("  EXPLAIN ANALYZE SELECT 1"),
            Some(ExplainKind::Analyze)
        );
        assert_eq!(
            detect("explain verbose select 1"),
            Some(ExplainKind::Verbose)
        );
        assert_eq!(
            detect("EXPLAIN ANALYZE VERBOSE select 1"),
            Some(ExplainKind::AnalyzeVerbose)
        );
        assert_eq!(
            detect("-- a comment\nexplain select 1"),
            Some(ExplainKind::Plan)
        );
    }

    #[test]
    fn strips_prefix() {
        assert_eq!(strip_prefix("EXPLAIN ANALYZE SELECT 1"), "SELECT 1");
        assert_eq!(strip_prefix("explain select 1"), "select 1");
        assert_eq!(strip_prefix("SELECT 1"), "SELECT 1");
    }
}
