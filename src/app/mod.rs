use std::path::PathBuf;
use std::sync::Arc;
use std::time::{Instant, SystemTime};

use arrow::record_batch::RecordBatch;
use iced::Task;
use iced::widget::text_editor;
use iced::widget::text_editor::{Action, Edit};
mod helpers;
pub use helpers::*;

mod view;
use crate::config::Config;
use crate::config::ThemeChoice;
use crate::engine::QueryEngine;
use crate::explain::ExplainKind;
use crate::explorer::{Explorer, ExplorerLoad, ExplorerTarget, LoadRequest};
use crate::flightsql::{FlightAuth, FlightSqlClient, FlightSqlConfig};
use crate::parquet_io::load_metadata;
use crate::store::{RecentFile, StateFile, StateStore};
use crate::widgets::MIN_COL_WIDTH;
use crate::wrangle::SharedSession;
use crate::wrangle::naming::derive_table_name;
use crate::{SubCommand, theme};
use theme::palette::PaletteKind;

mod state;
pub use state::*;

mod message;
pub use message::*;

mod update;

impl App {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn boot(app_dir: PathBuf, command: Option<SubCommand>) -> (Self, Task<Message>) {
        let config = Config::load(&app_dir);
        let local = SharedSession::new(&config.session, &config.runtime);
        let store = StateStore::new(&app_dir);
        let history = store.load_history();
        let column_widths = store.load_column_widths();
        let recent_files = store.load_recent_files();
        let mut app = App {
            local,
            config,
            store,
            column_widths,
            recent_files,
            app_dir,
            system_is_dark: crate::config::detect_system_dark(),
            ..Self::new()
        };
        app.sql.history = history;
        // Capture the main window's native handles so file dialogs can be
        // parented to it. Runs once the window exists.
        let capture_window = iced::window::latest().then(|maybe_id| match maybe_id {
            Some(id) => iced::window::run(id, ParentWindow::from_window)
                .map(|h| FileMessage::WindowReady(h).into()),
            None => Task::none(),
        });
        let register = app.register_state_tables_task();
        let initial = match command {
            Some(SubCommand::File { path }) => {
                Task::done(FileMessage::FilePicked(Some(path)).into())
            }
            Some(SubCommand::FlightSql { endpoint }) => {
                app.connect_form = Some(ConnectForm {
                    url: endpoint.clone(),
                    ..ConnectForm::default()
                });
                Task::done(FlightMessage::ConnectSubmit.into())
            }
            None => Task::none(),
        };
        (app, Task::batch([register, initial, capture_window]))
    }

    /// (Re)register the persistent-state Parquet files as tables in the local
    /// session, so the user can `SELECT * FROM history` etc. Best-effort: only
    /// files that exist are registered, and failures are ignored. Run at boot
    /// and after each history write so the `history` table stays fresh.
    fn register_state_tables_task(&self) -> Task<Message> {
        let local = self.local.clone();
        let store = self.store.clone();
        Task::perform(
            async move {
                for sf in StateFile::ALL {
                    if let Some(path) = store.file_path(sf)
                        && let Some(p) = path.to_str()
                    {
                        let _ = local
                            .clone()
                            .register_table_replace(sf.table_name(), p.to_string())
                            .await;
                    }
                }
            },
            |()| Message::Ignore,
        )
    }

    /// Whole-UI zoom, read by Iced's `scale_factor` each frame.
    pub fn scale_factor(&self) -> f32 {
        self.config.appearance.ui_scale
    }

    pub fn title(&self) -> String {
        match self.active_file() {
            Some(f) => format!(
                "DataFusion UI — {}",
                f.summary
                    .path
                    .file_name()
                    .and_then(|s| s.to_str())
                    .unwrap_or("?")
            ),
            None => "DataFusion UI".to_string(),
        }
    }
    pub fn theme_for(app: &App) -> iced::Theme {
        // Preview the unsaved draft theme while the settings page is open, so a
        // Cancel (which drops the draft) reverts to the saved theme for free.
        let choice = app
            .settings_draft
            .as_ref()
            .unwrap_or(&app.config)
            .appearance
            .theme;
        let kind = match choice {
            ThemeChoice::Instrument => PaletteKind::Dark,
            ThemeChoice::Light => PaletteKind::Light,
            ThemeChoice::OneDark => PaletteKind::OneDark,
            ThemeChoice::System => {
                if app.system_is_dark {
                    PaletteKind::Dark
                } else {
                    PaletteKind::Light
                }
            }
        };
        // Keep the active palette (read by all style helpers) in lockstep with
        // the iced theme we return.
        theme::palette::set_active(kind);
        match kind {
            PaletteKind::Light => theme::light_theme(),
            PaletteKind::OneDark => theme::one_dark_theme(),
            PaletteKind::Dark => theme::dark_theme(),
        }
    }
    // -- File lookup / selection helpers --

    fn file(&self, id: FileId) -> Option<&FileTab> {
        self.files.iter().find(|f| f.id == id)
    }

    fn file_mut(&mut self, id: FileId) -> Option<&mut FileTab> {
        self.files.iter_mut().find(|f| f.id == id)
    }

    /// The file the current selection points at, if any.
    fn selected_file_id(&self) -> Option<FileId> {
        match self.selection {
            Selection::File { id, .. } => Some(id),
            _ => None,
        }
    }

    pub fn active_file(&self) -> Option<&FileTab> {
        self.file(self.selected_file_id()?)
    }

    fn active_file_mut(&mut self) -> Option<&mut FileTab> {
        let id = self.selected_file_id()?;
        self.file_mut(id)
    }

    /// A sensible selection when the current one becomes invalid (e.g. its file
    /// was closed): first open file's Overview, else SQL if a source exists,
    /// else Welcome.
    fn default_selection(&self) -> Selection {
        if let Some(f) = self.files.first() {
            Selection::File {
                id: f.id,
                view: FileView::Overview,
            }
        } else if self.has_sql_source() {
            Selection::Sql
        } else {
            Selection::Welcome
        }
    }

    /// Reset `selection` to a valid target if it now points at something gone.
    fn ensure_valid_selection(&mut self) {
        let valid = match self.selection {
            Selection::Welcome => self.files.is_empty() && !self.has_sql_source(),
            Selection::File { id, .. } => self.file(id).is_some(),
            Selection::Sql => self.has_sql_source(),
            Selection::ConnInfo { conn } => conn < self.connections.len(),
            // The settings page is always reachable.
            Selection::Settings => true,
        };
        if !valid {
            self.selection = self.default_selection();
        }
    }

    pub fn update(&mut self, message: Message) -> Task<Message> {
        match message {
            Message::File(m) => self.update_file(m),
            Message::Sql(m) => self.update_sql(m),
            Message::Grid(m) => self.update_grid(m),
            Message::Flight(m) => self.update_flight(m),
            Message::ConnInfo(m) => self.update_conn_info(m),
            Message::Explorer(m) => self.update_explorer(m),
            Message::Settings(m) => self.update_settings(m),
            Message::Ignore => Task::none(),
        }
    }

    /// Rebuild the shared session from current settings and re-register every
    /// open file (returns a batched task that reloads their schemas).
    fn rebuild_local_session(&mut self) -> Task<Message> {
        self.local = SharedSession::new(&self.config.session, &self.config.runtime);
        let mut tasks = Vec::new();
        for ft in &mut self.files {
            ft.registered = false;
            ft.register_error = None;
            let id = ft.id;
            let name = ft.table_name.clone();
            if let Some(path_str) = ft.summary.path.to_str().map(str::to_string) {
                let shared = self.local.clone();
                tasks.push(Task::perform(
                    shared.register_file(name, path_str),
                    move |result| FileMessage::Registered { file: id, result }.into(),
                ));
            }
        }
        Task::batch(tasks)
    }

    /// Spawn the FlightSQL metadata RPC a [`LoadRequest`] asks for, routing the
    /// result back as an `ExplorerLoaded` message addressed to the right node.
    fn spawn_explorer_load(&self, req: LoadRequest) -> Task<Message> {
        match req {
            LoadRequest::Catalogs { conn } => {
                let Some(client) = self.connections.get(conn).cloned() else {
                    return Task::none();
                };
                Task::perform(client.list_catalogs(), move |r| {
                    Message::from(ExplorerMessage::Loaded {
                        target: ExplorerTarget::FlightRoot { conn },
                        load: ExplorerLoad::Catalogs(r),
                    })
                })
            }
            LoadRequest::Schemas { conn, catalog } => {
                let Some(client) = self.connections.get(conn).cloned() else {
                    return Task::none();
                };
                let target = ExplorerTarget::Catalog {
                    conn,
                    catalog: catalog.clone(),
                };
                Task::perform(client.list_schemas(Some(catalog)), move |r| {
                    ExplorerMessage::Loaded {
                        target: target.clone(),
                        load: ExplorerLoad::Schemas(r),
                    }
                    .into()
                })
            }
            LoadRequest::Tables {
                conn,
                catalog,
                schema,
            } => {
                let Some(client) = self.connections.get(conn).cloned() else {
                    return Task::none();
                };
                let target = ExplorerTarget::Schema {
                    conn,
                    catalog: catalog.clone(),
                    schema: schema.clone(),
                };
                Task::perform(client.list_tables(Some(catalog), schema), move |r| {
                    ExplorerMessage::Loaded {
                        target: target.clone(),
                        load: ExplorerLoad::Tables(r),
                    }
                    .into()
                })
            }
            LoadRequest::Columns {
                conn,
                qualified,
                target,
            } => {
                let Some(client) = self.connections.get(conn).cloned() else {
                    return Task::none();
                };
                Task::perform(client.table_columns(qualified), move |r| {
                    ExplorerMessage::Loaded {
                        target: target.clone(),
                        load: ExplorerLoad::Columns(r),
                    }
                    .into()
                })
            }
        }
    }

    /// Push a new editor tab bound to `engine`, seeded with `starter`, and make
    /// it active. Returns the new tab's id. Does not change the selection.
    fn push_editor(&mut self, engine: QueryEngine, starter: String) -> u64 {
        let id = self.sql.next_id;
        self.sql.next_id += 1;
        let title = engine.source_label();
        self.sql.editors.push(SqlEditorTab {
            id,
            engine,
            title,
            content: text_editor::Content::with_text(&starter),
            running: false,
            batch: None,
            schema: None,
            error: None,
            last_row_count: None,
            last_elapsed_ms: None,
            truncated: false,
            completion: None,
            diagnostics: Vec::new(),
            cell_detail: None,
            col_widths: Vec::new(),
            insights: Vec::new(),
            page: 0,
            explain: None,
            explain_raw: false,
            export_dialog: None,
            undo_stack: Vec::new(),
            redo_stack: Vec::new(),
            undo_group_open: false,
            last_edit_kind: None,
        });
        self.sql.active = self.sql.editors.len() - 1;
        id
    }

    fn run_editor(&mut self, id: u64) -> Task<Message> {
        let sql = match self.sql.editors.iter().find(|t| t.id == id) {
            Some(t) => t.content.text(),
            None => return Task::none(),
        };
        self.run_sql_text(id, sql)
    }

    /// Run `sql` against the editor's bound engine without altering its content
    /// (used by the EXPLAIN buttons, which wrap the query in place).
    fn run_sql_text(&mut self, id: u64, sql: String) -> Task<Message> {
        let cap = self.config.result_row_cap;
        let Some(t) = self.sql.editors.iter_mut().find(|t| t.id == id) else {
            return Task::none();
        };
        if sql.trim().is_empty() {
            return Task::none();
        }
        let engine = t.engine.clone();
        let source_label = engine.source_label();
        t.running = true;
        t.error = None;
        let started = Instant::now();
        Task::perform(
            async move {
                let result = engine.run_query(sql.clone(), cap).await;
                (sql, source_label, started.elapsed().as_millis(), result)
            },
            move |(sql, source_label, elapsed_ms, result)| {
                SqlMessage::Completed {
                    id,
                    sql,
                    source_label,
                    elapsed_ms,
                    result,
                }
                .into()
            },
        )
    }

    /// Build the completion [`sql_ide::Catalog`] for a tab's bound engine.
    fn catalog_for_engine(&self, engine: &QueryEngine) -> sql_ide::Catalog {
        match engine {
            QueryEngine::Local(_) => Explorer::local_catalog(
                self.files
                    .iter()
                    .map(|ft| (ft.table_name.as_str(), &ft.summary.schema)),
            ),
            QueryEngine::Flight(client) => self
                .connections
                .iter()
                .position(|c| Arc::ptr_eq(c, client))
                .map(|i| self.explorer.flight_catalog(i))
                .unwrap_or_default(),
        }
    }

    /// Recompute completions and diagnostics for one editor after an edit.
    /// The popup only opens when the cursor sits at the end of a word or just
    /// after a `.`, so it does not pop up on every keystroke (e.g. spaces).
    fn refresh_intellisense(&mut self, id: u64) {
        let Some(idx) = self.sql.editors.iter().position(|t| t.id == id) else {
            return;
        };
        let engine = self.sql.editors[idx].engine.clone();
        let catalog = self.catalog_for_engine(&engine);
        let tab = &mut self.sql.editors[idx];
        let text = tab.content.text();
        let cursor = tab.content.cursor();
        // sql_ide uses 1-based positions; the editor cursor is 0-based.
        let line = cursor.position.line as u64 + 1;
        let col = cursor.position.column as u64 + 1;

        tab.diagnostics = sql_ide::diagnostics(&text);

        if should_complete(&text, cursor.position.line, cursor.position.column) {
            let items = sql_ide::complete(&text, line, col, &catalog);
            tab.completion = if items.is_empty() {
                None
            } else {
                let selected = tab
                    .completion
                    .as_ref()
                    .map(|c| c.selected.min(items.len() - 1))
                    .unwrap_or(0);
                Some(CompletionState { items, selected })
            };
        } else {
            tab.completion = None;
        }
    }

    /// Apply a completion: delete the typed prefix and insert the candidate.
    fn accept_completion(&mut self, id: u64, index: usize) {
        let Some(tab) = self.sql.editors.iter_mut().find(|t| t.id == id) else {
            return;
        };
        let Some(item) = tab
            .completion
            .as_ref()
            .and_then(|c| c.items.get(index).cloned())
        else {
            return;
        };
        // Capture the pre-completion text so the whole insertion (the backspaces
        // plus the paste) undoes as a single step.
        tab.push_undo();
        tab.break_undo_group();
        for _ in 0..item.replace_len {
            tab.content.perform(Action::Edit(Edit::Backspace));
        }
        tab.content
            .perform(Action::Edit(Edit::Paste(Arc::new(item.insert_text))));
        tab.completion = None;
        let text = tab.content.text();
        tab.diagnostics = sql_ide::diagnostics(&text);
    }

    fn push_history(&mut self, entry: QueryHistoryEntry) {
        self.sql.history.push(entry);
        if self.sql.history.len() > HISTORY_CAP {
            let overflow = self.sql.history.len() - HISTORY_CAP;
            self.sql.history.drain(0..overflow);
        }
        // Best-effort persist (small file; mirrors Config::save being inline).
        self.store.save_history(&self.sql.history);
    }

    /// Record a freshly opened file at the front of the recent list (de-duped
    /// by path), cap it, and persist.
    fn record_recent_file(&mut self, path: &std::path::Path) {
        let Some(path_str) = path.to_str().map(str::to_string) else {
            return;
        };
        self.recent_files.retain(|r| r.path != path_str);
        let now_ms = SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map(|d| d.as_millis() as i64)
            .unwrap_or(0);
        self.recent_files.insert(
            0,
            RecentFile {
                path: path_str,
                last_opened_ms: now_ms,
            },
        );
        self.recent_files.truncate(RECENT_FILES_CAP);
        self.store.save_recent_files(&self.recent_files);
    }

    // -- Resizable columns ----------------------------------------------------

    /// Mutable per-column widths for a SQL editor's results grid.
    fn col_widths_mut(&mut self, id: u64) -> Option<&mut Vec<f32>> {
        self.sql
            .editors
            .iter_mut()
            .find(|t| t.id == id)
            .map(|t| &mut t.col_widths)
    }

    /// The currently displayed batch for a SQL editor's grid (for autofit).
    fn grid_batch(&self, id: u64) -> Option<&RecordBatch> {
        self.sql
            .editors
            .iter()
            .find(|t| t.id == id)
            .and_then(|t| t.batch.as_ref())
    }

    /// Per-column widths to use for a freshly loaded schema: the persisted set
    /// if it matches the column count, else defaults.
    fn seed_widths(&self, schema: &arrow::datatypes::Schema) -> Vec<f32> {
        let n = schema.fields().len();
        let sig = schema_signature(schema);
        match self.column_widths.get(&sig) {
            Some(w) if w.len() == n => w.clone(),
            _ => vec![crate::views::data::CELL_WIDTH; n],
        }
    }

    /// Persist a SQL editor grid's current widths under its schema signature.
    fn persist_col_widths(&mut self, id: u64) {
        let entry = self.sql.editors.iter().find(|t| t.id == id).and_then(|t| {
            t.schema
                .as_ref()
                .map(|s| (schema_signature(s.as_ref()), t.col_widths.clone()))
        });
        if let Some((sig, widths)) = entry {
            self.column_widths.insert(sig, widths);
            self.store.save_column_widths(&self.column_widths);
        }
    }

    /// Mutable export-dialog state for a SQL tab, if open.
    fn export_dialog_mut(&mut self, id: u64) -> Option<&mut ExportDialogState> {
        self.sql
            .editors
            .iter_mut()
            .find(|t| t.id == id)
            .and_then(|t| t.export_dialog.as_mut())
    }

    /// Whether the SQL workspace has at least one source to run against.
    pub fn has_sql_source(&self) -> bool {
        self.files.iter().any(|f| f.registered) || !self.connections.is_empty()
    }
}
