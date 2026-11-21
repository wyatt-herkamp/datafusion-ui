//! Application state: the [`App`] struct and all the per-file / per-tab / dialog
//! state types it owns, plus their small inherent impls.

use std::collections::HashMap;
use std::path::PathBuf;
use std::sync::Arc;
use std::time::SystemTime;

use ahash::AHashSet;
use arrow::datatypes::SchemaRef;
use arrow::record_batch::RecordBatch;
use chrono::TimeDelta;
use iced::widget::text_editor;
use iced::widget::text_editor::{Action, Motion};

use crate::config::Config;
use crate::engine::QueryEngine;
use crate::explain::ExplainKind;
use crate::explorer::Explorer;
use crate::export::{CsvOptions, ExportFormat, JsonOptions, ParquetOptions};
use crate::flightsql::FlightSqlClient;
use crate::parquet_io::FileSummary;
use crate::store::{RecentFile, StateStore};
use iced::window::raw_window_handle::{
    DisplayHandle, HandleError, HasDisplayHandle, HasWindowHandle, RawDisplayHandle,
    RawWindowHandle, WindowHandle,
};

use crate::wrangle::SharedSession;
use crate::wrangle::insights::ColumnInsight;

/// The main window's native handles, captured once at boot so native file
/// dialogs (`rfd`) can be parented to the app window.
///
/// Stored as owned raw handles so the value is `'static` and can travel through
/// the iced message/task machinery. The `unsafe impl Send`/`Sync` mirrors what
/// `rfd::FileDialog` already does internally — the handle is only ever used to
/// parent a dialog while the (single, long-lived) main window is alive.
#[derive(Debug, Clone)]
pub struct ParentWindow {
    window: RawWindowHandle,
    display: RawDisplayHandle,
}

// SAFETY: same rationale as `rfd::FileDialog`'s own `unsafe impl Send/Sync` —
// the raw handle is carried solely to hand to the platform dialog backend.
unsafe impl Send for ParentWindow {}
unsafe impl Sync for ParentWindow {}

impl ParentWindow {
    /// Capture the handles from the live window (called inside `window::run`).
    pub fn from_window(w: &dyn iced::window::Window) -> Option<Self> {
        Some(ParentWindow {
            window: w.window_handle().ok()?.as_raw(),
            display: w.display_handle().ok()?.as_raw(),
        })
    }
}

impl HasWindowHandle for ParentWindow {
    fn window_handle(&self) -> Result<WindowHandle<'_>, HandleError> {
        // SAFETY: the main window outlives every dialog opened from the app.
        Ok(unsafe { WindowHandle::borrow_raw(self.window) })
    }
}

impl HasDisplayHandle for ParentWindow {
    fn display_handle(&self) -> Result<DisplayHandle<'_>, HandleError> {
        // SAFETY: see `window_handle`.
        Ok(unsafe { DisplayHandle::borrow_raw(self.display) })
    }
}

/// Stable per-file id, assigned on open. Survives close/reorder so late async
/// results route to (or are harmlessly dropped for) the right file.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct FileId(pub u64);

/// One open Parquet file: its metadata, the table name it's registered under in
/// the shared session, and the small per-file view state (Overview schema
/// expansion + Row Groups selection). Data is browsed through the SQL workspace.
#[derive(Debug)]
pub struct FileTab {
    pub id: FileId,
    pub summary: FileSummary,
    /// Name this file is registered under in the shared local session.
    pub table_name: String,
    pub selected_row_group: Option<usize>,
    /// Page of the Row Groups table, in units of [`ROW_GROUP_PAGE_SIZE`].
    pub row_group_page: usize,
    /// Page of the expanded group's column-chunk table, in units of [`ROW_GROUP_PAGE_SIZE`].
    pub chunk_page: usize,
    /// Expanded rows in the Overview schema tree.
    pub expanded_schema_rows: AHashSet<usize>,
    /// Whether `register_file` into the shared session has completed, so the
    /// file is queryable from the SQL workspace.
    pub registered: bool,
    pub register_error: Option<String>,
}

impl FileTab {
    /// A freshly opened file: registration is in flight, nothing expanded yet.
    pub(crate) fn new(id: FileId, summary: FileSummary, table_name: String) -> Self {
        FileTab {
            id,
            summary,
            table_name,
            selected_row_group: None,
            row_group_page: 0,
            chunk_page: 0,
            expanded_schema_rows: AHashSet::new(),
            registered: false,
            register_error: None,
        }
    }
}

/// What the main content area is currently showing. Replaces the old global
/// `Tab`: navigation is now driven by the explorer sidebar.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum Selection {
    /// No file/connection-driven view selected (welcome / empty state).
    #[default]
    Welcome,
    /// A per-file view (Overview / Row Groups / Data) for the file `id`.
    File { id: FileId, view: FileView },
    /// The shared SQL workspace (its editor tab strip lives in the content).
    Sql,
    /// Info/health for the FlightSQL connection at this index.
    ConnInfo { conn: usize },
    /// The settings page.
    Settings,
}

/// Which per-file view is shown for a selected file.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum FileView {
    Overview,
    RowGroups,
}

#[derive(Debug, Default)]
pub struct App {
    /// Persisted user settings (appearance, session config, row cap).
    pub config: Config,
    /// Draft of the settings being edited on the Settings page (committed on save).
    pub settings_draft: Option<Config>,
    /// The shared local DataFusion session; every open file registers here.
    pub local: Arc<SharedSession>,

    /// Open Parquet files, each registered under a name in the shared session.
    pub files: Vec<FileTab>,
    /// Monotonic source of [`FileId`]s.
    pub next_file_id: u64,
    /// What the main content area is showing.
    pub selection: Selection,

    pub error: Option<String>,
    pub loading: bool,

    /// Active FlightSQL connections (the SQL workspace can target any of them).
    pub connections: Vec<Arc<FlightSqlClient>>,
    /// Per-connection info/health state, aligned with `connections`.
    pub conn_info: Vec<ConnInfoState>,
    /// Connect dialog state; `Some` while the modal is open.
    pub connect_form: Option<ConnectForm>,
    /// Tabbed SQL editor workspace (shared across local + remote sources).
    pub sql: SqlWorkspace,
    /// Left object-explorer tree (files + catalogs/schemas/tables), now global.
    pub explorer: Explorer,

    pub copy_notice: Option<String>,

    /// Parquet-backed persistent state (history, column widths, recent files).
    pub store: StateStore,
    /// Persisted per-schema column widths, keyed by schema signature. Seeds the
    /// per-grid `col_widths` whenever a matching schema is loaded.
    pub column_widths: HashMap<String, Vec<f32>>,
    /// Persisted uniform grid row height (global preference, not per schema).
    /// Seeds each new editor tab's `row_height`.
    pub grid_row_height: f32,
    /// Recently opened data files, newest first.
    pub recent_files: Vec<RecentFile>,
    /// Names of every scalar/aggregate/window function the local DataFusion
    /// session knows, captured at boot for completion.
    pub function_names: Vec<String>,

    pub app_dir: PathBuf,

    /// OS light/dark preference detected once at boot. Used to resolve the
    /// `System` theme choice; `true` means the OS prefers dark.
    pub system_is_dark: bool,

    /// The main window's native handles, captured at boot, used to parent
    /// native file dialogs. `None` until the capture task completes.
    pub window_parent: Option<ParentWindow>,
}

/// Live ping + server metadata for one FlightSQL connection.
#[derive(Debug, Default, Clone)]
pub struct ConnInfoState {
    pub ping_ms: Option<u128>,
    pub ping_error: Option<String>,
    pub pinging: bool,
    pub info: Vec<(String, String)>,
    pub info_loading: bool,
    pub info_error: Option<String>,
    pub info_loaded: bool,
}

// -- SQL editor workspace -----------------------------------------------------

/// A tabbed SQL workspace: each editor tab is bound to its own `QueryEngine`
/// (the open Parquet file's session, or a FlightSQL connection). History is
/// shared across all tabs and kept in memory only.
#[derive(Debug)]
pub struct SqlWorkspace {
    pub editors: Vec<SqlEditorTab>,
    pub active: usize,
    pub next_id: u64,
    pub history: Vec<QueryHistoryEntry>,
    pub history_collapsed: bool,
    /// Whether the "+ New query" source picker is open.
    pub source_picker_open: bool,
    /// Height of the editor text area, shared by every tab and persisted as a
    /// UI preference. Dragged via the divider under the editor.
    pub editor_height: f32,
}

impl Default for SqlWorkspace {
    fn default() -> Self {
        SqlWorkspace {
            editors: Vec::new(),
            active: 0,
            next_id: 0,
            history: Vec::new(),
            history_collapsed: false,
            source_picker_open: false,
            editor_height: DEFAULT_EDITOR_HEIGHT,
        }
    }
}

/// Default / drag bounds for the SQL editor text area height.
pub(crate) const DEFAULT_EDITOR_HEIGHT: f32 = 180.0;
pub(crate) const MIN_EDITOR_HEIGHT: f32 = 80.0;
pub(crate) const MAX_EDITOR_HEIGHT: f32 = 1200.0;

/// Rows shown per page in a SQL editor's results grid (client-side paging over
/// the already-fetched, capped result batch).
pub(crate) const RESULT_PAGE_SIZE: usize = 100;

/// Rows per page in the Row Groups tab, for both the group table and an expanded group's
/// column chunks. Rendering every row of a file with thousands of groups or columns freezes the UI.
pub(crate) const ROW_GROUP_PAGE_SIZE: usize = 100;

pub struct SqlEditorTab {
    /// Stable id used to route async results (survives tab close/reorder).
    pub id: u64,
    pub engine: QueryEngine,
    pub title: String,
    pub content: text_editor::Content,
    pub running: bool,
    /// Abort handle for the in-flight query, if one is running. Set when a
    /// query Task is spawned; taken on cancel or cleared on completion.
    pub query_handle: Option<iced::task::Handle>,
    pub batch: Option<RecordBatch>,
    pub schema: Option<SchemaRef>,
    pub error: Option<String>,
    pub last_row_count: Option<i64>,
    pub last_elapsed: Option<TimeDelta>,
    pub truncated: bool,
    /// Open autocomplete popup, if any.
    pub completion: Option<CompletionState>,
    /// Live syntax diagnostics (recomputed as the user types).
    pub diagnostics: Vec<sql_ide::Diagnostic>,
    /// Expanded nested-cell detail for this editor's results grid, if open.
    pub cell_detail: Option<CellDetail>,
    /// Read-only reader buffer for an expanded scalar (text) cell, so the detail
    /// overlay can wrap + select long values. `None` for nested cells (tree view).
    pub cell_detail_editor: Option<text_editor::Content>,
    /// Per-column display widths for the results grid, indexed by column.
    /// Re-seeded on each completed query (schema may change).
    pub col_widths: Vec<f32>,
    /// Uniform data-row height for this tab's results grid; seeded from the
    /// persisted global preference.
    pub row_height: f32,
    /// Per-column statistics for the current result set, shown in the grid's
    /// stats row. Computed from the result batch on completion; empty for
    /// EXPLAIN results and errors.
    pub insights: Vec<ColumnInsight>,
    /// Current page (0-based) of the results grid; reset to 0 on each new query.
    pub page: usize,
    /// `Some` when the last completed query was an EXPLAIN, driving the
    /// formatted plan view. `None` for ordinary result sets.
    pub explain: Option<ExplainKind>,
    /// When an explain result is showing, render the raw `(plan_type, plan)`
    /// grid instead of the formatted plan tree.
    pub explain_raw: bool,
    /// Open export-settings modal for this tab, if any.
    pub export_dialog: Option<ExportDialogState>,
    /// Undo history: text snapshots taken before edit groups. iced's
    /// `text_editor` has no native undo, so we keep our own stack.
    pub undo_stack: Vec<EditSnapshot>,
    /// Redo history: snapshots popped by undo, restorable until the next edit.
    pub redo_stack: Vec<EditSnapshot>,
    /// Whether the current run of edits is already captured by the top undo
    /// snapshot (coalescing marker, so a burst of typing is one undo step).
    pub undo_group_open: bool,
    /// The kind of the last edit, used to break undo groups on insert↔delete
    /// transitions.
    pub last_edit_kind: Option<EditKind>,
    /// Last reported viewport of the editor's scroll area, used to keep the
    /// caret visible and to place the completion popup.
    pub editor_viewport: Option<EditorViewport>,
    /// The `(` / `)` pair adjacent to the caret, as 0-based `(line, byte col)`
    /// positions, for bracket-match highlighting.
    pub bracket_pair: Option<[(usize, usize); 2]>,
    /// Whether the completion popup may open on the next intellisense refresh.
    /// Armed by typing (or Ctrl+Space), disarmed by caret moves and clicks, so
    /// clicking to the end of a word does not pop the list.
    pub completion_armed: bool,
}

/// Scroll position and visible height of an editor's scroll area.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct EditorViewport {
    pub offset_y: f32,
    pub height: f32,
}

/// A point-in-time snapshot of an editor's text and caret, used for undo/redo.
#[derive(Debug, Clone)]
pub struct EditSnapshot {
    pub text: String,
    pub line: usize,
    pub column: usize,
}

/// Coarse classification of an edit, used to decide where one undo group ends
/// and the next begins.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum EditKind {
    Insert,
    Delete,
    /// Enter / Paste / Indent — each forms its own undo group.
    Other,
}

/// Maximum number of undo snapshots retained per editor tab.
const UNDO_CAP: usize = 200;

impl SqlEditorTab {
    /// Capture the current text and caret position as a snapshot.
    pub fn snapshot(&self) -> EditSnapshot {
        let pos = self.content.cursor().position;
        EditSnapshot {
            text: self.content.text(),
            line: pos.line,
            column: pos.column,
        }
    }

    /// Push the current state onto the undo stack as the start of a new group,
    /// clearing the redo stack. Caps the stack to [`UNDO_CAP`] entries.
    pub fn push_undo(&mut self) {
        self.undo_stack.push(self.snapshot());
        if self.undo_stack.len() > UNDO_CAP {
            self.undo_stack.remove(0);
        }
        self.redo_stack.clear();
    }

    /// Note that an edit of `kind` is about to happen, opening a fresh undo
    /// group when needed (first edit of a run, a kind change, or a standalone
    /// `Other` edit). Coalesces consecutive same-kind edits into one group.
    pub fn begin_edit_group(&mut self, kind: EditKind) {
        let new_group =
            !self.undo_group_open || self.last_edit_kind != Some(kind) || kind == EditKind::Other;
        if new_group {
            self.push_undo();
            self.undo_group_open = true;
        }
        self.last_edit_kind = Some(kind);
    }

    /// Break the current undo group so the next edit starts a new one (called
    /// after caret moves / clicks and after applying a completion).
    pub fn break_undo_group(&mut self) {
        self.undo_group_open = false;
        self.last_edit_kind = None;
    }

    /// Replace the whole document as one undo step, leaving the caret at
    /// `(line, byte column)` and refreshing diagnostics. Every wholesale text
    /// rewrite (history load, format, toggle comment, …) goes through here.
    pub fn set_text(&mut self, text: &str, line: usize, column: usize) {
        self.push_undo();
        self.break_undo_group();
        self.replace_text(text, line, column);
    }

    /// Swap the document without touching the undo stacks (undo/redo use this
    /// after they have moved their own snapshots around).
    pub fn replace_text(&mut self, text: &str, line: usize, column: usize) {
        self.content = text_editor::Content::with_text(text);
        restore_cursor(&mut self.content, line, column);
        self.completion = None;
        self.completion_armed = false;
        self.diagnostics = sql_ide::diagnostics(text);
    }

    /// Byte offset of the caret within `content.text()`.
    pub fn caret_byte_offset(&self) -> usize {
        let pos = self.content.cursor().position;
        let mut off = 0usize;
        for (i, line) in self.content.lines().enumerate() {
            if i == pos.line {
                return off + pos.column.min(line.text.len());
            }
            off += line.text.len() + line.ending.as_str().len();
        }
        off
    }

    /// Text of the caret's line.
    pub fn caret_line_text(&self) -> String {
        let pos = self.content.cursor().position;
        self.content
            .line(pos.line)
            .map(|l| l.text.into_owned())
            .unwrap_or_default()
    }

    /// Caret column in chars (iced reports a byte index).
    pub fn caret_char_column(&self) -> usize {
        let pos = self.content.cursor().position;
        let line = self.caret_line_text();
        line[..pos.column.min(line.len())].chars().count()
    }

    /// Inclusive range of lines covered by the selection (or the caret line).
    pub fn selected_line_range(&self) -> (usize, usize) {
        let cursor = self.content.cursor();
        let a = cursor.position.line;
        let b = cursor.selection.map_or(a, |s| s.line);
        (a.min(b), a.max(b))
    }
}

/// Move the caret to `(line, byte column)` from a known origin. iced's `Content`
/// exposes no line/column setter (`move_to` is pixel-based), so we navigate by
/// motions; `Right` steps one char, so the byte column is converted first.
pub(crate) fn restore_cursor(content: &mut text_editor::Content, line: usize, column: usize) {
    content.perform(Action::Move(Motion::DocumentStart));
    for _ in 0..line {
        content.perform(Action::Move(Motion::Down));
    }
    content.perform(Action::Move(Motion::Home));
    let chars = content
        .line(line)
        .map(|l| l.text[..column.min(l.text.len())].chars().count())
        .unwrap_or(0);
    for _ in 0..chars {
        content.perform(Action::Move(Motion::Right));
    }
}

/// State of the export-settings modal for one SQL editor tab.
#[derive(Debug, Clone)]
pub struct ExportDialogState {
    pub format: ExportFormat,
    pub opts_parquet: ParquetOptions,
    pub opts_csv: CsvOptions,
    pub opts_json: JsonOptions,
    pub in_progress: bool,
    pub error: Option<String>,
    pub parquet_column_name: String,
}
impl Default for ExportDialogState {
    fn default() -> Self {
        Self {
            format: ExportFormat::Parquet,
            opts_parquet: Default::default(),
            opts_csv: Default::default(),
            opts_json: Default::default(),
            in_progress: false,
            error: None,
            parquet_column_name: String::new(),
        }
    }
}

/// State of the open autocomplete popup for one editor.
#[derive(Debug, Clone)]
pub struct CompletionState {
    pub items: Vec<sql_ide::Completion>,
    pub selected: usize,
}

impl std::fmt::Debug for SqlEditorTab {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("SqlEditorTab")
            .field("id", &self.id)
            .field("title", &self.title)
            .field("running", &self.running)
            .finish_non_exhaustive()
    }
}

/// Identifies a source to bind a new editor tab to.
#[derive(Debug, Clone, Copy)]
pub enum SourceRef {
    /// The open Parquet file with this id.
    File(FileId),
    /// The FlightSQL connection at this index in `App::connections`.
    Flight(usize),
}

// -- Query history (in-memory) ------------------------------------------------

#[derive(Debug, Clone)]
pub enum HistoryStatus {
    Ok,
    Err(String),
}

#[derive(Debug, Clone)]
pub struct QueryHistoryEntry {
    pub sql: String,
    pub source_label: String,
    pub status: HistoryStatus,
    pub row_count: Option<i64>,
    pub elapsed: TimeDelta,
    pub ran_at: SystemTime,
}

/// Now that history is persisted across restarts (see [`crate::store`]), keep a
/// larger window than the old in-memory-only cap.
pub(crate) const HISTORY_CAP: usize = 1000;

/// Most recent files to remember for the welcome-screen quick-reopen list.
pub(crate) const RECENT_FILES_CAP: usize = 20;

// -- FlightSQL connect form ---------------------------------------------------

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum AuthKind {
    #[default]
    None,
    Bearer,
    Basic,
}

#[derive(Debug, Clone, Default)]
pub struct ConnectForm {
    pub url: String,
    pub auth_kind: AuthKind,
    pub token: String,
    pub user: String,
    pub pass: String,
    pub connecting: bool,
    pub error: Option<String>,
}

#[derive(Debug, Clone)]
#[allow(dead_code)]
pub struct CellDetail {
    pub row: usize,
    pub col: usize,
    pub column_name: String,
    pub type_label: String,
    pub node: crate::format::NestedNode,
}
