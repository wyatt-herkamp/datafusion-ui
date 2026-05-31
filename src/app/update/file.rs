use iced::Task;

use super::super::*;

impl App {
    pub(crate) fn update_file(&mut self, m: FileMessage) -> Task<Message> {
        match m {
            FileMessage::OpenFilePressed => {
                let parent = self.window_parent.clone();
                Task::perform(
                    async move {
                        let dialog = rfd::AsyncFileDialog::new()
                            .add_filter("Parquet", &["parquet"])
                            .set_title("Open File");
                        let dialog = match &parent {
                            Some(p) => dialog.set_parent(p),
                            None => dialog,
                        };
                        dialog.pick_file().await.map(|h| h.path().to_path_buf())
                    },
                    |p| FileMessage::FilePicked(p).into(),
                )
            }
            FileMessage::FilePicked(None) => Task::none(),
            FileMessage::FilePicked(Some(path)) => {
                tracing::info!(path = %path.display(), "opening file");
                self.loading = true;
                self.error = None;
                Task::perform(load_metadata(path), |r| FileMessage::FileLoaded(r).into())
            }
            FileMessage::FileLoaded(Ok(summary)) => {
                self.loading = false;
                let path = summary.path.clone();
                self.record_recent_file(&path);
                // Derive a unique table name against the files already open.
                let existing: std::collections::HashSet<String> =
                    self.files.iter().map(|f| f.table_name.clone()).collect();
                let table_name = derive_table_name(&path, &existing);
                let id = FileId(self.next_file_id);
                self.next_file_id += 1;
                self.files
                    .push(FileTab::new(id, summary, table_name.clone()));
                self.explorer.on_file_open(id, table_name.clone());
                self.selection = Selection::File {
                    id,
                    view: FileView::Overview,
                };
                let Some(path_str) = path.to_str().map(str::to_string) else {
                    if let Some(ft) = self.file_mut(id) {
                        ft.register_error = Some("path is not valid UTF-8".into());
                    }
                    return Task::none();
                };
                let shared = self.local.clone();
                Task::perform(shared.register_file(table_name, path_str), move |result| {
                    FileMessage::Registered { file: id, result }.into()
                })
            }
            FileMessage::Registered { file, result } => {
                if let Some(ft) = self.file_mut(file) {
                    match result {
                        Ok(_schema) => {
                            ft.registered = true;
                            ft.register_error = None;
                        }
                        Err(e) => {
                            ft.register_error = Some(e.to_string());
                        }
                    }
                }
                Task::none()
            }
            FileMessage::FileLoaded(Err(e)) => {
                tracing::error!(error = %e, "file load failed");
                self.loading = false;
                self.error = Some(e.to_string());
                Task::none()
            }
            FileMessage::SelectFileView { file, view } => {
                self.selection = Selection::File { id: file, view };
                Task::none()
            }
            FileMessage::SelectSql => {
                self.selection = Selection::Sql;
                Task::none()
            }
            FileMessage::CloseFile { file } => {
                // Drop the file's table from the shared session; local editors
                // persist (they target the session, not a single file).
                if let Some(ft) = self.file(file) {
                    self.local.deregister(&ft.table_name);
                }
                self.files.retain(|f| f.id != file);
                self.explorer.on_file_close(file);
                self.ensure_valid_selection();
                Task::none()
            }
            FileMessage::RowGroupToggled(i) => {
                if let Some(ft) = self.active_file_mut() {
                    ft.selected_row_group = if ft.selected_row_group == Some(i) {
                        None
                    } else {
                        Some(i)
                    };
                }
                Task::none()
            }
            FileMessage::CopyCell(value) => {
                let preview = if value.chars().count() > 60 {
                    let mut p: String = value.chars().take(57).collect();
                    p.push('…');
                    p
                } else {
                    value.clone()
                };
                self.copy_notice = Some(format!("Copied: {preview}"));
                let clear = Task::perform(
                    async {
                        tokio::time::sleep(std::time::Duration::from_millis(1800)).await;
                    },
                    |_| FileMessage::ClearCopyNotice.into(),
                );
                Task::batch([iced::clipboard::write::<Message>(value), clear])
            }
            FileMessage::ClearCopyNotice => {
                self.copy_notice = None;
                Task::none()
            }
            FileMessage::WindowReady(handle) => {
                self.window_parent = handle;
                Task::none()
            }
            FileMessage::ToggleSchemaRow(i) => {
                if let Some(ft) = self.active_file_mut()
                    && !ft.expanded_schema_rows.remove(&i)
                {
                    ft.expanded_schema_rows.insert(i);
                }
                Task::none()
            }
        }
    }
}
