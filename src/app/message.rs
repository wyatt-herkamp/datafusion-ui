//! The application's message types. The top-level [`Message`] is split into
//! per-domain sub-enums so each handler module in `update/` owns one domain. The
//! `From<…> for Message` impls let a sub-message be built and `.into()`-converted
//! at a call site without spelling out the outer wrapper.

use std::path::PathBuf;
use std::sync::Arc;

use arrow::datatypes::SchemaRef;
use iced::widget::text_editor;

use crate::engine::QueryResult;
use crate::error::{AppError, FlightError, ParquetError, QueryError};
use crate::explorer::{ExplorerLoad, ExplorerTarget};
use crate::export::{ExportFormat, ParquetCompression};
use crate::flightsql::FlightSqlClient;
use crate::parquet_io::FileSummary;

use super::{AuthKind, FileId, FileView, ParentWindow, SourceRef};

/// All user/async actions, grouped by domain.
#[derive(Debug, Clone)]
pub enum Message {
    File(FileMessage),
    Sql(SqlMessage),
    Grid(GridMessage),
    Flight(FlightMessage),
    ConnInfo(ConnInfoMessage),
    Explorer(ExplorerMessage),
    Settings(SettingsMessage),
    /// Fire-and-forget completion of a background side effect (e.g. registering
    /// the persistent-state Parquet files as queryable tables).
    Ignore,
}

/// Opening / navigating / closing files, plus the per-file cell-detail + copy.
#[derive(Debug, Clone)]
pub enum FileMessage {
    OpenFilePressed,
    FilePicked(Option<PathBuf>),
    FileLoaded(Result<FileSummary, ParquetError>),
    /// Navigate to a per-file view (replaces the old global tab selection).
    SelectFileView {
        file: FileId,
        view: FileView,
    },
    /// Navigate to the shared SQL workspace.
    SelectSql,
    /// Close an open file and clean up its editors/selection.
    CloseFile {
        file: FileId,
    },
    /// The shared-session registration for an opened file completed.
    Registered {
        file: FileId,
        result: Result<SchemaRef, QueryError>,
    },
    RowGroupToggled(usize),
    ToggleSchemaRow(usize),
    CopyCell(String),
    ClearCopyNotice,
    /// The main window's native handles, captured at boot for dialog parenting.
    WindowReady(Option<ParentWindow>),
}

/// The shared, tabbed SQL editor workspace: editing, running, EXPLAIN, the
/// export modal, history, autocomplete, and the per-tab cell-detail.
#[derive(Debug, Clone)]
pub enum SqlMessage {
    NewQueryToggle,
    NewQueryForSource(SourceRef),
    EditorSelect(u64),
    EditorClose(u64),
    EditorAction(u64, text_editor::Action),
    /// Undo the last edit group in the editor (Ctrl/Cmd+Z). iced has no native
    /// undo, so this is driven by our own snapshot stack.
    Undo(u64),
    /// Redo the last undone edit group (Ctrl/Cmd+Shift+Z or Ctrl/Cmd+Y).
    Redo(u64),
    Run(u64),
    /// Run the editor's query wrapped in `EXPLAIN` (leaving the editor text as
    /// the user wrote it).
    Explain(u64),
    /// Run wrapped in `EXPLAIN ANALYZE`.
    ExplainAnalyze(u64),
    /// Toggle between the formatted plan view and the raw EXPLAIN grid.
    ExplainToggleRaw(u64),
    Completed {
        id: u64,
        sql: String,
        source_label: String,
        elapsed_ms: u128,
        result: Result<QueryResult, AppError>,
    },
    ExportOpen(u64),
    ExportCancel(u64),
    ExportSetFormat(u64, ExportFormat),
    ExportSetCompression(u64, ParquetCompression),
    ExportToggleHeader(u64),
    ExportToggleNdjson(u64),
    ExportDelimiter(u64, String),
    ExportConfirm(u64),
    ExportPathPicked {
        id: u64,
        path: Option<PathBuf>,
    },
    ExportCompleted {
        id: u64,
        result: Result<PathBuf, AppError>,
    },
    HistoryLoad(usize),
    HistoryRerun(usize),
    HistoryToggle,
    CompletionMove(u64, i32),
    CompletionAccept(u64, usize),
    CompletionAcceptSelected(u64),
    CompletionDismiss(u64),
    /// Expand a nested cell in a SQL editor's results grid.
    ShowCellDetail {
        id: u64,
        row: usize,
        col: usize,
    },
    CloseCellDetail {
        id: u64,
    },
    /// Jump the results grid to a (0-based) page; clamped to the valid range.
    SetResultPage {
        id: u64,
        page: usize,
    },
}

/// Resizable columns for a SQL editor's results grid (keyed by editor id).
#[derive(Debug, Clone)]
#[allow(clippy::enum_variant_names)] // every variant is about a Column; the prefix reads well here
pub enum GridMessage {
    /// Live width update while dragging a column's resize handle.
    ColumnResize { id: u64, col: usize, width: f32 },
    /// Drag finished — persist the editor grid's widths.
    ColumnResizeEnd { id: u64 },
    /// Double-click — auto-size the column to its widest visible value.
    ColumnAutofit { id: u64, col: usize },
}

/// The FlightSQL connect modal and connection lifecycle.
#[derive(Debug, Clone)]
pub enum FlightMessage {
    OpenConnectForm,
    CloseConnectForm,
    ConnectFormUrl(String),
    ConnectFormAuthKind(AuthKind),
    ConnectFormToken(String),
    ConnectFormUser(String),
    ConnectFormPass(String),
    ConnectSubmit,
    Connected(Result<Arc<FlightSqlClient>, FlightError>),
    Disconnect(usize),
}

/// The per-connection info / health panel (ping + server metadata).
#[derive(Debug, Clone)]
pub enum ConnInfoMessage {
    /// Open the info/health view for the connection at this index.
    Open(usize),
    /// Ping the connection at this index (measures RPC round-trip).
    Ping(usize),
    PingResult {
        conn: usize,
        result: Result<u128, FlightError>,
    },
    InfoResult {
        conn: usize,
        result: Result<Vec<(String, String)>, FlightError>,
    },
}

/// The object-explorer tree in the SQL view's sidebar.
#[derive(Debug, Clone)]
pub enum ExplorerMessage {
    PanelToggle,
    Toggle(ExplorerTarget),
    Loaded {
        target: ExplorerTarget,
        load: ExplorerLoad,
    },
    /// Insert `SELECT * FROM <qualified> LIMIT 100` into an editor bound to the
    /// connection the table belongs to (switching to / opening one as needed).
    InsertTable {
        conn: usize,
        qualified: String,
    },
}

/// The settings page.
#[derive(Debug, Clone)]
pub enum SettingsMessage {
    Open,
    SetUiScale(f32),
    SetTheme(crate::config::ThemeChoice),
    /// Edit a session/queries field on the draft by key (parsed on save).
    Field(SettingsField, String),
    /// Nudge a numeric draft field by a signed step (from the +/- buttons).
    FieldStep(SettingsField, i64),
    ToggleFlag(SettingsToggle),
    SetMemoryPool(crate::config::MemoryPoolKind),
    SetDiskManager(crate::config::DiskManagerKind),
    /// Open a folder picker for the spill directory.
    PickSpillDir,
    SpillDirPicked(Option<PathBuf>),
    Save,
    Cancel,
}

macro_rules! sub_message_from {
    ($($variant:ident => $ty:ty),* $(,)?) => {
        $(impl From<$ty> for Message {
            fn from(m: $ty) -> Self {
                Message::$variant(m)
            }
        })*
    };
}

sub_message_from! {
    File => FileMessage,
    Sql => SqlMessage,
    Grid => GridMessage,
    Flight => FlightMessage,
    ConnInfo => ConnInfoMessage,
    Explorer => ExplorerMessage,
    Settings => SettingsMessage,
}

/// Free-form (text-input) settings fields, parsed when saving.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SettingsField {
    TargetPartitions,
    BatchSize,
    ResultRowCap,
    DefaultCatalog,
    DefaultSchema,
    MemoryLimitMb,
    DiskManagerPath,
    MaxTempDirSizeMb,
}

/// Boolean settings toggled in place on the draft.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SettingsToggle {
    RepartitionFileScans,
    RepartitionJoins,
    RepartitionAggregations,
    RepartitionSorts,
    InformationSchema,
    CollectStatistics,
}
