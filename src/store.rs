//! App-managed persistent state, stored as Parquet in the OS *data* dir
//! (distinct from the human-edited `config.toml` in the *config* dir).
//!
//! Three small whole-file snapshots are rewritten on change — query history,
//! per-schema column widths, and recently opened files. Parquet (rather than a
//! bespoke format) is deliberate: the files are also queryable by the app's own
//! DataFusion engine, and we already depend on the `parquet`/`arrow` writers.
//!
//! Everything here is best-effort, mirroring [`crate::config::Config`]: a load
//! failure yields empty defaults and a save failure is logged and swallowed, so
//! a corrupt or missing state file never blocks startup.

use std::collections::HashMap;
use std::fs::File;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use arrow::array::{Array, Float32Array, Int32Array, Int64Array, StringArray};
use arrow::datatypes::{DataType, Field, Schema};
use arrow::record_batch::RecordBatch;
use parquet::arrow::ArrowWriter;
use parquet::arrow::arrow_reader::ParquetRecordBatchReaderBuilder;
use parquet::basic::Compression;
use parquet::file::properties::WriterProperties;

use crate::app::{HistoryStatus, QueryHistoryEntry};

const HISTORY_FILE: &str = "query_history.parquet";
const WIDTHS_FILE: &str = "column_widths.parquet";
const RECENT_FILE: &str = "recent_files.parquet";

/// A recently opened data file, newest first.
#[derive(Debug, Clone)]
pub struct RecentFile {
    pub path: String,
    pub last_opened_ms: i64,
}

/// Handle to the on-disk state directory. Cheap to clone (`Option<PathBuf>`),
/// so it can be moved into an Iced `Task` for off-thread saves. A `None` dir
/// (no data dir available) silently disables persistence.
#[derive(Debug, Clone, Default)]
pub struct StateStore {
    dir: PathBuf,
}

impl StateStore {
    /// Resolve the OS data dir (e.g. `~/.local/share/datafusion-ui`).
    pub fn new(app_dir: impl AsRef<Path>) -> Self {
        StateStore {
            dir: app_dir.as_ref().join("data"),
        }
    }

    /// Construct a store rooted at an explicit directory (used in tests).
    #[cfg(test)]
    pub fn with_dir(dir: PathBuf) -> Self {
        StateStore { dir }
    }

    fn file(&self, name: &str) -> Option<PathBuf> {
        Some(self.dir.join(name))
    }

    /// Path to a state file (used to register it as a queryable table).
    pub fn file_path(&self, name: StateFile) -> Option<PathBuf> {
        let p = self.file(name.filename())?;
        p.exists().then_some(p)
    }

    /// Write `batch` to `name` (Snappy Parquet, single file), creating the dir.
    fn write(&self, name: &str, batch: &RecordBatch) {
        let Some(path) = self.file(name) else {
            return;
        };
        if let Some(parent) = path.parent()
            && let Err(e) = std::fs::create_dir_all(parent)
        {
            tracing::warn!(error = %e, "could not create state dir");
            return;
        }
        let props = WriterProperties::builder()
            .set_compression(Compression::SNAPPY)
            .build();
        let result = (|| {
            let file = File::create(&path)?;
            let mut writer = ArrowWriter::try_new(file, batch.schema(), Some(props))?;
            writer.write(batch)?;
            writer.close()?;
            Ok::<_, parquet::errors::ParquetError>(())
        })();
        match result {
            Ok(()) => tracing::debug!(path = %path.display(), "saved state file"),
            Err(e) => {
                tracing::warn!(error = %e, path = %path.display(), "could not write state file")
            }
        }
    }

    /// Read every batch from `name`, or an empty Vec if missing/unreadable.
    fn read(&self, name: &str) -> Vec<RecordBatch> {
        let Some(path) = self.file(name) else {
            return Vec::new();
        };
        if !path.exists() {
            return Vec::new();
        }
        let result = (|| {
            let file = File::open(&path)?;
            let reader = ParquetRecordBatchReaderBuilder::try_new(file)?.build()?;
            reader.collect::<Result<Vec<_>, _>>()
        })();
        match result {
            Ok(batches) => batches,
            Err(e) => {
                tracing::warn!(error = %e, path = %path.display(), "could not read state file; ignoring");
                Vec::new()
            }
        }
    }

    // -- Query history --------------------------------------------------------

    pub fn load_history(&self) -> Vec<QueryHistoryEntry> {
        let mut out = Vec::new();
        for batch in self.read(HISTORY_FILE) {
            let sql = str_col(&batch, "sql");
            let source = str_col(&batch, "source_label");
            let status = str_col(&batch, "status");
            let error = str_col(&batch, "error");
            let row_count = i64_col(&batch, "row_count");
            let elapsed = i64_col(&batch, "elapsed_ms");
            let ran_at = i64_col(&batch, "ran_at_ms");
            for r in 0..batch.num_rows() {
                let status = match get_str(&status, r) {
                    Some("err") => {
                        HistoryStatus::Err(get_str(&error, r).unwrap_or_default().to_string())
                    }
                    _ => HistoryStatus::Ok,
                };
                out.push(QueryHistoryEntry {
                    sql: get_str(&sql, r).unwrap_or_default().to_string(),
                    source_label: get_str(&source, r).unwrap_or_default().to_string(),
                    status,
                    row_count: get_i64(&row_count, r).map(|v| v as usize),
                    elapsed_ms: get_i64(&elapsed, r).unwrap_or(0) as u128,
                    ran_at: millis_to_systemtime(get_i64(&ran_at, r).unwrap_or(0)),
                });
            }
        }
        out
    }

    pub fn save_history(&self, entries: &[QueryHistoryEntry]) {
        let schema = Arc::new(Schema::new(vec![
            Field::new("sql", DataType::Utf8, false),
            Field::new("source_label", DataType::Utf8, false),
            Field::new("status", DataType::Utf8, false),
            Field::new("error", DataType::Utf8, true),
            Field::new("row_count", DataType::Int64, true),
            Field::new("elapsed_ms", DataType::Int64, false),
            Field::new("ran_at_ms", DataType::Int64, false),
        ]));
        let sql = StringArray::from_iter_values(entries.iter().map(|e| e.sql.as_str()));
        let source = StringArray::from_iter_values(entries.iter().map(|e| e.source_label.as_str()));
        let status = StringArray::from_iter_values(entries.iter().map(|e| match &e.status {
            HistoryStatus::Ok => "ok",
            HistoryStatus::Err(_) => "err",
        }));
        let error = StringArray::from(
            entries
                .iter()
                .map(|e| match &e.status {
                    HistoryStatus::Err(msg) => Some(msg.clone()),
                    HistoryStatus::Ok => None,
                })
                .collect::<Vec<_>>(),
        );
        let row_count = Int64Array::from(
            entries
                .iter()
                .map(|e| e.row_count.map(|v| v as i64))
                .collect::<Vec<_>>(),
        );
        let elapsed = Int64Array::from_iter_values(entries.iter().map(|e| e.elapsed_ms as i64));
        let ran_at =
            Int64Array::from_iter_values(entries.iter().map(|e| systemtime_to_millis(e.ran_at)));
        match RecordBatch::try_new(
            schema,
            vec![
                Arc::new(sql),
                Arc::new(source),
                Arc::new(status),
                Arc::new(error),
                Arc::new(row_count),
                Arc::new(elapsed),
                Arc::new(ran_at),
            ],
        ) {
            Ok(batch) => self.write(HISTORY_FILE, &batch),
            Err(e) => tracing::warn!(error = %e, "could not build history batch"),
        }
    }

    // -- Column widths --------------------------------------------------------

    pub fn load_column_widths(&self) -> HashMap<String, Vec<f32>> {
        // Collect (sig, col_index, width) rows, then assemble each signature's
        // widths in column order.
        let mut rows: HashMap<String, Vec<(i32, f32)>> = HashMap::new();
        for batch in self.read(WIDTHS_FILE) {
            let sig = str_col(&batch, "schema_sig");
            let idx = i32_col(&batch, "col_index");
            let width = f32_col(&batch, "width");
            for r in 0..batch.num_rows() {
                let Some(s) = get_str(&sig, r) else { continue };
                rows.entry(s.to_string()).or_default().push((
                    get_i32(&idx, r).unwrap_or(0),
                    get_f32(&width, r).unwrap_or(0.0),
                ));
            }
        }
        rows.into_iter()
            .map(|(sig, mut cols)| {
                cols.sort_by_key(|(i, _)| *i);
                (sig, cols.into_iter().map(|(_, w)| w).collect())
            })
            .collect()
    }

    pub fn save_column_widths(&self, widths: &HashMap<String, Vec<f32>>) {
        let schema = Arc::new(Schema::new(vec![
            Field::new("schema_sig", DataType::Utf8, false),
            Field::new("col_index", DataType::Int32, false),
            Field::new("width", DataType::Float32, false),
        ]));
        let mut sigs: Vec<&str> = Vec::new();
        let mut indices: Vec<i32> = Vec::new();
        let mut vals: Vec<f32> = Vec::new();
        for (sig, cols) in widths {
            for (i, w) in cols.iter().enumerate() {
                sigs.push(sig.as_str());
                indices.push(i as i32);
                vals.push(*w);
            }
        }
        match RecordBatch::try_new(
            schema,
            vec![
                Arc::new(StringArray::from(sigs)),
                Arc::new(Int32Array::from(indices)),
                Arc::new(Float32Array::from(vals)),
            ],
        ) {
            Ok(batch) => self.write(WIDTHS_FILE, &batch),
            Err(e) => tracing::warn!(error = %e, "could not build column-widths batch"),
        }
    }

    // -- Recent files ---------------------------------------------------------

    pub fn load_recent_files(&self) -> Vec<RecentFile> {
        let mut out = Vec::new();
        for batch in self.read(RECENT_FILE) {
            let path = str_col(&batch, "path");
            let opened = i64_col(&batch, "last_opened_ms");
            for r in 0..batch.num_rows() {
                let Some(p) = get_str(&path, r) else { continue };
                out.push(RecentFile {
                    path: p.to_string(),
                    last_opened_ms: get_i64(&opened, r).unwrap_or(0),
                });
            }
        }
        out.sort_by_key(|r| std::cmp::Reverse(r.last_opened_ms));
        out
    }

    pub fn save_recent_files(&self, recent: &[RecentFile]) {
        let schema = Arc::new(Schema::new(vec![
            Field::new("path", DataType::Utf8, false),
            Field::new("last_opened_ms", DataType::Int64, false),
        ]));
        let path = StringArray::from_iter_values(recent.iter().map(|r| r.path.as_str()));
        let opened = Int64Array::from_iter_values(recent.iter().map(|r| r.last_opened_ms));
        match RecordBatch::try_new(schema, vec![Arc::new(path), Arc::new(opened)]) {
            Ok(batch) => self.write(RECENT_FILE, &batch),
            Err(e) => tracing::warn!(error = %e, "could not build recent-files batch"),
        }
    }
}

/// The persisted state files, for registering as queryable DataFusion tables.
#[derive(Debug, Clone, Copy)]
pub enum StateFile {
    History,
    ColumnWidths,
    RecentFiles,
}

impl StateFile {
    fn filename(self) -> &'static str {
        match self {
            StateFile::History => HISTORY_FILE,
            StateFile::ColumnWidths => WIDTHS_FILE,
            StateFile::RecentFiles => RECENT_FILE,
        }
    }

    /// The table name this file registers under in the local session.
    pub fn table_name(self) -> &'static str {
        match self {
            StateFile::History => "history",
            StateFile::ColumnWidths => "column_widths",
            StateFile::RecentFiles => "recent_files",
        }
    }

    pub const ALL: [StateFile; 3] = [
        StateFile::History,
        StateFile::ColumnWidths,
        StateFile::RecentFiles,
    ];
}

fn systemtime_to_millis(t: SystemTime) -> i64 {
    t.duration_since(UNIX_EPOCH)
        .map(|d| d.as_millis() as i64)
        .unwrap_or(0)
}

fn millis_to_systemtime(ms: i64) -> SystemTime {
    UNIX_EPOCH + Duration::from_millis(ms.max(0) as u64)
}

fn get_str(a: &StringArray, r: usize) -> Option<&str> {
    (r < a.len() && a.is_valid(r)).then(|| a.value(r))
}

fn get_i64(a: &Int64Array, r: usize) -> Option<i64> {
    (r < a.len() && a.is_valid(r)).then(|| a.value(r))
}

fn get_i32(a: &Int32Array, r: usize) -> Option<i32> {
    (r < a.len() && a.is_valid(r)).then(|| a.value(r))
}

fn get_f32(a: &Float32Array, r: usize) -> Option<f32> {
    (r < a.len() && a.is_valid(r)).then(|| a.value(r))
}
fn get_col<A: Array + Clone + 'static>(batch: &RecordBatch, name: &str) -> Option<A> {
    batch
        .column_by_name(name)
        .and_then(|c| c.as_any().downcast_ref::<A>().cloned())
}
macro_rules! get_col_or_null {
    (
        $(
            $get_fn:ident($array_ty:ty)
        ),*
    ) => {
        $(
            #[inline(always)]
            fn $get_fn(batch: &RecordBatch, name: &str) -> $array_ty {
                get_col(batch, name)
                    .unwrap_or_else(|| <$array_ty>::new_null(batch.num_rows()))
            }
        )*
    };
}

get_col_or_null!(
    str_col(StringArray),
    i64_col(Int64Array),
    i32_col(Int32Array),
    f32_col(Float32Array)
);

#[cfg(test)]
mod tests {
    use super::*;
    use std::time::Duration;

    fn temp_dir(tag: &str) -> PathBuf {
        let mut d = std::env::temp_dir();
        d.push(format!("dfui-store-test-{}-{tag}", std::process::id()));
        let _ = std::fs::remove_dir_all(&d);
        d
    }

    #[test]
    fn history_round_trip() {
        let store = StateStore::with_dir(temp_dir("history"));
        let entries = vec![
            QueryHistoryEntry {
                sql: "SELECT 1".into(),
                source_label: "local session".into(),
                status: HistoryStatus::Ok,
                row_count: Some(1),
                elapsed_ms: 12,
                ran_at: UNIX_EPOCH + Duration::from_millis(1_700_000_000_000),
            },
            QueryHistoryEntry {
                sql: "SELECT bad".into(),
                source_label: "flight: x".into(),
                status: HistoryStatus::Err("boom".into()),
                row_count: None,
                elapsed_ms: 3,
                ran_at: UNIX_EPOCH + Duration::from_millis(1_700_000_001_000),
            },
        ];
        store.save_history(&entries);
        let loaded = store.load_history();
        assert_eq!(loaded.len(), 2);
        assert_eq!(loaded[0].sql, "SELECT 1");
        assert_eq!(loaded[0].row_count, Some(1));
        assert!(matches!(loaded[0].status, HistoryStatus::Ok));
        assert_eq!(loaded[1].row_count, None);
        match &loaded[1].status {
            HistoryStatus::Err(m) => assert_eq!(m, "boom"),
            HistoryStatus::Ok => panic!("expected error status"),
        }
        let _ = std::fs::remove_dir_all(&store.dir);
    }

    #[test]
    fn widths_and_recent_round_trip() {
        let store = StateStore::with_dir(temp_dir("widths"));
        let mut widths = HashMap::new();
        widths.insert("a:Int32|b:Utf8".to_string(), vec![120.0, 88.5]);
        store.save_column_widths(&widths);
        let loaded = store.load_column_widths();
        assert_eq!(loaded.get("a:Int32|b:Utf8"), Some(&vec![120.0, 88.5]));

        let recent = vec![
            RecentFile {
                path: "/a.parquet".into(),
                last_opened_ms: 100,
            },
            RecentFile {
                path: "/b.parquet".into(),
                last_opened_ms: 200,
            },
        ];
        store.save_recent_files(&recent);
        let loaded = store.load_recent_files();
        // Sorted newest-first.
        assert_eq!(loaded[0].path, "/b.parquet");
        assert_eq!(loaded[1].path, "/a.parquet");
        let _ = std::fs::remove_dir_all(store.dir);
    }
}
