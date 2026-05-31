//! The metadata model shared by the object explorer and the completion engine.
//!
//! A [`Catalog`] is the set of databases (FlightSQL "catalogs") reachable from a
//! single source. A local Parquet file is modeled as one synthetic database with
//! one unnamed schema holding the table `t`.

/// All databases known for one source (a file or a FlightSQL connection).
#[derive(Debug, Clone, Default, PartialEq)]
pub struct Catalog {
    pub databases: Vec<Database>,
}

/// A FlightSQL catalog (or the single synthetic catalog for a local file).
#[derive(Debug, Clone, PartialEq)]
pub struct Database {
    pub name: String,
    pub schemas: Vec<SchemaNs>,
}

/// A schema namespace within a [`Database`]. FlightSQL allows an unnamed schema,
/// hence `name` is optional.
#[derive(Debug, Clone, PartialEq)]
pub struct SchemaNs {
    pub name: Option<String>,
    pub tables: Vec<TableMeta>,
}

/// A table and its columns. `qualified` is the name to drop into a query
/// (e.g. `catalog.schema.table`, already escaped/joined by the producer).
#[derive(Debug, Clone, PartialEq)]
pub struct TableMeta {
    pub name: String,
    pub qualified: String,
    pub columns: Vec<ColumnMeta>,
}

/// A single column with a human-readable type string.
#[derive(Debug, Clone, PartialEq)]
pub struct ColumnMeta {
    pub name: String,
    pub data_type: String,
}

impl Catalog {
    /// Iterate over every table in every database/schema.
    pub fn tables(&self) -> impl Iterator<Item = &TableMeta> {
        self.databases
            .iter()
            .flat_map(|d| d.schemas.iter())
            .flat_map(|s| s.tables.iter())
    }

    /// Find a table by either its bare name or its fully-qualified name
    /// (case-insensitive). Used to resolve `FROM <name>` / `<name>.<col>`.
    pub fn find_table(&self, name: &str) -> Option<&TableMeta> {
        let needle = name.trim_matches('"');
        self.tables().find(|t| {
            t.name.eq_ignore_ascii_case(needle) || t.qualified.eq_ignore_ascii_case(needle)
        })
    }
}
