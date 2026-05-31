//! Object-explorer state for the SQL view's left tree.
//!
//! Holds the lazily-loaded catalog→schema→table→column hierarchy for every
//! source (the open Parquet file plus each FlightSQL connection) and converts
//! the loaded portion into a [`sql_ide::Catalog`] for completion.
//!
//! Nodes are addressed by [`ExplorerTarget`] (carried in messages) rather than
//! by Vec indices, so loads route back to the right node without depending on
//! traversal order.

use arrow::datatypes::SchemaRef;
use sql_ide::{Catalog, ColumnMeta, Database, SchemaNs, TableMeta};

use crate::app::FileId;
use crate::error::FlightError;
use crate::flightsql::{ColumnEntry, TableEntry};

/// Which source/level a toggle or load refers to.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ExplorerTarget {
    /// An open file's root node (its Overview/Row Groups/Data + schema).
    FileRoot {
        file: FileId,
    },
    /// An open file's "schema (N cols)" node.
    FileSchema {
        file: FileId,
    },
    /// A FlightSQL connection root (its catalog list).
    FlightRoot {
        conn: usize,
    },
    Catalog {
        conn: usize,
        catalog: String,
    },
    Schema {
        conn: usize,
        catalog: String,
        schema: Option<String>,
    },
    Table {
        conn: usize,
        catalog: String,
        schema: Option<String>,
        table: String,
    },
}

/// The async payload delivered for a node's children.
#[derive(Debug, Clone)]
pub enum ExplorerLoad {
    Catalogs(Result<Vec<String>, FlightError>),
    Schemas(Result<Vec<Option<String>>, FlightError>),
    Tables(Result<Vec<TableEntry>, FlightError>),
    Columns(Result<Vec<ColumnEntry>, FlightError>),
}

#[derive(Debug, Default)]
pub struct Explorer {
    /// Whether the whole left panel is collapsed to a thin rail.
    pub collapsed: bool,
    /// One node per open file, addressed by [`FileId`].
    pub files: Vec<FileNode>,
    /// One tree per FlightSQL connection, aligned with `App::connections`.
    pub flights: Vec<FlightTree>,
}

/// Sidebar state for one open file. The schema columns are read from the
/// `FileTab` itself when rendering; this only tracks expansion.
#[derive(Debug)]
pub struct FileNode {
    pub id: FileId,
    pub name: String,
    pub expanded: bool,
    pub schema_expanded: bool,
}

#[derive(Debug, Default)]
pub struct FlightTree {
    pub expanded: bool,
    pub loaded: bool,
    pub loading: bool,
    pub error: Option<String>,
    pub catalogs: Vec<CatalogNode>,
}

#[derive(Debug, Default)]
pub struct CatalogNode {
    pub name: String,
    pub expanded: bool,
    pub loaded: bool,
    pub loading: bool,
    pub error: Option<String>,
    pub schemas: Vec<SchemaNode>,
}

#[derive(Debug, Default)]
pub struct SchemaNode {
    pub name: Option<String>,
    pub expanded: bool,
    pub loaded: bool,
    pub loading: bool,
    pub error: Option<String>,
    pub tables: Vec<TableNode>,
}

#[derive(Debug, Default)]
pub struct TableNode {
    pub name: String,
    pub table_type: String,
    pub expanded: bool,
    pub loaded: bool,
    pub loading: bool,
    pub error: Option<String>,
    pub columns: Vec<ColumnEntry>,
}

/// What a toggle decided needs fetching (the caller spawns the RPC).
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum LoadRequest {
    Catalogs {
        conn: usize,
    },
    Schemas {
        conn: usize,
        catalog: String,
    },
    Tables {
        conn: usize,
        catalog: String,
        schema: Option<String>,
    },
    Columns {
        conn: usize,
        qualified: String,
        target: ExplorerTarget,
    },
}

impl Explorer {
    pub fn on_connect(&mut self) {
        self.flights.push(FlightTree::default());
    }

    pub fn on_disconnect(&mut self, idx: usize) {
        if idx < self.flights.len() {
            self.flights.remove(idx);
        }
    }

    pub fn on_file_open(&mut self, id: FileId, name: String) {
        self.files.push(FileNode {
            id,
            name,
            expanded: true,
            schema_expanded: false,
        });
    }

    pub fn on_file_close(&mut self, id: FileId) {
        self.files.retain(|f| f.id != id);
    }

    fn file_node_mut(&mut self, id: FileId) -> Option<&mut FileNode> {
        self.files.iter_mut().find(|f| f.id == id)
    }

    /// Toggle a node's expansion. Returns a [`LoadRequest`] when expanding a node
    /// whose children have not been fetched yet.
    pub fn toggle(&mut self, target: &ExplorerTarget) -> Option<LoadRequest> {
        match target {
            ExplorerTarget::FileRoot { file } => {
                if let Some(node) = self.file_node_mut(*file) {
                    node.expanded = !node.expanded;
                }
                None
            }
            ExplorerTarget::FileSchema { file } => {
                if let Some(node) = self.file_node_mut(*file) {
                    node.schema_expanded = !node.schema_expanded;
                }
                None
            }
            ExplorerTarget::FlightRoot { conn } => {
                let tree = self.flights.get_mut(*conn)?;
                tree.expanded = !tree.expanded;
                if tree.expanded && !tree.loaded && !tree.loading {
                    tree.loading = true;
                    Some(LoadRequest::Catalogs { conn: *conn })
                } else {
                    None
                }
            }
            ExplorerTarget::Catalog { conn, catalog } => {
                let node = self.catalog_mut(*conn, catalog)?;
                node.expanded = !node.expanded;
                if node.expanded && !node.loaded && !node.loading {
                    node.loading = true;
                    Some(LoadRequest::Schemas {
                        conn: *conn,
                        catalog: catalog.clone(),
                    })
                } else {
                    None
                }
            }
            ExplorerTarget::Schema {
                conn,
                catalog,
                schema,
            } => {
                let node = self.schema_mut(*conn, catalog, schema)?;
                node.expanded = !node.expanded;
                if node.expanded && !node.loaded && !node.loading {
                    node.loading = true;
                    Some(LoadRequest::Tables {
                        conn: *conn,
                        catalog: catalog.clone(),
                        schema: schema.clone(),
                    })
                } else {
                    None
                }
            }
            ExplorerTarget::Table {
                conn,
                catalog,
                schema,
                table,
            } => {
                let qualified = qualify(catalog, schema, table);
                let node = self.table_mut(*conn, catalog, schema, table)?;
                node.expanded = !node.expanded;
                if node.expanded && !node.loaded && !node.loading {
                    node.loading = true;
                    Some(LoadRequest::Columns {
                        conn: *conn,
                        qualified,
                        target: target.clone(),
                    })
                } else {
                    None
                }
            }
        }
    }

    /// Store the result of a child load against the addressed node.
    pub fn apply(&mut self, target: &ExplorerTarget, load: ExplorerLoad) {
        match (target, load) {
            (ExplorerTarget::FlightRoot { conn }, ExplorerLoad::Catalogs(result)) => {
                if let Some(tree) = self.flights.get_mut(*conn) {
                    tree.loading = false;
                    tree.loaded = true;
                    match result {
                        Ok(names) => {
                            tree.catalogs = names
                                .into_iter()
                                .map(|name| CatalogNode {
                                    name,
                                    ..Default::default()
                                })
                                .collect();
                        }
                        Err(e) => tree.error = Some(e.to_string()),
                    }
                }
            }
            (ExplorerTarget::Catalog { conn, catalog }, ExplorerLoad::Schemas(result)) => {
                if let Some(node) = self.catalog_mut(*conn, catalog) {
                    node.loading = false;
                    node.loaded = true;
                    match result {
                        Ok(names) => {
                            node.schemas = names
                                .into_iter()
                                .map(|name| SchemaNode {
                                    name,
                                    ..Default::default()
                                })
                                .collect();
                        }
                        Err(e) => node.error = Some(e.to_string()),
                    }
                }
            }
            (
                ExplorerTarget::Schema {
                    conn,
                    catalog,
                    schema,
                },
                ExplorerLoad::Tables(result),
            ) => {
                if let Some(node) = self.schema_mut(*conn, catalog, schema) {
                    node.loading = false;
                    node.loaded = true;
                    match result {
                        Ok(entries) => {
                            node.tables = entries
                                .into_iter()
                                .map(|e| TableNode {
                                    name: e.table,
                                    table_type: e.table_type,
                                    ..Default::default()
                                })
                                .collect();
                        }
                        Err(e) => node.error = Some(e.to_string()),
                    }
                }
            }
            (
                ExplorerTarget::Table {
                    conn,
                    catalog,
                    schema,
                    table,
                },
                ExplorerLoad::Columns(result),
            ) => {
                if let Some(node) = self.table_mut(*conn, catalog, schema, table) {
                    node.loading = false;
                    node.loaded = true;
                    match result {
                        Ok(cols) => node.columns = cols,
                        Err(e) => node.error = Some(e.to_string()),
                    }
                }
            }
            _ => {}
        }
    }

    fn catalog_mut(&mut self, conn: usize, catalog: &str) -> Option<&mut CatalogNode> {
        self.flights
            .get_mut(conn)?
            .catalogs
            .iter_mut()
            .find(|c| c.name == catalog)
    }

    fn schema_mut(
        &mut self,
        conn: usize,
        catalog: &str,
        schema: &Option<String>,
    ) -> Option<&mut SchemaNode> {
        self.catalog_mut(conn, catalog)?
            .schemas
            .iter_mut()
            .find(|s| &s.name == schema)
    }

    fn table_mut(
        &mut self,
        conn: usize,
        catalog: &str,
        schema: &Option<String>,
        table: &str,
    ) -> Option<&mut TableNode> {
        self.schema_mut(conn, catalog, schema)?
            .tables
            .iter_mut()
            .find(|t| t.name == table)
    }

    /// Build a completion catalog for the shared local session: one table per
    /// open file, keyed by its registered name.
    pub fn local_catalog<'a>(
        tables: impl IntoIterator<Item = (&'a str, &'a SchemaRef)>,
    ) -> Catalog {
        let tables = tables
            .into_iter()
            .map(|(name, schema)| TableMeta {
                name: name.to_string(),
                qualified: name.to_string(),
                columns: schema
                    .fields()
                    .iter()
                    .map(|f| ColumnMeta {
                        name: f.name().clone(),
                        data_type: f.data_type().to_string(),
                    })
                    .collect(),
            })
            .collect();
        Catalog {
            databases: vec![Database {
                name: "datafusion".into(),
                schemas: vec![SchemaNs { name: None, tables }],
            }],
        }
    }

    /// Build a completion catalog from whatever has been loaded for `conn`.
    pub fn flight_catalog(&self, conn: usize) -> Catalog {
        let Some(tree) = self.flights.get(conn) else {
            return Catalog::default();
        };
        let databases = tree
            .catalogs
            .iter()
            .map(|c| Database {
                name: c.name.clone(),
                schemas: c
                    .schemas
                    .iter()
                    .map(|s| SchemaNs {
                        name: s.name.clone(),
                        tables: s
                            .tables
                            .iter()
                            .map(|t| TableMeta {
                                name: t.name.clone(),
                                qualified: qualify(&c.name, &s.name, &t.name),
                                columns: t
                                    .columns
                                    .iter()
                                    .map(|col| ColumnMeta {
                                        name: col.name.clone(),
                                        data_type: col.data_type.clone(),
                                    })
                                    .collect(),
                            })
                            .collect(),
                    })
                    .collect(),
            })
            .collect();
        Catalog { databases }
    }
}

/// Join a catalog/schema/table into a dotted, query-ready name.
pub fn qualify(catalog: &str, schema: &Option<String>, table: &str) -> String {
    let mut parts = Vec::new();
    if !catalog.is_empty() {
        parts.push(catalog.to_string());
    }
    if let Some(s) = schema
        && !s.is_empty()
    {
        parts.push(s.clone());
    }
    parts.push(table.to_string());
    parts.join(".")
}
