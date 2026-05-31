//! Minimal FlightSQL client: connect to a server, run a SQL statement, and
//! collect the result into a single Arrow `RecordBatch`.
//!
//! Modeled on datafusion-dft's FlightSQL support, trimmed to what the UI needs
//! (no benchmarking, no metadata RPCs). `FlightSqlServiceClient<Channel>` is
//! `Clone` (the underlying tonic `Channel` multiplexes over one connection), so
//! each query clones the stored client and calls the `&mut self` RPCs on the
//! clone — no `Mutex` and no serialization between concurrent queries.
//!
//! Known limitations (acceptable for v1):
//! - Plaintext `http://` only; `https://` would need a tonic TLS feature.
//! - `FlightInfo.endpoint[*].location` is ignored — every ticket is fetched over
//!   the original channel. Fine for single-node servers (the common case).

use std::sync::Arc;
use std::time::Instant;

use arrow::array::{Array, StringArray, UInt32Array, UnionArray};
use arrow::datatypes::SchemaRef;
use arrow::record_batch::RecordBatch;
use arrow::util::display::array_value_to_string;
use arrow_flight::FlightInfo;
use arrow_flight::sql::client::FlightSqlServiceClient;
use arrow_flight::sql::{CommandGetDbSchemas, CommandGetTables, SqlInfo};
use base64::Engine as _;
use base64::engine::general_purpose::STANDARD as BASE64;
use futures::TryStreamExt;
use tonic::transport::{Channel, Endpoint};

use datafusion::physical_plan::stream::RecordBatchStreamAdapter;

use crate::error::FlightError;
use crate::wrangle::session::concat_or_empty;

/// A table returned by `list_tables`. The catalog/schema are already known from
/// the request, so only the table's own name and type are carried.
#[derive(Debug, Clone)]
pub struct TableEntry {
    pub table: String,
    pub table_type: String,
}

/// A column returned by `table_columns`.
#[derive(Debug, Clone)]
pub struct ColumnEntry {
    pub name: String,
    pub data_type: String,
}

#[derive(Debug, Clone)]
pub struct FlightSqlConfig {
    pub url: String,
    pub auth: Option<FlightAuth>,
}

#[derive(Debug, Clone)]
pub enum FlightAuth {
    Bearer(String),
    Basic { user: String, pass: String },
}

pub struct FlightSqlClient {
    client: FlightSqlServiceClient<Channel>,
    /// Short `host:port` label for the UI and query history.
    pub label: String,
    /// The full endpoint URL the connection was opened against.
    pub url: String,
}

impl std::fmt::Debug for FlightSqlClient {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("FlightSqlClient")
            .field("label", &self.label)
            .finish_non_exhaustive()
    }
}

impl FlightSqlClient {
    pub async fn connect(cfg: FlightSqlConfig) -> Result<Arc<Self>, FlightError> {
        tracing::info!(url = %cfg.url, "connecting to flightsql server");
        let endpoint = Endpoint::from_shared(cfg.url.clone())
            .map_err(|e| FlightError::InvalidUrl(e.to_string()))?;
        let channel = endpoint
            .connect()
            .await
            .map_err(|e| FlightError::Connect(e.to_string()))?;
        let mut client = FlightSqlServiceClient::new(channel);

        match &cfg.auth {
            Some(FlightAuth::Bearer(token)) => client.set_token(token.clone()),
            Some(FlightAuth::Basic { user, pass }) => {
                // Standard FlightSQL handshake: exchange credentials; if the server
                // returns a bearer token it is stored on the client automatically.
                // Also set a static Basic header so servers that authenticate every
                // request (rather than via handshake) still work.
                let encoded = BASE64.encode(format!("{user}:{pass}"));
                client.set_header("authorization", format!("Basic {encoded}"));
                if let Err(e) = client.handshake(user, pass).await {
                    tracing::warn!(error = %e, "flightsql handshake failed; relying on Basic header");
                }
            }
            None => {}
        }

        let label = label_from_url(&cfg.url);
        Ok(Arc::new(Self {
            client,
            label,
            url: cfg.url,
        }))
    }

    /// Round-trip a lightweight metadata RPC and report the elapsed time in ms.
    pub async fn ping(self: Arc<Self>) -> Result<u128, FlightError> {
        let mut client = self.client.clone();
        let started = Instant::now();
        client.get_catalogs().await.map_err(|e| FlightError::Rpc {
            op: "ping",
            msg: e.to_string(),
        })?;
        Ok(started.elapsed().as_millis())
    }

    /// Fetch a curated set of server metadata values (FlightSQL `GetSqlInfo`),
    /// decoded into display `(label, value)` pairs. Returns an empty list if the
    /// server doesn't implement the RPC.
    pub async fn server_info(self: Arc<Self>) -> Result<Vec<(String, String)>, FlightError> {
        // (SqlInfo key, human label) — requested in this order.
        const REQUESTED: &[(SqlInfo, &str)] = &[
            (SqlInfo::FlightSqlServerName, "Server"),
            (SqlInfo::FlightSqlServerVersion, "Version"),
            (SqlInfo::FlightSqlServerArrowVersion, "Arrow version"),
            (SqlInfo::FlightSqlServerReadOnly, "Read-only"),
        ];

        let mut client = self.client.clone();
        let info = client
            .get_sql_info(REQUESTED.iter().map(|(k, _)| *k).collect())
            .await
            .map_err(|e| FlightError::Rpc {
                op: "get_sql_info",
                msg: e.to_string(),
            })?;
        let batches = Self::collect_metadata(&mut client, info).await?;

        let mut out = Vec::new();
        for batch in &batches {
            let Some(names) = batch
                .column_by_name("info_name")
                .and_then(|c| c.as_any().downcast_ref::<UInt32Array>())
            else {
                continue;
            };
            let Some(values) = batch
                .column_by_name("value")
                .and_then(|c| c.as_any().downcast_ref::<UnionArray>())
            else {
                continue;
            };
            for i in 0..batch.num_rows() {
                let key = names.value(i);
                let label = REQUESTED
                    .iter()
                    .find(|(k, _)| *k as u32 == key)
                    .map(|(_, l)| l.to_string())
                    .unwrap_or_else(|| format!("info {key}"));
                // The active union slot is a length-1 array; format slot 0.
                let slot = values.value(i);
                let value =
                    array_value_to_string(slot.as_ref(), 0).unwrap_or_else(|_| "?".to_string());
                out.push((label, value));
            }
        }
        Ok(out)
    }

    /// Execute `sql` and collect up to `cap` rows into one `RecordBatch`.
    /// Returns `(batch, schema, truncated)` where `truncated` is true if the
    /// result was cut short at `cap`.
    pub async fn run_sql(
        self: Arc<Self>,
        sql: String,
        cap: usize,
    ) -> Result<(RecordBatch, SchemaRef, bool), FlightError> {
        // Clone the stored client so we can call its `&mut self` RPCs without a lock.
        let mut client = self.client.clone();
        tracing::debug!(sql = %sql, "flightsql execute");
        let info = client
            .execute(sql, None)
            .await
            .map_err(|e| FlightError::Rpc {
                op: "execute",
                msg: e.to_string(),
            })?;

        let mut batches: Vec<RecordBatch> = Vec::new();
        let mut schema: Option<SchemaRef> = None;
        let mut rows = 0usize;
        let mut truncated = false;

        'outer: for endpoint in info.endpoint.iter() {
            let Some(ticket) = endpoint.ticket.clone() else {
                continue;
            };
            let mut stream = client.do_get(ticket).await.map_err(|e| FlightError::Rpc {
                op: "do_get",
                msg: e.to_string(),
            })?;
            while let Some(batch) = stream.try_next().await.map_err(|e| FlightError::Rpc {
                op: "stream",
                msg: e.to_string(),
            })? {
                if schema.is_none() {
                    schema = Some(batch.schema());
                }
                rows += batch.num_rows();
                batches.push(batch);
                if rows >= cap {
                    truncated = true;
                    break 'outer;
                }
            }
        }

        let schema = schema.unwrap_or_else(|| Arc::new(arrow::datatypes::Schema::empty()));
        let batch = concat_or_empty(schema.clone(), batches).map_err(|e| FlightError::Rpc {
            op: "concat",
            msg: e.to_string(),
        })?;
        tracing::debug!(rows = batch.num_rows(), truncated, "flightsql result");
        Ok((batch, schema, truncated))
    }

    /// Execute `sql` and stream *every* row (no cap) for export. Endpoints are
    /// fetched eagerly into memory, then replayed as a `SendableRecordBatchStream`
    /// so the unified writer can treat local and remote sources identically.
    pub async fn run_sql_stream(
        self: Arc<Self>,
        sql: String,
    ) -> Result<datafusion::physical_plan::SendableRecordBatchStream, FlightError> {
        let mut client = self.client.clone();
        tracing::debug!(sql = %sql, "flightsql export stream");
        let info = client
            .execute(sql, None)
            .await
            .map_err(|e| FlightError::Rpc {
                op: "execute",
                msg: e.to_string(),
            })?;

        let mut batches: Vec<RecordBatch> = Vec::new();
        let mut schema: Option<SchemaRef> = None;
        for endpoint in info.endpoint.iter() {
            let Some(ticket) = endpoint.ticket.clone() else {
                continue;
            };
            let mut stream = client.do_get(ticket).await.map_err(|e| FlightError::Rpc {
                op: "do_get",
                msg: e.to_string(),
            })?;
            while let Some(batch) = stream.try_next().await.map_err(|e| FlightError::Rpc {
                op: "stream",
                msg: e.to_string(),
            })? {
                if schema.is_none() {
                    schema = Some(batch.schema());
                }
                batches.push(batch);
            }
        }

        let schema = schema.unwrap_or_else(|| Arc::new(arrow::datatypes::Schema::empty()));
        let stream = futures::stream::iter(
            batches
                .into_iter()
                .map(|b| Ok(b) as datafusion::error::Result<RecordBatch>),
        );
        Ok(Box::pin(RecordBatchStreamAdapter::new(schema, stream)))
    }

    /// Fetch every batch behind a metadata `FlightInfo` (no row cap — metadata
    /// result sets are small). Shared by the catalog-listing RPCs below.
    async fn collect_metadata(
        client: &mut FlightSqlServiceClient<Channel>,
        info: FlightInfo,
    ) -> Result<Vec<RecordBatch>, FlightError> {
        let mut batches = Vec::new();
        for endpoint in info.endpoint.iter() {
            let Some(ticket) = endpoint.ticket.clone() else {
                continue;
            };
            let mut stream = client.do_get(ticket).await.map_err(|e| FlightError::Rpc {
                op: "do_get",
                msg: e.to_string(),
            })?;
            while let Some(batch) = stream.try_next().await.map_err(|e| FlightError::Rpc {
                op: "stream",
                msg: e.to_string(),
            })? {
                batches.push(batch);
            }
        }
        Ok(batches)
    }

    /// List catalog names available on the server (FlightSQL `GetCatalogs`).
    pub async fn list_catalogs(self: Arc<Self>) -> Result<Vec<String>, FlightError> {
        let mut client = self.client.clone();
        let info = client.get_catalogs().await.map_err(|e| FlightError::Rpc {
            op: "get_catalogs",
            msg: e.to_string(),
        })?;
        let batches = Self::collect_metadata(&mut client, info).await?;
        let mut out = Vec::new();
        for batch in &batches {
            out.extend(string_column(batch, "catalog_name")?.into_iter().flatten());
        }
        Ok(out)
    }

    /// List schema names, optionally narrowed to one catalog (FlightSQL
    /// `GetDbSchemas`). A `None` entry is the server's unnamed schema.
    pub async fn list_schemas(
        self: Arc<Self>,
        catalog: Option<String>,
    ) -> Result<Vec<Option<String>>, FlightError> {
        let mut client = self.client.clone();
        let info = client
            .get_db_schemas(CommandGetDbSchemas {
                catalog,
                db_schema_filter_pattern: None,
            })
            .await
            .map_err(|e| FlightError::Rpc {
                op: "get_db_schemas",
                msg: e.to_string(),
            })?;
        let batches = Self::collect_metadata(&mut client, info).await?;
        let mut out = Vec::new();
        for batch in &batches {
            out.extend(string_column(batch, "db_schema_name")?);
        }
        Ok(out)
    }

    /// List tables within a catalog/schema (FlightSQL `GetTables`).
    pub async fn list_tables(
        self: Arc<Self>,
        catalog: Option<String>,
        schema: Option<String>,
    ) -> Result<Vec<TableEntry>, FlightError> {
        let mut client = self.client.clone();
        let info = client
            .get_tables(CommandGetTables {
                catalog,
                db_schema_filter_pattern: schema,
                table_name_filter_pattern: None,
                table_types: Vec::new(),
                include_schema: false,
            })
            .await
            .map_err(|e| FlightError::Rpc {
                op: "get_tables",
                msg: e.to_string(),
            })?;
        let batches = Self::collect_metadata(&mut client, info).await?;
        let mut out = Vec::new();
        for batch in &batches {
            let tables = string_column(batch, "table_name")?;
            let types = string_column(batch, "table_type")?;
            for i in 0..batch.num_rows() {
                let Some(table) = tables.get(i).cloned().flatten() else {
                    continue;
                };
                out.push(TableEntry {
                    table,
                    table_type: types.get(i).cloned().flatten().unwrap_or_default(),
                });
            }
        }
        Ok(out)
    }

    /// Resolve a table's columns. Rather than decode the optional IPC
    /// `table_schema` blob, this runs `SELECT * FROM <qualified> LIMIT 0` and
    /// reads the result schema — robust and reuses the existing query path.
    pub async fn table_columns(
        self: Arc<Self>,
        qualified: String,
    ) -> Result<Vec<ColumnEntry>, FlightError> {
        let sql = format!("SELECT * FROM {qualified} LIMIT 0");
        let (_, schema, _) = self.run_sql(sql, 1).await?;
        Ok(schema
            .fields()
            .iter()
            .map(|f| ColumnEntry {
                name: f.name().clone(),
                data_type: f.data_type().to_string(),
            })
            .collect())
    }
}

/// Read a Utf8 column by name into owned optional strings. Errors if the column
/// is missing or not a `StringArray`.
fn string_column(batch: &RecordBatch, name: &str) -> Result<Vec<Option<String>>, FlightError> {
    let Some(array) = batch.column_by_name(name) else {
        return Err(FlightError::Metadata(format!(
            "batch missing column `{name}`"
        )));
    };
    let array = array
        .as_any()
        .downcast_ref::<StringArray>()
        .ok_or_else(|| FlightError::Metadata(format!("column `{name}` is not Utf8")))?;
    Ok((0..array.len())
        .map(|i| {
            if array.is_null(i) {
                None
            } else {
                Some(array.value(i).to_string())
            }
        })
        .collect())
}

/// Best-effort `host:port` extraction for display, falling back to the raw url.
fn label_from_url(url: &str) -> String {
    let no_scheme = url.split_once("://").map(|(_, rest)| rest).unwrap_or(url);
    no_scheme.trim_end_matches('/').to_string()
}
