//! Schema-aware autocomplete.
//!
//! This is a heuristic, token-based engine — not a full binder. It scans the
//! token stream around the cursor to decide what kind of name is expected
//! (table, column, keyword) and resolves simple `FROM x [AS] a` aliases. That is
//! enough for the common editing cases without the cost of full semantic
//! analysis.

use crate::catalog::Catalog;
use crate::lex::{SpannedToken, TokenKind, lex};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CompletionKind {
    Keyword,
    Function,
    Table,
    Column,
}

/// A single completion candidate. `replace_len` is how many characters of the
/// partially-typed word (immediately before the cursor) the caller should
/// remove before inserting `insert_text`.
#[derive(Debug, Clone, PartialEq)]
pub struct Completion {
    pub label: String,
    pub kind: CompletionKind,
    pub insert_text: String,
    pub detail: Option<String>,
    pub replace_len: usize,
}

const KEYWORDS: &[&str] = &[
    "SELECT",
    "FROM",
    "WHERE",
    "GROUP BY",
    "ORDER BY",
    "HAVING",
    "LIMIT",
    "OFFSET",
    "JOIN",
    "LEFT JOIN",
    "RIGHT JOIN",
    "INNER JOIN",
    "FULL JOIN",
    "ON",
    "AS",
    "AND",
    "OR",
    "NOT",
    "IN",
    "IS",
    "NULL",
    "LIKE",
    "BETWEEN",
    "DISTINCT",
    "WITH",
    "UNION",
    "ALL",
    "CASE",
    "WHEN",
    "THEN",
    "ELSE",
    "END",
    "ASC",
    "DESC",
    "INSERT",
    "INTO",
    "VALUES",
    "UPDATE",
    "SET",
    "DELETE",
    "CREATE",
    "TABLE",
    "EXPLAIN",
];

const FUNCTIONS: &[&str] = &[
    "count",
    "sum",
    "avg",
    "min",
    "max",
    "abs",
    "round",
    "floor",
    "ceil",
    "coalesce",
    "cast",
    "length",
    "lower",
    "upper",
    "trim",
    "substr",
    "concat",
    "now",
    "date_trunc",
    "extract",
    "to_timestamp",
    "array_agg",
    "approx_distinct",
    "stddev",
    "variance",
];

/// Compute completions for the cursor at (1-based) `line`, `column` in `sql`.
pub fn complete(sql: &str, line: u64, column: u64, catalog: &Catalog) -> Vec<Completion> {
    let cursor = cursor_byte_offset(sql, line, column);
    let before = &sql[..cursor];

    let (prefix, after_dot, qualifier) = parse_prefix(before);
    let replace_len = prefix.chars().count();
    let prefix_lc = prefix.to_ascii_lowercase();

    // Member access `qualifier.<prefix>` → that table's columns only.
    if after_dot {
        let mut out = Vec::new();
        if let Some(table) = resolve_qualifier(&qualifier, sql, catalog) {
            for col in &table.columns {
                push_if_matches(
                    &mut out,
                    &col.name,
                    CompletionKind::Column,
                    Some(col.data_type.clone()),
                    &prefix_lc,
                    replace_len,
                );
            }
        }
        return out;
    }

    let ctx = clause_context(before);
    let mut out = Vec::new();

    match ctx {
        Clause::Table => {
            for table in catalog.tables() {
                push_if_matches(
                    &mut out,
                    &table.name,
                    CompletionKind::Table,
                    Some(table.qualified.clone()),
                    &prefix_lc,
                    replace_len,
                );
                if !table.qualified.eq_ignore_ascii_case(&table.name) {
                    push_if_matches(
                        &mut out,
                        &table.qualified,
                        CompletionKind::Table,
                        Some("table".into()),
                        &prefix_lc,
                        replace_len,
                    );
                }
            }
        }
        Clause::Expr => {
            // Columns from tables in FROM scope, then functions, then keywords.
            for table in tables_in_scope(sql, catalog) {
                for col in &table.columns {
                    push_if_matches(
                        &mut out,
                        &col.name,
                        CompletionKind::Column,
                        Some(format!("{} · {}", table.name, col.data_type)),
                        &prefix_lc,
                        replace_len,
                    );
                }
            }
            for f in FUNCTIONS {
                push_if_matches(
                    &mut out,
                    f,
                    CompletionKind::Function,
                    Some("function".into()),
                    &prefix_lc,
                    replace_len,
                );
            }
            for kw in KEYWORDS {
                push_if_matches(
                    &mut out,
                    kw,
                    CompletionKind::Keyword,
                    None,
                    &prefix_lc,
                    replace_len,
                );
            }
        }
        Clause::Start => {
            for kw in KEYWORDS {
                push_if_matches(
                    &mut out,
                    kw,
                    CompletionKind::Keyword,
                    None,
                    &prefix_lc,
                    replace_len,
                );
            }
        }
    }

    out
}

fn push_if_matches(
    out: &mut Vec<Completion>,
    candidate: &str,
    kind: CompletionKind,
    detail: Option<String>,
    prefix_lc: &str,
    replace_len: usize,
) {
    if prefix_lc.is_empty() || candidate.to_ascii_lowercase().starts_with(prefix_lc) {
        out.push(Completion {
            label: candidate.to_string(),
            kind,
            insert_text: candidate.to_string(),
            detail,
            replace_len,
        });
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Clause {
    /// Right after FROM/JOIN — expect a table name.
    Table,
    /// In an expression position — expect columns/functions/keywords.
    Expr,
    /// Empty / start of statement — keywords only.
    Start,
}

/// Decide what is expected at the cursor from the last meaningful keyword.
fn clause_context(before: &str) -> Clause {
    let tokens: Vec<SpannedToken> = lex(before)
        .into_iter()
        .filter(|t| !matches!(t.kind, TokenKind::Whitespace | TokenKind::Comment))
        .collect();

    // Walk backwards for the most recent anchor keyword.
    for tok in tokens.iter().rev() {
        if tok.kind != TokenKind::Keyword {
            continue;
        }
        let kw = tok.text.to_ascii_uppercase();
        match kw.as_str() {
            "FROM" | "JOIN" | "INTO" | "UPDATE" | "TABLE" => return Clause::Table,
            "SELECT" | "WHERE" | "ON" | "HAVING" | "BY" | "SET" | "AND" | "OR" | "NOT" | "IN"
            | "VALUES" | "WHEN" | "THEN" | "ELSE" => return Clause::Expr,
            _ => return Clause::Expr,
        }
    }
    if tokens.is_empty() {
        Clause::Start
    } else {
        Clause::Expr
    }
}

/// Extract the identifier being typed immediately before the cursor. Returns
/// `(prefix, after_dot, qualifier)` where `after_dot` indicates `qualifier.`
/// member access.
fn parse_prefix(before: &str) -> (String, bool, String) {
    let chars: Vec<char> = before.chars().collect();
    let mut i = chars.len();
    while i > 0 && is_ident_char(chars[i - 1]) {
        i -= 1;
    }
    let prefix: String = chars[i..].iter().collect();

    // Is the char before the prefix a '.'? If so, read the qualifier ident.
    if i > 0 && chars[i - 1] == '.' {
        let mut j = i - 1;
        while j > 0 && is_ident_char(chars[j - 1]) {
            j -= 1;
        }
        let qualifier: String = chars[j..i - 1].iter().collect();
        return (prefix, true, qualifier);
    }
    (prefix, false, String::new())
}

fn is_ident_char(c: char) -> bool {
    c.is_alphanumeric() || c == '_'
}

/// Resolve a `qualifier` (table name or alias) to its [`TableMeta`].
fn resolve_qualifier<'a>(
    qualifier: &str,
    sql: &str,
    catalog: &'a Catalog,
) -> Option<&'a crate::catalog::TableMeta> {
    // Direct table name first.
    if let Some(t) = catalog.find_table(qualifier) {
        return Some(t);
    }
    // Otherwise look for `FROM/JOIN <table> [AS] <qualifier>`.
    let refs = from_references(sql);
    for (table, alias) in refs {
        if alias
            .as_deref()
            .is_some_and(|a| a.eq_ignore_ascii_case(qualifier))
        {
            return catalog.find_table(&table);
        }
    }
    None
}

/// Tables referenced by FROM/JOIN clauses, looked up in the catalog.
fn tables_in_scope<'a>(sql: &str, catalog: &'a Catalog) -> Vec<&'a crate::catalog::TableMeta> {
    let mut out = Vec::new();
    for (table, _alias) in from_references(sql) {
        if let Some(t) = catalog.find_table(&table)
            && !out
                .iter()
                .any(|existing: &&crate::catalog::TableMeta| std::ptr::eq(*existing, t))
        {
            out.push(t);
        }
    }
    out
}

/// Parse `(table_name, optional_alias)` pairs from FROM/JOIN clauses by scanning
/// the token stream. Handles `FROM t`, `FROM t a`, and `FROM t AS a`.
fn from_references(sql: &str) -> Vec<(String, Option<String>)> {
    let tokens: Vec<SpannedToken> = lex(sql)
        .into_iter()
        .filter(|t| !matches!(t.kind, TokenKind::Whitespace | TokenKind::Comment))
        .collect();

    let mut out = Vec::new();
    let mut i = 0;
    while i < tokens.len() {
        let kw = tokens[i].text.to_ascii_uppercase();
        let is_anchor = tokens[i].kind == TokenKind::Keyword && (kw == "FROM" || kw == "JOIN");
        if is_anchor && i + 1 < tokens.len() {
            // Collect a (possibly dotted) table name: ident (. ident)*
            let mut name = String::new();
            let mut j = i + 1;
            loop {
                if j >= tokens.len() || tokens[j].kind == TokenKind::Keyword {
                    break;
                }
                if tokens[j].kind == TokenKind::Identifier {
                    name.push_str(&tokens[j].text);
                    j += 1;
                    if j < tokens.len() && tokens[j].text == "." {
                        name.push('.');
                        j += 1;
                        continue;
                    }
                }
                break;
            }
            if name.is_empty() {
                i += 1;
                continue;
            }
            // Optional alias: `AS ident` or bare `ident`.
            let mut alias = None;
            if j < tokens.len() {
                if tokens[j].kind == TokenKind::Keyword && tokens[j].text.eq_ignore_ascii_case("AS")
                {
                    j += 1;
                }
                if j < tokens.len() && tokens[j].kind == TokenKind::Identifier {
                    alias = Some(tokens[j].text.clone());
                    j += 1;
                }
            }
            out.push((name, alias));
            i = j;
        } else {
            i += 1;
        }
    }
    out
}

/// Byte offset into `sql` of the (1-based) `line`/`column` cursor position.
fn cursor_byte_offset(sql: &str, line: u64, column: u64) -> usize {
    let mut off = 0usize;
    for (idx, l) in sql.split_inclusive('\n').enumerate() {
        if (idx as u64) + 1 == line {
            for (chars, (b, _)) in l.char_indices().enumerate() {
                if chars as u64 == column.saturating_sub(1) {
                    return off + b;
                }
            }
            return off + l.trim_end_matches('\n').len();
        }
        off += l.len();
    }
    sql.len()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::catalog::{Catalog, ColumnMeta, Database, SchemaNs, TableMeta};

    fn sample_catalog() -> Catalog {
        Catalog {
            databases: vec![Database {
                name: "cat".into(),
                schemas: vec![SchemaNs {
                    name: Some("sch".into()),
                    tables: vec![TableMeta {
                        name: "orders".into(),
                        qualified: "cat.sch.orders".into(),
                        columns: vec![
                            ColumnMeta {
                                name: "id".into(),
                                data_type: "Int64".into(),
                            },
                            ColumnMeta {
                                name: "amount".into(),
                                data_type: "Float64".into(),
                            },
                        ],
                    }],
                }],
            }],
        }
    }

    fn labels(c: &[Completion]) -> Vec<String> {
        c.iter().map(|x| x.label.clone()).collect()
    }

    #[test]
    fn after_from_suggests_tables() {
        let cat = sample_catalog();
        let sql = "SELECT * FROM ";
        let comps = complete(sql, 1, (sql.len() + 1) as u64, &cat);
        assert!(labels(&comps).contains(&"orders".to_string()));
    }

    #[test]
    fn from_prefix_filters() {
        let cat = sample_catalog();
        let sql = "SELECT * FROM or";
        let comps = complete(sql, 1, (sql.len() + 1) as u64, &cat);
        assert!(
            comps
                .iter()
                .all(|c| c.label.to_lowercase().starts_with("or"))
        );
        assert_eq!(comps[0].replace_len, 2);
    }

    #[test]
    fn member_access_suggests_columns() {
        let cat = sample_catalog();
        let sql = "SELECT o. FROM orders o";
        // Cursor right after the dot (char position 9 -> column 10).
        let dot = sql.find('.').unwrap();
        let comps = complete(sql, 1, (dot + 2) as u64, &cat);
        let l = labels(&comps);
        assert!(l.contains(&"id".to_string()));
        assert!(l.contains(&"amount".to_string()));
    }

    #[test]
    fn select_suggests_columns_from_scope() {
        let cat = sample_catalog();
        let sql = "SELECT  FROM orders";
        // Cursor after "SELECT " (column 8).
        let comps = complete(sql, 1, 8, &cat);
        let l = labels(&comps);
        assert!(l.contains(&"id".to_string()));
        assert!(l.contains(&"amount".to_string()));
    }
}
