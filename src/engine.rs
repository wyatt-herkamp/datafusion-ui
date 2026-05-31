//! `QueryEngine` — the single abstraction the SQL editor talks to. A query runs
//! against either the open Parquet file's in-process DataFusion session or a
//! connected FlightSQL server. Both arms are `Arc`-backed, so the engine is
//! `Clone + Send + 'static` and can be moved into an Iced `Task::perform`.

use std::sync::Arc;

use arrow::datatypes::SchemaRef;
use arrow::record_batch::RecordBatch;

use crate::error::{AppError, QueryError};
use crate::flightsql::FlightSqlClient;
use crate::wrangle::SharedSession;
use crate::wrangle::session::run_sql_capped_on;

#[derive(Debug, Clone)]
pub enum QueryEngine {
    /// The shared local DataFusion session (all open files registered by name).
    Local(Arc<SharedSession>),
    Flight(Arc<FlightSqlClient>),
}

#[derive(Debug, Clone)]
pub struct QueryResult {
    pub batch: RecordBatch,
    pub schema: SchemaRef,
    pub row_count: usize,
    /// True if the result was cut short at the row cap.
    pub truncated: bool,
}

impl QueryEngine {
    /// Short label for tab titles and history entries.
    pub fn source_label(&self) -> String {
        match self {
            QueryEngine::Local(_) => "local session".to_string(),
            QueryEngine::Flight(client) => format!("flight: {}", client.label),
        }
    }

    /// Run arbitrary SQL, collecting up to `cap` rows into one batch.
    /// Clone-and-move into `Task::perform`.
    pub async fn run_query(self, sql: String, cap: usize) -> Result<QueryResult, AppError> {
        let (batch, schema, truncated) = match self {
            QueryEngine::Local(session) => run_sql_capped_on(&session.ctx, sql, cap).await?,
            QueryEngine::Flight(client) => client.run_sql(sql, cap).await?,
        };
        let row_count = batch.num_rows();
        Ok(QueryResult {
            batch,
            schema,
            row_count,
            truncated,
        })
    }

    /// Stream the full (uncapped) result of `sql` for export. Local runs through
    /// DataFusion's `execute_stream`; Flight re-fetches every endpoint. Both
    /// arms yield a `SendableRecordBatchStream` so the writer is engine-agnostic.
    pub async fn export_stream(
        self,
        sql: String,
    ) -> Result<datafusion::physical_plan::SendableRecordBatchStream, AppError> {
        match self {
            QueryEngine::Local(session) => {
                let df = session
                    .ctx
                    .sql(&sql)
                    .await
                    .map_err(|e| QueryError::Plan(e.to_string()))?;
                Ok(df
                    .execute_stream()
                    .await
                    .map_err(|e| QueryError::Collect(e.to_string()))?)
            }
            QueryEngine::Flight(client) => Ok(client.run_sql_stream(sql).await?),
        }
    }
}
