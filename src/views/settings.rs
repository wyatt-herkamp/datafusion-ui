//! The settings page: appearance (UI scale), the local DataFusion session
//! config, and the query result row cap. Edits mutate a draft `Config` held on
//! the app; "Save" commits and persists it.

use iced::widget::{Space, button, column, container, row, slider, text_input};
use iced::{Element, Length, Theme};

use crate::app::{Message, SettingsField, SettingsMessage, SettingsToggle};
use crate::config::{
    Config, DiskManagerKind, MemoryPoolKind, ThemeChoice, UI_SCALE_MAX, UI_SCALE_MIN,
};
use crate::theme;

const LABEL_W: f32 = 220.0;
const FIELD_W: f32 = 140.0;
const NUM_W: f32 = 110.0;

pub fn view(cfg: &Config) -> Element<'_, Message> {
    let appearance = section(
        "Appearance",
        column![
            theme_selector(cfg.appearance.theme),
            scale_row(cfg.appearance.ui_scale)
        ]
        .spacing(10),
    );

    let s = &cfg.session;
    let performance = section(
        "DataFusion session · performance",
        column![
            num_row(
                "Target partitions",
                s.target_partitions,
                1,
                SettingsField::TargetPartitions,
            ),
            hint(
                "1 keeps SELECT * in storage order; higher parallelizes scans but interleaves rows."
            ),
            num_row("Batch size", s.batch_size, 1024, SettingsField::BatchSize),
            toggle_row(
                "Repartition file scans",
                s.repartition_file_scans,
                SettingsToggle::RepartitionFileScans
            ),
            toggle_row(
                "Repartition joins",
                s.repartition_joins,
                SettingsToggle::RepartitionJoins
            ),
            toggle_row(
                "Repartition aggregations",
                s.repartition_aggregations,
                SettingsToggle::RepartitionAggregations
            ),
            toggle_row(
                "Repartition sorts",
                s.repartition_sorts,
                SettingsToggle::RepartitionSorts
            ),
        ]
        .spacing(10),
    );

    let behavior = section(
        "DataFusion session · behavior",
        column![
            toggle_row(
                "Enable information_schema",
                s.information_schema,
                SettingsToggle::InformationSchema
            ),
            toggle_row(
                "Collect statistics",
                s.collect_statistics,
                SettingsToggle::CollectStatistics
            ),
            text_row(
                "Default catalog",
                s.default_catalog.clone(),
                SettingsField::DefaultCatalog
            ),
            text_row(
                "Default schema",
                s.default_schema.clone(),
                SettingsField::DefaultSchema
            ),
        ]
        .spacing(10),
    );

    let r = &cfg.runtime;
    let runtime = section(
        "DataFusion runtime",
        column![
            pool_selector(r.memory_pool),
            num_row(
                "Memory limit (MiB)",
                r.memory_limit_mb,
                256,
                SettingsField::MemoryLimitMb
            ),
            hint("Applied only when the pool is Greedy or Fair-spill."),
            disk_selector(r.disk_manager),
            spill_dir_row(r.disk_manager_path.clone()),
            hint("Used only when the disk manager is set to Custom dir."),
            num_row(
                "Max spill dir (MiB)",
                r.max_temp_dir_size_mb,
                512,
                SettingsField::MaxTempDirSizeMb
            ),
            hint("0 keeps DataFusion's default cap (100 GB)."),
        ]
        .spacing(10),
    );

    let queries = section(
        "Queries",
        column![
            num_row(
                "Result row cap",
                cfg.result_row_cap,
                1000,
                SettingsField::ResultRowCap
            ),
            hint("The most rows a query pulls into the results grid."),
        ]
        .spacing(10),
    );

    let footer = row![
        button(theme::ui_medium("Save & apply").size(13))
            .style(theme::accent_button)
            .padding([8, 18])
            .on_press(SettingsMessage::Save.into()),
        Space::new().width(Length::Fixed(10.0)),
        button(theme::ui_medium("Cancel").size(13))
            .style(theme::ghost_button)
            .padding([8, 18])
            .on_press(SettingsMessage::Cancel.into()),
    ]
    .align_y(iced::Alignment::Center);

    column![
        theme::display_strong("Settings"),
        appearance,
        performance,
        behavior,
        runtime,
        queries,
        footer,
    ]
    .spacing(24)
    .width(Length::Fixed(620.0))
    .into()
}

fn theme_selector<'a>(current: ThemeChoice) -> Element<'a, Message> {
    let mut r = row![container(theme::ui("Theme")).width(Length::Fixed(LABEL_W))]
        .align_y(iced::Alignment::Center);
    for choice in ThemeChoice::ALL {
        r = r
            .push(
                button(theme::ui_medium(choice.label()).size(12))
                    .style(theme::tab_button(choice == current))
                    .padding([4, 12])
                    .on_press(SettingsMessage::SetTheme(choice).into()),
            )
            .push(Space::new().width(Length::Fixed(4.0)));
    }
    r.into()
}

fn pool_selector<'a>(current: MemoryPoolKind) -> Element<'a, Message> {
    let mut r = row![container(theme::ui("Memory pool")).width(Length::Fixed(LABEL_W))]
        .align_y(iced::Alignment::Center);
    for kind in MemoryPoolKind::ALL {
        r = r
            .push(
                button(theme::ui_medium(kind.label()).size(12))
                    .style(theme::tab_button(kind == current))
                    .padding([4, 12])
                    .on_press(SettingsMessage::SetMemoryPool(kind).into()),
            )
            .push(Space::new().width(Length::Fixed(4.0)));
    }
    r.into()
}

fn disk_selector<'a>(current: DiskManagerKind) -> Element<'a, Message> {
    let mut r = row![container(theme::ui("Disk manager")).width(Length::Fixed(LABEL_W))]
        .align_y(iced::Alignment::Center);
    for kind in DiskManagerKind::ALL {
        r = r
            .push(
                button(theme::ui_medium(kind.label()).size(12))
                    .style(theme::tab_button(kind == current))
                    .padding([4, 12])
                    .on_press(SettingsMessage::SetDiskManager(kind).into()),
            )
            .push(Space::new().width(Length::Fixed(4.0)));
    }
    r.into()
}

fn section<'a>(title: &str, body: impl Into<Element<'a, Message>>) -> Element<'a, Message> {
    column![theme::label_text(title), body.into()]
        .spacing(12)
        .into()
}

fn scale_row<'a>(scale: f32) -> Element<'a, Message> {
    let pct = format!("{}%", (scale * 100.0).round() as i32);
    row![
        container(theme::ui("UI scale")).width(Length::Fixed(LABEL_W)),
        slider(UI_SCALE_MIN..=UI_SCALE_MAX, scale, |v| {
            SettingsMessage::SetUiScale(v).into()
        })
        .step(0.05f32)
        .width(Length::Fixed(240.0)),
        Space::new().width(Length::Fixed(12.0)),
        theme::mono(pct),
    ]
    .align_y(iced::Alignment::Center)
    .into()
}

/// A numeric field: `[ − ]  <input>  [ + ]`. The steppers nudge the draft value
/// by `step`; the input still accepts free typing.
fn num_row<'a>(label: &str, value: usize, step: i64, field: SettingsField) -> Element<'a, Message> {
    let input = text_input("", &value.to_string())
        .on_input(move |v| SettingsMessage::Field(field, v).into())
        .padding([6, 8])
        .width(Length::Fixed(NUM_W));
    row![
        container(theme::ui(label.to_string())).width(Length::Fixed(LABEL_W)),
        step_btn("−", SettingsMessage::FieldStep(field, -step).into()),
        input,
        step_btn("+", SettingsMessage::FieldStep(field, step).into()),
    ]
    .spacing(6)
    .align_y(iced::Alignment::Center)
    .into()
}

fn step_btn<'a>(sym: &'static str, msg: Message) -> Element<'a, Message> {
    button(theme::ui_medium(sym).size(14))
        .style(theme::ghost_button)
        .padding([4, 11])
        .on_press(msg)
        .into()
}

fn text_row<'a>(label: &str, value: String, field: SettingsField) -> Element<'a, Message> {
    let input = text_input("", &value)
        .on_input(move |v| SettingsMessage::Field(field, v).into())
        .padding([6, 8])
        .width(Length::Fixed(FIELD_W));
    row![
        container(theme::ui(label.to_string())).width(Length::Fixed(LABEL_W)),
        input,
    ]
    .align_y(iced::Alignment::Center)
    .into()
}

/// The spill-directory field: a text input plus a folder picker.
fn spill_dir_row<'a>(value: String) -> Element<'a, Message> {
    let input = text_input("", &value)
        .on_input(|v| SettingsMessage::Field(SettingsField::DiskManagerPath, v).into())
        .padding([6, 8])
        .width(Length::Fixed(FIELD_W));
    row![
        container(theme::ui("Spill directory")).width(Length::Fixed(LABEL_W)),
        input,
        Space::new().width(Length::Fixed(6.0)),
        button(theme::ui_medium("Browse…").size(12))
            .style(theme::ghost_button)
            .padding([6, 12])
            .on_press(SettingsMessage::PickSpillDir.into()),
    ]
    .align_y(iced::Alignment::Center)
    .into()
}

fn toggle_row<'a>(label: &str, on: bool, toggle: SettingsToggle) -> Element<'a, Message> {
    let txt = if on { "On" } else { "Off" };
    row![
        container(theme::ui(label.to_string())).width(Length::Fixed(LABEL_W)),
        button(theme::ui_medium(txt).size(12))
            .style(theme::tab_button(on))
            .padding([4, 16])
            .on_press(SettingsMessage::ToggleFlag(toggle).into()),
    ]
    .align_y(iced::Alignment::Center)
    .into()
}

fn hint<'a>(s: &str) -> Element<'a, Message> {
    theme::mono_sm(s.to_string())
        .style(|_: &Theme| iced::widget::text::Style {
            color: Some(theme::palette::fg_dim()),
        })
        .into()
}
