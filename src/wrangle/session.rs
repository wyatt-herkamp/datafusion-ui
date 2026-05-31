//! The shared local DataFusion session.
//!
//! All open Parquet files register as named tables in one [`SharedSession`]
//! (built from the user's [`SessionSettings`]), so the SQL Workspace can query
//! any of them by name and tell them apart.

use std::sync::Arc;

use arrow::datatypes::SchemaRef;
use arrow::record_batch::RecordBatch;
use datafusion::prelude::{ParquetReadOptions, SessionContext};

use crate::config::{RuntimeSettings, SessionSettings};
use crate::error::QueryError;

/// The single DataFusion context every open file registers into. Cheaply
/// clonable internals (the `SessionContext` is `Arc`-backed), but we keep one
/// canonical instance behind an `Arc<SharedSession>` so registrations made on
/// one handle are visible to all.
pub struct SharedSession {
    pub ctx: SessionContext,
}

impl std::fmt::Debug for SharedSession {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("SharedSession").finish_non_exhaustive()
    }
}

impl Default for SharedSession {
    fn default() -> Self {
        Self {
            ctx: SessionContext::new_with_config_rt(
                SessionSettings::default().to_session_config(),
                RuntimeSettings::default().to_runtime_env(),
            ),
        }
    }
}

impl SharedSession {
    /// Build a fresh shared session from the given session + runtime settings.
    pub fn new(session: &SessionSettings, runtime: &RuntimeSettings) -> Arc<Self> {
        let ctx = SessionContext::new_with_config_rt(
            session.to_session_config(),
            runtime.to_runtime_env(),
        );
        Arc::new(Self { ctx })
    }

    /// Register a Parquet file under `name`, returning its Arrow schema.
    pub async fn register_file(
        self: Arc<Self>,
        name: String,
        path: String,
    ) -> Result<SchemaRef, QueryError> {
        tracing::info!(name = %name, path = %path, "registering parquet in shared session");
        self.ctx
            .register_parquet(&name, &path, ParquetReadOptions::default())
            .await
            .map_err(|e| {
                tracing::error!(error = %e, "register parquet failed");
                QueryError::Register(e.to_string())
            })?;
        let df = self
            .ctx
            .table(&name)
            .await
            .map_err(|e| QueryError::Register(e.to_string()))?;
        Ok(Arc::new(df.schema().as_arrow().clone()))
    }

    /// (Re)register a Parquet file under `name`, replacing any existing table.
    /// Used for the app's own persistent-state files so they can be queried in
    /// SQL (e.g. `SELECT * FROM history`).
    pub async fn register_table_replace(
        self: Arc<Self>,
        name: &'static str,
        path: String,
    ) -> Result<(), QueryError> {
        let _ = self.ctx.deregister_table(name);
        self.ctx
            .register_parquet(name, &path, ParquetReadOptions::default())
            .await
            .map_err(|e| QueryError::Register(format!("{name}: {e}")))?;
        Ok(())
    }

    /// Drop a registered table (best-effort; logs on failure).
    pub fn deregister(&self, name: &str) {
        if let Err(e) = self.ctx.deregister_table(name) {
            tracing::warn!(error = %e, name, "deregister table failed");
        }
    }
}

/// Run arbitrary SQL against `ctx`, collecting whole batches until `cap` rows
/// are exceeded. Shared by the per-file handle and the shared-session local
/// query engine.
pub async fn run_sql_capped_on(
    ctx: &SessionContext,
    sql: String,
    cap: usize,
) -> Result<(RecordBatch, SchemaRef, bool), QueryError> {
    tracing::debug!(sql = %sql, "run_sql_capped");
    let df = ctx.sql(&sql).await.map_err(|e| {
        tracing::warn!(error = %e, sql = %sql, "sql failed");
        QueryError::Plan(e.to_string())
    })?;
    let schema: SchemaRef = Arc::new(df.schema().as_arrow().clone());
    let batches = df
        .collect()
        .await
        .map_err(|e| QueryError::Collect(e.to_string()))?;

    // Cap client-side: keep whole batches until we exceed `cap` rows.
    let mut kept: Vec<RecordBatch> = Vec::new();
    let mut rows = 0usize;
    let mut truncated = false;
    for b in batches {
        if rows >= cap {
            truncated = true;
            break;
        }
        let remaining = cap - rows;
        if b.num_rows() > remaining {
            kept.push(b.slice(0, remaining));
            truncated = true;
            break;
        }
        rows += b.num_rows();
        kept.push(b);
    }

    let batch = concat_or_empty(schema.clone(), kept)?;
    Ok((batch, schema, truncated))
}

pub(crate) fn concat_or_empty(
    schema: SchemaRef,
    batches: Vec<RecordBatch>,
) -> Result<RecordBatch, QueryError> {
    if batches.is_empty() {
        return Ok(RecordBatch::new_empty(schema));
    }
    if batches.len() == 1 {
        return Ok(batches.into_iter().next().unwrap());
    }
    arrow::compute::concat_batches(&schema, &batches).map_err(|e| QueryError::Concat(e.to_string()))
}
