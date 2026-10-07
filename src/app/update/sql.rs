use std::sync::Arc;

use iced::Task;
use iced::widget::text_editor::{Action, Edit};

use crate::export::{ExportFormat, ExportOptions};

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
                    // Scalar cells open a read-only, wrapping reader; nested
                    // cells (List/Struct/Map) keep the tree view.
                    t.cell_detail_editor = match &node {
                        crate::format::NestedNode::Leaf(s) => {
                            Some(text_editor::Content::with_text(s))
                        }
                        crate::format::NestedNode::Null => {
                            Some(text_editor::Content::with_text(""))
                        }
                        _ => None,
                    };
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
                    t.cell_detail_editor = None;
                }
                Task::none()
            }
            SqlMessage::CellDetailEditorAction { id, action } => {
                // Read-only: apply only non-edit actions (drag-select, caret
                // moves, scroll) so text stays selectable/copyable but immutable.
                if let Some(t) = self.sql.editors.iter_mut().find(|t| t.id == id)
                    && let Some(ed) = t.cell_detail_editor.as_mut()
                    && !action.is_edit()
                {
                    ed.perform(action);
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
                let Some(t) = self.sql.editors.iter_mut().find(|t| t.id == id) else {
                    return Task::none();
                };
                // Snapshot before edits so Ctrl+Z can revert them; caret
                // moves / clicks just break the current undo group.
                let kind = edit_kind(&action);
                match kind {
                    Some(kind) => t.begin_edit_group(kind),
                    None => t.break_undo_group(),
                }
                // Only typing arms the popup; moving or clicking disarms it.
                t.completion_armed = matches!(
                    action,
                    Action::Edit(Edit::Insert(_)) | Action::Edit(Edit::Backspace)
                );
                let keeps_caret_in_view = matches!(action, Action::Scroll { .. });
                match action {
                    Action::Edit(Edit::Insert(c))
                        if auto_close_pair(c).is_some() || is_closer(c) =>
                    {
                        apply_auto_close(t, c);
                    }
                    Action::Edit(Edit::Backspace) if between_auto_pair(t) => {
                        // Backspace between `()` / `''` removes both halves.
                        t.content.perform(Action::Edit(Edit::Delete));
                        t.content.perform(Action::Edit(Edit::Backspace));
                    }
                    other => t.content.perform(other),
                }
                self.refresh_intellisense(id);
                if keeps_caret_in_view {
                    Task::none()
                } else {
                    self.scroll_editor_to_caret(id)
                }
            }
            SqlMessage::Undo(id) => {
                if let Some(t) = self.sql.editors.iter_mut().find(|t| t.id == id)
                    && let Some(prev) = t.undo_stack.pop()
                {
                    t.redo_stack.push(t.snapshot());
                    t.replace_text(&prev.text, prev.line, prev.column);
                    t.undo_group_open = false;
                    t.last_edit_kind = None;
                }
                self.refresh_intellisense(id);
                self.scroll_editor_to_caret(id)
            }
            SqlMessage::Redo(id) => {
                if let Some(t) = self.sql.editors.iter_mut().find(|t| t.id == id)
                    && let Some(next) = t.redo_stack.pop()
                {
                    t.undo_stack.push(t.snapshot());
                    t.replace_text(&next.text, next.line, next.column);
                    t.undo_group_open = false;
                    t.last_edit_kind = None;
                }
                self.refresh_intellisense(id);
                self.scroll_editor_to_caret(id)
            }
            SqlMessage::NewlineAutoIndent(id) => {
                if let Some(t) = self.sql.editors.iter_mut().find(|t| t.id == id) {
                    let line = t.caret_line_text();
                    let caret = t.content.cursor().position.column.min(line.len());
                    // Only the indentation that precedes the caret carries over,
                    // so Enter at the start of an indented line does not indent.
                    let indent: String = line[..caret]
                        .chars()
                        .take_while(|c| *c == ' ' || *c == '\t')
                        .collect();
                    t.begin_edit_group(EditKind::Other);
                    t.content.perform(Action::Edit(Edit::Enter));
                    if !indent.is_empty() {
                        t.content
                            .perform(Action::Edit(Edit::Paste(Arc::new(indent))));
                    }
                    t.completion_armed = false;
                }
                self.refresh_intellisense(id);
                self.scroll_editor_to_caret(id)
            }
            SqlMessage::ToggleComment(id) => {
                if let Some(t) = self.sql.editors.iter_mut().find(|t| t.id == id) {
                    let (first, last) = t.selected_line_range();
                    let caret = t.content.cursor().position;
                    let mut lines: Vec<String> =
                        t.content.lines().map(|l| l.text.into_owned()).collect();
                    let targets = first..=last.min(lines.len().saturating_sub(1));
                    let all_commented = targets
                        .clone()
                        .filter(|i| !lines[*i].trim().is_empty())
                        .all(|i| lines[i].trim_start().starts_with("--"));
                    let any_content = targets.clone().any(|i| !lines[i].trim().is_empty());
                    if any_content {
                        for i in targets {
                            lines[i] = if all_commented {
                                uncomment_line(&lines[i])
                            } else {
                                comment_line(&lines[i])
                            };
                        }
                        let new_col = lines
                            .get(caret.line)
                            .map_or(0, |l| caret.column.min(l.len()));
                        t.set_text(&lines.join("\n"), caret.line, new_col);
                    }
                }
                self.refresh_intellisense(id);
                Task::none()
            }
            SqlMessage::DuplicateLine(id) => {
                if let Some(t) = self.sql.editors.iter_mut().find(|t| t.id == id) {
                    let caret = t.content.cursor().position;
                    let mut lines: Vec<String> =
                        t.content.lines().map(|l| l.text.into_owned()).collect();
                    if let Some(line) = lines.get(caret.line).cloned() {
                        lines.insert(caret.line + 1, line);
                        t.set_text(&lines.join("\n"), caret.line + 1, caret.column);
                    }
                }
                self.refresh_intellisense(id);
                self.scroll_editor_to_caret(id)
            }
            SqlMessage::CompletionRequest(id) => {
                if let Some(t) = self.sql.editors.iter_mut().find(|t| t.id == id) {
                    t.completion_armed = true;
                }
                self.refresh_intellisense_forced(id);
                Task::none()
            }
            SqlMessage::GotoDiagnostic(id) => {
                if let Some(t) = self.sql.editors.iter_mut().find(|t| t.id == id)
                    && let Some(d) = t.diagnostics.first()
                {
                    let line = d.line.unwrap_or(1).saturating_sub(1) as usize;
                    let col_chars = d.column.unwrap_or(1).saturating_sub(1) as usize;
                    let byte_col = t
                        .content
                        .line(line)
                        .map(|l| {
                            l.text
                                .char_indices()
                                .nth(col_chars)
                                .map_or(l.text.len(), |(b, _)| b)
                        })
                        .unwrap_or(0);
                    t.break_undo_group();
                    t.completion = None;
                    t.completion_armed = false;
                    restore_cursor(&mut t.content, line, byte_col);
                }
                self.refresh_intellisense(id);
                self.scroll_editor_to_caret(id)
            }
            SqlMessage::EditorResize(h) => {
                self.sql.editor_height = h.clamp(MIN_EDITOR_HEIGHT, MAX_EDITOR_HEIGHT);
                Task::none()
            }
            SqlMessage::EditorResizeEnd => {
                self.persist_ui_prefs();
                Task::none()
            }
            SqlMessage::EditorResizeReset => {
                self.sql.editor_height = DEFAULT_EDITOR_HEIGHT;
                self.persist_ui_prefs();
                Task::none()
            }
            SqlMessage::EditorScrolled {
                id,
                offset_y,
                height,
            } => {
                if let Some(t) = self.sql.editors.iter_mut().find(|t| t.id == id) {
                    t.editor_viewport = Some(EditorViewport { offset_y, height });
                }
                Task::none()
            }
            SqlMessage::Format(id) => {
                let Some(t) = self.sql.editors.iter_mut().find(|t| t.id == id) else {
                    return Task::none();
                };
                let style = sql_ide::FormatStyle::default();
                match t.content.selection() {
                    Some(sel) if !sel.trim().is_empty() => {
                        let formatted = sql_ide::format_sql(&sel, &style);
                        // Paste replaces the selection; snapshot first so the
                        // whole replacement undoes as one step.
                        t.push_undo();
                        t.break_undo_group();
                        t.content
                            .perform(Action::Edit(Edit::Paste(Arc::new(formatted))));
                        t.completion = None;
                        t.completion_armed = false;
                    }
                    _ => {
                        let text = t.content.text();
                        let formatted = sql_ide::format_sql(&text, &style);
                        if formatted != text {
                            t.set_text(&formatted, 0, 0);
                        }
                    }
                }
                self.refresh_intellisense(id);
                self.scroll_editor_to_caret(id)
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
            SqlMessage::RunSmart(id) => {
                let Some(t) = self.sql.editors.iter().find(|t| t.id == id) else {
                    return Task::none();
                };
                let sql = smart_run_sql(t);
                self.run_sql_text(id, sql)
            }
            SqlMessage::Cancel(id) => {
                if let Some(t) = self.sql.editors.iter_mut().find(|t| t.id == id) {
                    if let Some(handle) = t.query_handle.take() {
                        handle.abort();
                    }
                    t.running = false;
                }
                // Neutral, auto-clearing notice (reuses the export-notice pattern).
                self.copy_notice = Some("Query cancelled".to_string());
                Task::perform(
                    async {
                        tokio::time::sleep(std::time::Duration::from_millis(2500)).await;
                    },
                    |_| FileMessage::ClearCopyNotice.into(),
                )
            }
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
                    d.format = fmt;
                }
                Task::none()
            }
            SqlMessage::ExportSetCompression(id, c) => {
                if let Some(d) = self.export_dialog_mut(id) {
                    d.opts_parquet.compression = c;
                }
                Task::none()
            }
            SqlMessage::ExportSetDictionary(id, value) => {
                if let Some(d) = self.export_dialog_mut(id) {
                    d.opts_parquet.dictionary = value;
                }
                Task::none()
            }
            SqlMessage::ExportSetParquetVersion(id, version) => {
                if let Some(d) = self.export_dialog_mut(id) {
                    d.opts_parquet.version = version;
                }
                Task::none()
            }
            SqlMessage::ExportParquetColumnName(id, column) => {
                if let Some(d) = self.export_dialog_mut(id) {
                    d.parquet_column_name = column;
                }
                Task::none()
            }
            SqlMessage::ExportParquetColumnOptions {
                id,
                column,
                options,
            } => {
                if let Some(d) = self.export_dialog_mut(id) {
                    if let Some(options) = options {
                        d.opts_parquet.per_column_options.insert(column, options);
                    } else {
                        d.opts_parquet.per_column_options.remove(&column);
                    }
                }
                Task::none()
            }
            SqlMessage::ExportToggleHeader(id, value) => {
                if let Some(d) = self.export_dialog_mut(id) {
                    d.opts_csv.header = value;
                }
                Task::none()
            }
            SqlMessage::ExportToggleNdjson(id, value) => {
                if let Some(d) = self.export_dialog_mut(id) {
                    d.opts_json.ndjson = value;
                }
                Task::none()
            }
            SqlMessage::ExportDelimiter(id, s) => {
                if let Some(d) = self.export_dialog_mut(id)
                    && let Some(b) = s.bytes().next()
                {
                    d.opts_csv.delimiter = b;
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
                let fmt = dialog.format;
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

                let dialog = dialog.clone();
                let engine = t.engine.clone();
                let sql = t.content.text();
                Task::perform(
                    async move {
                        let stream = engine.export_stream(sql).await?;
                        Ok(match dialog.format {
                            ExportFormat::Parquet => {
                                dialog.opts_parquet.write_stream(stream, path).await?
                            }
                            ExportFormat::Csv => dialog.opts_csv.write_stream(stream, path).await?,
                            ExportFormat::Json => {
                                dialog.opts_json.write_stream(stream, path).await?
                            }
                        })
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
                elapsed,
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
                    t.query_handle = None;
                    t.last_elapsed = Some(elapsed);
                    t.explain = explain_kind;
                    // Name the tab after what it queries so several tabs on
                    // one source stay distinguishable in the strip.
                    t.title = match sql_ide::first_table_name(&sql) {
                        Some(table) => format!("{table} · {source_label}"),
                        None => source_label.clone(),
                    };
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
                    elapsed,
                    ran_at: SystemTime::now(),
                });
                // Refresh the queryable `history` table to include this run.
                self.register_state_tables_task()
            }
            SqlMessage::HistoryLoad(i) => {
                if let Some(entry) = self.sql.history.get(i) {
                    let sql = entry.sql.clone();
                    if let Some(t) = self.sql.editors.get_mut(self.sql.active) {
                        t.set_text(&sql, 0, 0);
                    }
                }
                Task::none()
            }
            SqlMessage::HistoryRerun(i) => {
                if let Some(entry) = self.sql.history.get(i) {
                    let sql = entry.sql.clone();
                    if let Some(t) = self.sql.editors.get_mut(self.sql.active) {
                        t.set_text(&sql, 0, 0);
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

/// What Ctrl+Enter runs: the selection if there is one, else the statement
/// under the caret, else the whole buffer.
fn smart_run_sql(t: &SqlEditorTab) -> String {
    match t.content.selection() {
        Some(sel) if !sel.trim().is_empty() => sel,
        _ => {
            let text = t.content.text();
            match sql_ide::statement_at(&text, t.caret_byte_offset()) {
                Some(range) => text[range].to_string(),
                None => text,
            }
        }
    }
}

/// The closing character auto-inserted after typing `c`, if any.
fn auto_close_pair(c: char) -> Option<char> {
    match c {
        '(' => Some(')'),
        '[' => Some(']'),
        '\'' => Some('\''),
        '"' => Some('"'),
        _ => None,
    }
}

fn is_closer(c: char) -> bool {
    matches!(c, ')' | ']')
}

/// Whether the caret sits between an opener and its matching closer with
/// nothing selected, e.g. `(|)`.
fn between_auto_pair(t: &SqlEditorTab) -> bool {
    if t.content.cursor().selection.is_some() {
        return false;
    }
    let line = t.caret_line_text();
    let caret = t.content.cursor().position.column.min(line.len());
    let prev = line[..caret].chars().next_back();
    let next = line[caret..].chars().next();
    prev.and_then(auto_close_pair)
        .is_some_and(|closer| next == Some(closer))
}

/// Insert `c` with bracket/quote auto-closing: a closer or quote typed before
/// an identical char just steps over it; an opener typed with no selection
/// inserts the pair and leaves the caret between them. Quotes directly after
/// an identifier char (`it's`) are inserted plainly.
fn apply_auto_close(t: &mut SqlEditorTab, c: char) {
    let line = t.caret_line_text();
    let caret = t.content.cursor().position.column.min(line.len());
    let next = line[caret..].chars().next();
    let prev = line[..caret].chars().next_back();
    let has_selection = t.content.cursor().selection.is_some();

    let Some(closer) = auto_close_pair(c) else {
        // A bare closer: step over a matching one, otherwise insert it.
        if next == Some(c) && !has_selection {
            t.content
                .perform(Action::Move(iced::widget::text_editor::Motion::Right));
        } else {
            t.content.perform(Action::Edit(Edit::Insert(c)));
        }
        return;
    };
    let is_quote = c == closer;
    if is_quote && next == Some(c) && !has_selection {
        t.content
            .perform(Action::Move(iced::widget::text_editor::Motion::Right));
        return;
    }
    let plain = has_selection
        || (is_quote && prev.is_some_and(|p| p.is_alphanumeric() || p == '_'))
        || next.is_some_and(|n| n.is_alphanumeric() || n == '_');
    t.content.perform(Action::Edit(Edit::Insert(c)));
    if !plain {
        t.content.perform(Action::Edit(Edit::Insert(closer)));
        t.content
            .perform(Action::Move(iced::widget::text_editor::Motion::Left));
    }
}

fn comment_line(line: &str) -> String {
    if line.trim().is_empty() {
        return line.to_string();
    }
    let indent = line.len() - line.trim_start().len();
    format!("{}-- {}", &line[..indent], &line[indent..])
}

fn uncomment_line(line: &str) -> String {
    let indent = line.len() - line.trim_start().len();
    let body = &line[indent..];
    let stripped = body
        .strip_prefix("-- ")
        .or_else(|| body.strip_prefix("--"))
        .unwrap_or(body);
    format!("{}{}", &line[..indent], stripped)
}

#[cfg(test)]
mod tests {
    use super::*;
    use iced::widget::text_editor::Motion;

    /// An app with one local editor tab holding `text`, caret at the end.
    fn app_with(text: &str) -> (App, u64) {
        let mut app = App::new();
        let local = app.local.clone();
        let id = app.push_editor(QueryEngine::Local(local), text.to_string());
        let t = app.sql.editors.iter_mut().find(|t| t.id == id).unwrap();
        t.content.perform(Action::Move(Motion::DocumentEnd));
        (app, id)
    }

    fn tab(app: &App, id: u64) -> &SqlEditorTab {
        app.sql.editors.iter().find(|t| t.id == id).unwrap()
    }

    fn send(app: &mut App, m: SqlMessage) {
        let _ = app.update_sql(m);
    }

    #[test]
    fn auto_close_paren_and_overtype() {
        let (mut app, id) = app_with("SELECT count");
        send(
            &mut app,
            SqlMessage::EditorAction(id, Action::Edit(Edit::Insert('('))),
        );
        assert_eq!(tab(&app, id).content.text(), "SELECT count()");
        assert_eq!(tab(&app, id).content.cursor().position.column, 13);
        send(
            &mut app,
            SqlMessage::EditorAction(id, Action::Edit(Edit::Insert('*'))),
        );
        send(
            &mut app,
            SqlMessage::EditorAction(id, Action::Edit(Edit::Insert(')'))),
        );
        // `)` typed before the auto-inserted closer steps over it.
        assert_eq!(tab(&app, id).content.text(), "SELECT count(*)");
    }

    #[test]
    fn backspace_between_pair_removes_both() {
        let (mut app, id) = app_with("SELECT ");
        send(
            &mut app,
            SqlMessage::EditorAction(id, Action::Edit(Edit::Insert('('))),
        );
        assert_eq!(tab(&app, id).content.text(), "SELECT ()");
        send(
            &mut app,
            SqlMessage::EditorAction(id, Action::Edit(Edit::Backspace)),
        );
        assert_eq!(tab(&app, id).content.text(), "SELECT ");
    }

    #[test]
    fn quote_after_identifier_is_plain() {
        let (mut app, id) = app_with("it");
        send(
            &mut app,
            SqlMessage::EditorAction(id, Action::Edit(Edit::Insert('\''))),
        );
        assert_eq!(tab(&app, id).content.text(), "it'");
    }

    #[test]
    fn newline_keeps_indentation() {
        let (mut app, id) = app_with("SELECT\n  a,");
        send(&mut app, SqlMessage::NewlineAutoIndent(id));
        assert_eq!(tab(&app, id).content.text(), "SELECT\n  a,\n  ");
        // One undo step reverts the whole Enter + indent.
        send(&mut app, SqlMessage::Undo(id));
        assert_eq!(tab(&app, id).content.text(), "SELECT\n  a,");
    }

    #[test]
    fn toggle_comment_round_trips() {
        let (mut app, id) = app_with("  SELECT 1");
        send(&mut app, SqlMessage::ToggleComment(id));
        assert_eq!(tab(&app, id).content.text(), "  -- SELECT 1");
        send(&mut app, SqlMessage::ToggleComment(id));
        assert_eq!(tab(&app, id).content.text(), "  SELECT 1");
    }

    #[test]
    fn duplicate_line_moves_caret_down() {
        let (mut app, id) = app_with("SELECT 1\nSELECT 2");
        send(&mut app, SqlMessage::DuplicateLine(id));
        assert_eq!(tab(&app, id).content.text(), "SELECT 1\nSELECT 2\nSELECT 2");
        assert_eq!(tab(&app, id).content.cursor().position.line, 2);
    }

    #[test]
    fn smart_run_picks_statement_under_caret() {
        let (mut app, id) = app_with("SELECT 1;\nSELECT 2;\nSELECT 3");
        assert_eq!(smart_run_sql(tab(&app, id)), "SELECT 3");
        let t = app.sql.editors.iter_mut().find(|t| t.id == id).unwrap();
        t.content.perform(Action::Move(Motion::DocumentStart));
        assert_eq!(smart_run_sql(tab(&app, id)), "SELECT 1");
        let t = app.sql.editors.iter_mut().find(|t| t.id == id).unwrap();
        t.content.perform(Action::SelectAll);
        assert_eq!(
            smart_run_sql(tab(&app, id)),
            "SELECT 1;\nSELECT 2;\nSELECT 3"
        );
        let (app, id) = app_with("SELECT 42");
        assert_eq!(smart_run_sql(tab(&app, id)), "SELECT 42");
    }

    #[test]
    fn format_rewrites_buffer_as_one_undo_step() {
        let (mut app, id) = app_with("select a from t where a>1");
        send(&mut app, SqlMessage::Format(id));
        let text = tab(&app, id).content.text();
        assert!(text.starts_with("SELECT"));
        assert!(text.contains("\nFROM"));
        send(&mut app, SqlMessage::Undo(id));
        assert_eq!(tab(&app, id).content.text(), "select a from t where a>1");
    }

    #[test]
    fn completion_opens_only_while_typing() {
        let (mut app, id) = app_with("SELECT * FROM t WHERE ");
        send(
            &mut app,
            SqlMessage::EditorAction(id, Action::Edit(Edit::Insert('s'))),
        );
        assert!(tab(&app, id).completion.is_some());
        // A caret move closes it, and clicking back to the word end does not
        // reopen it.
        send(
            &mut app,
            SqlMessage::EditorAction(id, Action::Move(Motion::Left)),
        );
        assert!(tab(&app, id).completion.is_none());
        send(
            &mut app,
            SqlMessage::EditorAction(id, Action::Move(Motion::Right)),
        );
        assert!(tab(&app, id).completion.is_none());
        send(&mut app, SqlMessage::CompletionRequest(id));
        assert!(tab(&app, id).completion.is_some());
    }

    #[test]
    fn bracket_pair_tracks_caret() {
        let (mut app, id) = app_with("SELECT (1)");
        app.refresh_intellisense(id);
        assert_eq!(tab(&app, id).bracket_pair, Some([(0, 7), (0, 9)]));
        send(
            &mut app,
            SqlMessage::EditorAction(id, Action::Move(Motion::DocumentStart)),
        );
        assert_eq!(tab(&app, id).bracket_pair, None);
    }

    #[test]
    fn goto_diagnostic_moves_caret() {
        let (mut app, id) = app_with("SELECT 1\nFROM t WHERE )");
        app.refresh_intellisense(id);
        assert!(!tab(&app, id).diagnostics.is_empty());
        send(
            &mut app,
            SqlMessage::EditorAction(id, Action::Move(Motion::DocumentStart)),
        );
        send(&mut app, SqlMessage::GotoDiagnostic(id));
        let d = tab(&app, id).diagnostics[0].clone();
        let pos = tab(&app, id).content.cursor().position;
        assert_eq!(pos.line as u64 + 1, d.line.unwrap());
    }
}
