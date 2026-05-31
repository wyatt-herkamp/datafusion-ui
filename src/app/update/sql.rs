use iced::Task;
use iced::widget::text_editor::{Action, Edit, Motion};

use super::super::*;

impl App {
    pub(crate) fn update_sql(&mut self, m: SqlMessage) -> Task<Message> {
        match m {
            SqlMessage::ShowCellDetail { id, row, col } => {
                if let Some(t) = self.sql.editors.iter_mut().find(|t| t.id == id)
                    && let Some(batch) = t.batch.as_ref()
                    && col < batch.num_columns()
                    && row < batch.num_rows()
                {
                    let opts = crate::format::default_options();
                    let array = batch.column(col);
                    let node = crate::format::cell_node(array.as_ref(), row, &opts);
                    let schema = batch.schema();
                    let field = schema.field(col);
                    t.cell_detail = Some(CellDetail {
                        row,
                        col,
                        column_name: field.name().clone(),
                        type_label: crate::format::type_label(field.data_type()),
                        node,
                    });
                }
                Task::none()
            }
            SqlMessage::CloseCellDetail { id } => {
                if let Some(t) = self.sql.editors.iter_mut().find(|t| t.id == id) {
                    t.cell_detail = None;
                }
                Task::none()
            }
            SqlMessage::SetResultPage { id, page } => {
                if let Some(t) = self.sql.editors.iter_mut().find(|t| t.id == id) {
                    let rows = t.batch.as_ref().map_or(0, |b| b.num_rows());
                    let pages = rows.div_ceil(RESULT_PAGE_SIZE).max(1);
                    t.page = page.min(pages - 1);
                }
                Task::none()
            }
            SqlMessage::NewQueryToggle => {
                self.sql.source_picker_open = !self.sql.source_picker_open;
                Task::none()
            }
            SqlMessage::NewQueryForSource(src) => {
                self.sql.source_picker_open = false;
                self.selection = Selection::Sql;
                match src {
                    SourceRef::File(id) => {
                        let starter = self
                            .file(id)
                            .map(|ft| format!("SELECT * FROM {} LIMIT 100", ft.table_name))
                            .unwrap_or_else(|| "SELECT 1".to_string());
                        let local = self.local.clone();
                        self.push_editor(QueryEngine::Local(local), starter);
                    }
                    SourceRef::Flight(i) => {
                        if let Some(c) = self.connections.get(i).cloned() {
                            self.push_editor(QueryEngine::Flight(c), "SELECT 1".to_string());
                        }
                    }
                }
                Task::none()
            }
            SqlMessage::EditorSelect(id) => {
                if let Some(i) = self.sql.editors.iter().position(|t| t.id == id) {
                    self.sql.active = i;
                }
                Task::none()
            }
            SqlMessage::EditorClose(id) => {
                if let Some(i) = self.sql.editors.iter().position(|t| t.id == id) {
                    self.sql.editors.remove(i);
                    if self.sql.active >= self.sql.editors.len() {
                        self.sql.active = self.sql.editors.len().saturating_sub(1);
                    }
                }
                Task::none()
            }
            SqlMessage::EditorAction(id, action) => {
                if let Some(t) = self.sql.editors.iter_mut().find(|t| t.id == id) {
                    // Snapshot before edits so Ctrl+Z can revert them; caret
                    // moves / clicks just break the current undo group.
                    match edit_kind(&action) {
                        Some(kind) => t.begin_edit_group(kind),
                        None => t.break_undo_group(),
                    }
                    t.content.perform(action);
                }
                self.refresh_intellisense(id);
                Task::none()
            }
            SqlMessage::Undo(id) => {
                if let Some(t) = self.sql.editors.iter_mut().find(|t| t.id == id)
                    && let Some(prev) = t.undo_stack.pop()
                {
                    t.redo_stack.push(t.snapshot());
                    t.content = text_editor::Content::with_text(&prev.text);
                    restore_cursor(&mut t.content, prev.line, prev.column);
                    t.completion = None;
                    t.undo_group_open = false;
                    t.last_edit_kind = None;
                    t.diagnostics = sql_ide::diagnostics(&prev.text);
                }
                Task::none()
            }
            SqlMessage::Redo(id) => {
                if let Some(t) = self.sql.editors.iter_mut().find(|t| t.id == id)
                    && let Some(next) = t.redo_stack.pop()
                {
                    t.undo_stack.push(t.snapshot());
                    t.content = text_editor::Content::with_text(&next.text);
                    restore_cursor(&mut t.content, next.line, next.column);
                    t.completion = None;
                    t.undo_group_open = false;
                    t.last_edit_kind = None;
                    t.diagnostics = sql_ide::diagnostics(&next.text);
                }
                Task::none()
            }
            SqlMessage::CompletionMove(id, delta) => {
                if let Some(t) = self.sql.editors.iter_mut().find(|t| t.id == id)
                    && let Some(c) = &mut t.completion
                {
                    let n = c.items.len() as i32;
                    if n > 0 {
                        c.selected = (((c.selected as i32 + delta) % n + n) % n) as usize;
                    }
                }
                Task::none()
            }
            SqlMessage::CompletionAccept(id, index) => {
                self.accept_completion(id, index);
                Task::none()
            }
            SqlMessage::CompletionAcceptSelected(id) => {
                let index = self
                    .sql
                    .editors
                    .iter()
                    .find(|t| t.id == id)
                    .and_then(|t| t.completion.as_ref())
                    .map(|c| c.selected);
                if let Some(index) = index {
                    self.accept_completion(id, index);
                }
                Task::none()
            }
            SqlMessage::CompletionDismiss(id) => {
                if let Some(t) = self.sql.editors.iter_mut().find(|t| t.id == id) {
                    t.completion = None;
                }
                Task::none()
            }
            SqlMessage::Run(id) => self.run_editor(id),
            SqlMessage::Explain(id) | SqlMessage::ExplainAnalyze(id) => {
                let kind = if matches!(m, SqlMessage::ExplainAnalyze(_)) {
                    ExplainKind::Analyze
                } else {
                    ExplainKind::Plan
                };
                let Some(t) = self.sql.editors.iter().find(|t| t.id == id) else {
                    return Task::none();
                };
                let inner = t.content.text();
                let sql = format!("{}{}", kind.prefix(), crate::explain::strip_prefix(&inner));
                self.run_sql_text(id, sql)
            }
            SqlMessage::ExplainToggleRaw(id) => {
                if let Some(t) = self.sql.editors.iter_mut().find(|t| t.id == id) {
                    t.explain_raw = !t.explain_raw;
                }
                Task::none()
            }
            SqlMessage::ExportOpen(id) => {
                if let Some(t) = self.sql.editors.iter_mut().find(|t| t.id == id) {
                    t.export_dialog = Some(ExportDialogState::default());
                }
                Task::none()
            }
            SqlMessage::ExportCancel(id) => {
                if let Some(t) = self.sql.editors.iter_mut().find(|t| t.id == id) {
                    t.export_dialog = None;
                }
                Task::none()
            }
            SqlMessage::ExportSetFormat(id, fmt) => {
                if let Some(d) = self.export_dialog_mut(id) {
                    d.options.format = fmt;
                }
                Task::none()
            }
            SqlMessage::ExportSetCompression(id, c) => {
                if let Some(d) = self.export_dialog_mut(id) {
                    d.options.parquet_compression = c;
                }
                Task::none()
            }
            SqlMessage::ExportToggleHeader(id) => {
                if let Some(d) = self.export_dialog_mut(id) {
                    d.options.csv_header = !d.options.csv_header;
                }
                Task::none()
            }
            SqlMessage::ExportToggleNdjson(id) => {
                if let Some(d) = self.export_dialog_mut(id) {
                    d.options.json_ndjson = !d.options.json_ndjson;
                }
                Task::none()
            }
            SqlMessage::ExportDelimiter(id, s) => {
                if let Some(d) = self.export_dialog_mut(id)
                    && let Some(b) = s.bytes().next()
                {
                    d.options.csv_delimiter = b;
                }
                Task::none()
            }
            SqlMessage::ExportConfirm(id) => {
                let Some(t) = self.sql.editors.iter().find(|t| t.id == id) else {
                    return Task::none();
                };
                let Some(dialog) = t.export_dialog.as_ref() else {
                    return Task::none();
                };
                let fmt = dialog.options.format;
                let ext = fmt.extension();
                let default_name = format!("export.{ext}");
                let parent = self.window_parent.clone();
                Task::perform(
                    async move {
                        let dialog = rfd::AsyncFileDialog::new()
                            .add_filter(fmt.label(), &[ext])
                            .set_file_name(default_name)
                            .set_title("Save Exported File");
                        let dialog = match &parent {
                            Some(p) => dialog.set_parent(p),
                            None => dialog,
                        };
                        dialog.save_file().await.map(|h| h.path().to_path_buf())
                    },
                    move |path| SqlMessage::ExportPathPicked { id, path }.into(),
                )
            }
            SqlMessage::ExportPathPicked { path: None, .. } => Task::none(),
            SqlMessage::ExportPathPicked {
                id,
                path: Some(path),
            } => {
                let Some(t) = self.sql.editors.iter_mut().find(|t| t.id == id) else {
                    return Task::none();
                };
                let Some(dialog) = t.export_dialog.as_mut() else {
                    return Task::none();
                };
                dialog.in_progress = true;
                dialog.error = None;
                let options = dialog.options;
                let engine = t.engine.clone();
                let sql = t.content.text();
                Task::perform(
                    async move {
                        let stream = engine.export_stream(sql).await?;
                        Ok(crate::export::write_stream(stream, path, options).await?)
                    },
                    move |result| SqlMessage::ExportCompleted { id, result }.into(),
                )
            }
            SqlMessage::ExportCompleted { id, result } => match result {
                Ok(path) => {
                    if let Some(t) = self.sql.editors.iter_mut().find(|t| t.id == id) {
                        t.export_dialog = None;
                    }
                    self.copy_notice = Some(format!("Exported to {}", path.display()));
                    Task::perform(
                        async {
                            tokio::time::sleep(std::time::Duration::from_millis(2500)).await;
                        },
                        |_| FileMessage::ClearCopyNotice.into(),
                    )
                }
                Err(e) => {
                    if let Some(t) = self.sql.editors.iter_mut().find(|t| t.id == id)
                        && let Some(d) = t.export_dialog.as_mut()
                    {
                        d.in_progress = false;
                        d.error = Some(e.to_string());
                    }
                    Task::none()
                }
            },
            SqlMessage::Completed {
                id,
                sql,
                source_label,
                elapsed_ms,
                result,
            } => {
                let (status, row_count) = match &result {
                    Ok(r) => (HistoryStatus::Ok, Some(r.row_count)),
                    Err(e) => (HistoryStatus::Err(e.to_string()), None),
                };
                // Seed per-column widths from persisted prefs before borrowing
                // the tab mutably (schema may have changed since the last run).
                let seeded = match &result {
                    Ok(r) => Some(self.seed_widths(r.schema.as_ref())),
                    Err(_) => None,
                };
                let explain_kind = crate::explain::detect(&sql);
                if let Some(t) = self.sql.editors.iter_mut().find(|t| t.id == id) {
                    t.running = false;
                    t.last_elapsed_ms = Some(elapsed_ms);
                    t.explain = explain_kind;
                    match result {
                        Ok(r) => {
                            // Column stats for the grid's stats row — only for
                            // ordinary result sets, not EXPLAIN output.
                            t.insights = if explain_kind.is_none() {
                                crate::wrangle::insights::compute_from_batch(&r.batch)
                            } else {
                                Vec::new()
                            };
                            t.batch = Some(r.batch);
                            t.schema = Some(r.schema);
                            t.last_row_count = Some(r.row_count);
                            t.truncated = r.truncated;
                            t.error = None;
                            t.page = 0;
                            if let Some(w) = seeded {
                                t.col_widths = w;
                            }
                        }
                        Err(e) => {
                            t.error = Some(e.to_string());
                            t.batch = None;
                            t.truncated = false;
                            t.insights = Vec::new();
                        }
                    }
                }
                self.push_history(QueryHistoryEntry {
                    sql,
                    source_label,
                    status,
                    row_count,
                    elapsed_ms,
                    ran_at: SystemTime::now(),
                });
                // Refresh the queryable `history` table to include this run.
                self.register_state_tables_task()
            }
            SqlMessage::HistoryLoad(i) => {
                if let Some(entry) = self.sql.history.get(i) {
                    let sql = entry.sql.clone();
                    if let Some(t) = self.sql.editors.get_mut(self.sql.active) {
                        t.content = text_editor::Content::with_text(&sql);
                        t.reset_undo();
                    }
                }
                Task::none()
            }
            SqlMessage::HistoryRerun(i) => {
                if let Some(entry) = self.sql.history.get(i) {
                    let sql = entry.sql.clone();
                    if let Some(t) = self.sql.editors.get_mut(self.sql.active) {
                        t.content = text_editor::Content::with_text(&sql);
                        t.reset_undo();
                        let id = t.id;
                        return self.run_editor(id);
                    }
                }
                Task::none()
            }
            SqlMessage::HistoryToggle => {
                self.sql.history_collapsed = !self.sql.history_collapsed;
                Task::none()
            }
        }
    }
}

/// Classify a `text_editor` action for undo grouping. Returns `None` for
/// non-editing actions (caret moves, clicks, selection, scroll).
fn edit_kind(action: &text_editor::Action) -> Option<EditKind> {
    match action {
        Action::Edit(Edit::Insert(_)) => Some(EditKind::Insert),
        Action::Edit(Edit::Backspace) | Action::Edit(Edit::Delete) => Some(EditKind::Delete),
        Action::Edit(_) => Some(EditKind::Other),
        _ => None,
    }
}

/// Move the caret to `(line, column)` from a known origin. iced's `Content`
/// exposes no line/column setter (`move_to` is pixel-based), so we navigate by
/// motions. Exact because undo restores the identical text.
fn restore_cursor(content: &mut text_editor::Content, line: usize, column: usize) {
    content.perform(Action::Move(Motion::DocumentStart));
    for _ in 0..line {
        content.perform(Action::Move(Motion::Down));
    }
    content.perform(Action::Move(Motion::Home));
    for _ in 0..column {
        content.perform(Action::Move(Motion::Right));
    }
}
