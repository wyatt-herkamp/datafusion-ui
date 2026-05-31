//! Parser-only syntax validation. This catches malformed SQL before the query
//! is sent to DataFusion / FlightSQL; it does not do semantic checks (unknown
//! tables/columns) — that requires planning against a real catalog.

use sqlparser::dialect::GenericDialect;
use sqlparser::parser::Parser;

/// A single syntax problem. `line`/`column` are 1-based (sqlparser convention)
/// and may be `None` when the parser could not attribute a location.
#[derive(Debug, Clone, PartialEq)]
pub struct Diagnostic {
    pub message: String,
    pub line: Option<u64>,
    pub column: Option<u64>,
}

/// Validate `sql`. Returns an empty vec when the statement parses cleanly.
/// Blank input is treated as valid (nothing to run yet).
pub fn diagnostics(sql: &str) -> Vec<Diagnostic> {
    if sql.trim().is_empty() {
        return Vec::new();
    }
    let dialect = GenericDialect {};
    match Parser::parse_sql(&dialect, sql) {
        Ok(_) => Vec::new(),
        Err(err) => vec![from_parser_error(err)],
    }
}

fn from_parser_error(err: sqlparser::parser::ParserError) -> Diagnostic {
    // sqlparser embeds the location in the message as "... at Line: N, Column: M".
    let message = err.to_string();
    let (line, column) = parse_location(&message);
    Diagnostic {
        message,
        line,
        column,
    }
}

/// Best-effort extraction of `Line: N, Column: M` from a parser error message.
fn parse_location(message: &str) -> (Option<u64>, Option<u64>) {
    let after = |key: &str| -> Option<u64> {
        let idx = message.find(key)? + key.len();
        let rest = &message[idx..];
        let digits: String = rest
            .trim_start()
            .chars()
            .take_while(|c| c.is_ascii_digit())
            .collect();
        digits.parse().ok()
    };
    (after("Line:"), after("Column:"))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn valid_sql_has_no_diagnostics() {
        assert!(diagnostics("SELECT 1").is_empty());
        assert!(diagnostics("   ").is_empty());
    }

    #[test]
    fn invalid_sql_reports_a_diagnostic() {
        let diags = diagnostics("SELECT FROM");
        assert_eq!(diags.len(), 1);
        assert!(!diags[0].message.is_empty());
    }
}
