use iced::Task;

use super::super::*;

impl App {
    pub(crate) fn update_settings(&mut self, m: SettingsMessage) -> Task<Message> {
        match m {
            SettingsMessage::Open => {
                self.settings_draft = Some(self.config.clone());
                self.selection = Selection::Settings;
                Task::none()
            }
            SettingsMessage::SetUiScale(scale) => {
                // Applies live so the user sees the zoom immediately.
                let scale = scale.clamp(crate::config::UI_SCALE_MIN, crate::config::UI_SCALE_MAX);
                self.config.appearance.ui_scale = scale;
                if let Some(draft) = &mut self.settings_draft {
                    draft.appearance.ui_scale = scale;
                }
                Task::none()
            }
            SettingsMessage::SetTheme(choice) => {
                // Applied to the draft only; the theme callback previews the
                // draft live and Cancel reverts by dropping it.
                if let Some(draft) = &mut self.settings_draft {
                    draft.appearance.theme = choice;
                }
                Task::none()
            }
            SettingsMessage::Field(field, value) => {
                if let Some(draft) = &mut self.settings_draft {
                    apply_settings_field(draft, field, &value);
                }
                Task::none()
            }
            SettingsMessage::FieldStep(field, delta) => {
                if let Some(draft) = &mut self.settings_draft
                    && let Some(v) = num_field_mut(draft, field)
                {
                    *v = (*v as i64 + delta).max(0) as usize;
                }
                Task::none()
            }
            SettingsMessage::PickSpillDir => {
                let parent = self.window_parent.clone();
                Task::perform(
                    async move {
                        let dialog = rfd::AsyncFileDialog::new().set_title("Select TMP directory");
                        let dialog = match &parent {
                            Some(p) => dialog.set_parent(p),
                            None => dialog,
                        };
                        dialog.pick_folder().await.map(|h| h.path().to_path_buf())
                    },
                    |p| SettingsMessage::SpillDirPicked(p).into(),
                )
            }
            SettingsMessage::SpillDirPicked(None) => Task::none(),
            SettingsMessage::SpillDirPicked(Some(path)) => {
                if let Some(draft) = &mut self.settings_draft {
                    draft.runtime.disk_manager_path = path.to_string_lossy().into_owned();
                    // Picking a directory implies wanting the custom disk manager.
                    draft.runtime.disk_manager = crate::config::DiskManagerKind::Specified;
                }
                Task::none()
            }
            SettingsMessage::ToggleFlag(toggle) => {
                if let Some(draft) = &mut self.settings_draft {
                    apply_settings_toggle(draft, toggle);
                }
                Task::none()
            }
            SettingsMessage::SetMemoryPool(kind) => {
                if let Some(draft) = &mut self.settings_draft {
                    draft.runtime.memory_pool = kind;
                }
                Task::none()
            }
            SettingsMessage::SetDiskManager(kind) => {
                if let Some(draft) = &mut self.settings_draft {
                    draft.runtime.disk_manager = kind;
                }
                Task::none()
            }
            SettingsMessage::Save => {
                let Some(draft) = self.settings_draft.take() else {
                    return Task::none();
                };
                let draft = draft.sanitized();
                let rebuild =
                    draft.session != self.config.session || draft.runtime != self.config.runtime;
                self.config = draft;
                self.config.save(&self.app_dir);
                self.selection = Selection::Sql;
                self.ensure_valid_selection();
                if rebuild {
                    self.rebuild_local_session()
                } else {
                    Task::none()
                }
            }
            SettingsMessage::Cancel => {
                // Discard draft; restore the live UI scale to the saved value.
                self.settings_draft = None;
                self.selection = Selection::Sql;
                self.ensure_valid_selection();
                Task::none()
            }
        }
    }
}
