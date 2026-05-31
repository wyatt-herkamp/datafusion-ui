//! Free helper functions used by the update handlers and views: settings-draft
//! mutation, schema signatures, column autofit, and small view builders.

use std::path::PathBuf;

use arrow::record_batch::RecordBatch;
use iced::widget::{Space, button, column, container, scrollable};
use iced::{Element, Length};

use crate::config::Config;
use crate::format::{default_options, row_strings};
use crate::store::RecentFile;
use crate::theme;
use crate::theme::palette;
use crate::widgets::MIN_COL_WIDTH;

use super::message::{FileMessage, Message, SettingsField, SettingsToggle};

/// Apply a text-input settings field onto the draft, parsing numbers leniently
/// (an unparseable/empty value leaves the field unchanged; bounds are enforced
/// on save via [`Config::sanitized`]).
pub fn apply_settings_field(cfg: &mut Config, field: SettingsField, value: &str) {
    let v = value.trim();
    match field {
        SettingsField::TargetPartitions => {
            if let Ok(n) = v.parse() {
                cfg.session.target_partitions = n;
            }
        }
        SettingsField::BatchSize => {
            if let Ok(n) = v.parse() {
                cfg.session.batch_size = n;
            }
        }
        SettingsField::ResultRowCap => {
            if let Ok(n) = v.parse() {
                cfg.result_row_cap = n;
            }
        }
        SettingsField::DefaultCatalog => cfg.session.default_catalog = value.to_string(),
        SettingsField::DefaultSchema => cfg.session.default_schema = value.to_string(),
        SettingsField::MemoryLimitMb => {
            if let Ok(n) = v.parse() {
                cfg.runtime.memory_limit_mb = n;
            }
        }
        SettingsField::DiskManagerPath => cfg.runtime.disk_manager_path = value.to_string(),
        SettingsField::MaxTempDirSizeMb => {
            if let Ok(n) = v.parse() {
                cfg.runtime.max_temp_dir_size_mb = n;
            }
        }
    }
}

/// Mutable reference to a numeric draft field (for the +/- steppers). Non-numeric
/// fields return `None`.
pub(crate) fn num_field_mut(cfg: &mut Config, field: SettingsField) -> Option<&mut usize> {
    match field {
        SettingsField::TargetPartitions => Some(&mut cfg.session.target_partitions),
        SettingsField::BatchSize => Some(&mut cfg.session.batch_size),
        SettingsField::ResultRowCap => Some(&mut cfg.result_row_cap),
        SettingsField::MemoryLimitMb => Some(&mut cfg.runtime.memory_limit_mb),
        SettingsField::MaxTempDirSizeMb => Some(&mut cfg.runtime.max_temp_dir_size_mb),
        SettingsField::DefaultCatalog
        | SettingsField::DefaultSchema
        | SettingsField::DiskManagerPath => None,
    }
}

/// Flip a boolean settings toggle on the draft.
pub fn apply_settings_toggle(cfg: &mut Config, toggle: SettingsToggle) {
    let s = &mut cfg.session;
    match toggle {
        SettingsToggle::RepartitionFileScans => {
            s.repartition_file_scans = !s.repartition_file_scans
        }
        SettingsToggle::RepartitionJoins => s.repartition_joins = !s.repartition_joins,
        SettingsToggle::RepartitionAggregations => {
            s.repartition_aggregations = !s.repartition_aggregations
        }
        SettingsToggle::RepartitionSorts => s.repartition_sorts = !s.repartition_sorts,
        SettingsToggle::InformationSchema => s.information_schema = !s.information_schema,
        SettingsToggle::CollectStatistics => s.collect_statistics = !s.collect_statistics,
    }
}

/// Stable signature of a schema (field names + types), used to key persisted
/// column widths so a re-run with the same shape restores its widths.
pub(crate) fn schema_signature(schema: &arrow::datatypes::Schema) -> String {
    schema
        .fields()
        .iter()
        .map(|f| format!("{}:{}", f.name(), f.data_type()))
        .collect::<Vec<_>>()
        .join("|")
}

/// Width that fits the widest visible value in `col` (header included),
/// approximating monospace glyph width. Scans up to 1000 rows to bound cost.
pub(crate) fn autofit_width(batch: &RecordBatch, col: usize) -> f32 {
    if col >= batch.num_columns() {
        return crate::views::data::CELL_WIDTH;
    }
    let opts = default_options();
    let mut max_chars = batch.schema().field(col).name().chars().count();
    let scan = batch.num_rows().min(1000);
    for r in 0..scan {
        if let Some(v) = row_strings(batch, r, &opts).get(col) {
            max_chars = max_chars.max(v.chars().count());
        }
    }
    const MONO_CHAR_PX: f32 = 7.0;
    const PADDING: f32 = 28.0;
    (max_chars as f32 * MONO_CHAR_PX + PADDING).clamp(MIN_COL_WIDTH, 600.0)
}

pub(crate) fn welcome_view(recent: &[RecentFile]) -> Element<'_, Message> {
    let open_btn = button(theme::ui_medium("Open Parquet file…").size(13))
        .style(theme::accent_button)
        .padding([6, 16])
        .on_press(FileMessage::OpenFilePressed.into());

    let mut col = column![
        theme::display_strong("DataFusion UI"),
        theme::mono_sm("Open a Parquet file or connect to a FlightSQL server to begin."),
        open_btn,
    ]
    .spacing(10);

    if !recent.is_empty() {
        let mut list = column![theme::label_text("RECENT FILES")].spacing(4);
        for rf in recent {
            let path = PathBuf::from(&rf.path);
            let name = path
                .file_name()
                .and_then(|n| n.to_str())
                .unwrap_or(rf.path.as_str())
                .to_string();
            let entry = button(
                column![
                    theme::ui_medium(name).size(13),
                    theme::mono_sm(rf.path.clone()),
                ]
                .spacing(1),
            )
            .style(theme::ghost_button)
            .padding([4, 8])
            .width(Length::Fixed(420.0))
            .on_press(FileMessage::FilePicked(Some(path)).into());
            list = list.push(entry);
        }
        col = col.push(Space::new().height(Length::Fixed(8.0))).push(list);
    }

    container(col)
        .padding(24)
        .center_x(Length::Fill)
        .center_y(Length::Fill)
        .into()
}

/// Wrap static content in a bidirectional scroll area.
pub(crate) fn scroll_body(content: Element<'_, Message>) -> Element<'_, Message> {
    scrollable(container(content).padding(12))
        .direction(iced::widget::scrollable::Direction::Both {
            vertical: iced::widget::scrollable::Scrollbar::default(),
            horizontal: iced::widget::scrollable::Scrollbar::default(),
        })
        .width(Length::Fill)
        .height(Length::Fill)
        .into()
}

/// Decide whether to open the completion popup at the cursor. We only complete
/// when the character before the cursor is part of an identifier or a `.`, so
/// the popup stays quiet on whitespace/punctuation. `line`/`column` are 0-based.
pub(crate) fn should_complete(text: &str, line: usize, column: usize) -> bool {
    if column == 0 {
        return false;
    }
    let Some(current) = text.split('\n').nth(line) else {
        return false;
    };
    current
        .chars()
        .nth(column - 1)
        .is_some_and(|c| c.is_alphanumeric() || c == '_' || c == '.')
}

pub(crate) fn flight_pill_style(_theme: &iced::Theme) -> iced::widget::container::Style {
    iced::widget::container::Style {
        background: Some(iced::Background::Color(palette::accent_cool_soft())),
        text_color: Some(palette::accent_cool()),
        border: iced::Border {
            color: palette::accent_cool(),
            width: 1.0,
            radius: 999.0.into(),
        },
        ..iced::widget::container::Style::default()
    }
}
