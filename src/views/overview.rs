use arrow::datatypes::{DataType, Field};
use iced::widget::container::Style as ContainerStyle;
use iced::widget::text::Wrapping;
use iced::widget::{Space, button, column, container, mouse_area, row, text, tooltip};
use iced::{Background, Border, Element, Length, Theme};
use parquet::file::metadata::SortingColumn;

use crate::app::{FileMessage, Message};
use crate::format::{human_bytes, type_label, type_label_full};
use crate::parquet_io::FileSummary;
use crate::theme as ui_theme;

pub fn view<'a>(
    file: &'a FileSummary,
    expanded: &'a ahash::AHashSet<usize>,
) -> Element<'a, Message> {
    let meta = file.metadata.file_metadata();

    let (compressed, uncompressed) =
        file.metadata
            .row_groups()
            .iter()
            .fold((0_i64, 0_i64), |(c, u), rg| {
                let chunk_c: i64 = rg.columns().iter().map(|cc| cc.compressed_size()).sum();
                let chunk_u: i64 = rg.columns().iter().map(|cc| cc.uncompressed_size()).sum();
                (c + chunk_c, u + chunk_u)
            });

    let mut col = column![
        section("File"),
        kv("Path", file.path.display().to_string()),
        kv("Size on disk", human_bytes(file.file_size_bytes)),
        section("Contents"),
        kv("Total rows", format!("{}", file.total_rows)),
        kv("Row groups", format!("{}", file.metadata.num_row_groups())),
        kv("Columns", format!("{}", file.schema.fields().len())),
        section("Writer"),
        kv("Parquet version", format!("{}", meta.version())),
        kv(
            "Created by",
            meta.created_by().unwrap_or("(unknown)").to_string()
        ),
        section("Storage"),
        kv(
            "Uncompressed (sum)",
            human_bytes(uncompressed.max(0) as u64)
        ),
        kv("Compressed (sum)", human_bytes(compressed.max(0) as u64)),
        kv(
            "Compression ratio",
            if compressed > 0 {
                format!("{:.2}x", uncompressed as f64 / compressed as f64)
            } else {
                "—".into()
            },
        ),
    ]
    .spacing(6);

    col = col.push(section("Sort Order"));
    col = col.push(kv("Row groups", sort_order_summary(file)));

    if let Some(kv_pairs) = meta.key_value_metadata()
        && !kv_pairs.is_empty()
    {
        col = col.push(section("Key/Value Metadata"));
        for entry in kv_pairs {
            let value = entry.value.clone().unwrap_or_else(|| "(none)".into());
            col = col.push(kv(&entry.key, value));
        }
    }

    col = col.push(section("Schema"));
    col = col.push(schema_table(file, expanded));

    col.into()
}

pub fn format_sorting_columns(file: &FileSummary, cols: &[SortingColumn]) -> String {
    if cols.is_empty() {
        return "(none specified)".into();
    }
    cols.iter()
        .map(|sc| {
            let name = file
                .schema
                .fields()
                .get(sc.column_idx as usize)
                .map(|f| f.name().as_str())
                .unwrap_or("?");
            let dir = if sc.descending { "DESC" } else { "ASC" };
            let nulls = if sc.nulls_first {
                "NULLS FIRST"
            } else {
                "NULLS LAST"
            };
            format!("{name} {dir} {nulls}")
        })
        .collect::<Vec<_>>()
        .join(", ")
}

fn sort_order_summary(file: &FileSummary) -> String {
    let groups = file.metadata.row_groups();
    if groups.is_empty() {
        return "(no row groups)".into();
    }

    let key = |rg: &parquet::file::metadata::RowGroupMetaData| -> Option<Vec<(i32, bool, bool)>> {
        rg.sorting_columns().map(|v| {
            v.iter()
                .map(|sc| (sc.column_idx, sc.descending, sc.nulls_first))
                .collect()
        })
    };

    let first_key = key(&groups[0]);
    let uniform = groups.iter().all(|rg| key(rg) == first_key);

    match (first_key.is_some(), uniform) {
        (false, true) => "(not specified)".into(),
        (true, true) => {
            let cols = groups[0].sorting_columns().unwrap();
            format_sorting_columns(file, cols)
        }
        (_, false) => "(varies by row group — see Row Groups tab)".into(),
    }
}

fn section(title: &str) -> Element<'_, Message> {
    container(
        column![
            ui_theme::label_text(title),
            container(Space::new())
                .height(Length::Fixed(1.0))
                .width(Length::Fixed(28.0))
                .style(ui_theme::tab_underline),
        ]
        .spacing(4),
    )
    .padding([14, 0])
    .into()
}

fn kv<'a>(label: &'a str, value: impl Into<String>) -> Element<'a, Message> {
    let value = value.into();
    row![
        container(
            ui_theme::ui(label.to_string())
                .size(12)
                .style(|_: &Theme| text::Style {
                    color: Some(ui_theme::palette::fg_muted()),
                }),
        )
        .width(iced::Length::Fixed(180.0)),
        ui_theme::mono(value).style(|_: &Theme| text::Style {
            color: Some(ui_theme::palette::fg_primary()),
        }),
    ]
    .spacing(8)
    .into()
}

const SCHEMA_HEADERS: [&str; 7] = [
    "Column",
    "Physical",
    "Logical",
    "Arrow type",
    "Nullable",
    "Max def",
    "Max rep",
];

const SCHEMA_COL_WIDTHS: [f32; 7] = [240.0, 110.0, 220.0, 260.0, 80.0, 80.0, 80.0];

fn schema_table<'a>(
    file: &'a FileSummary,
    expanded: &'a ahash::AHashSet<usize>,
) -> Element<'a, Message> {
    let schema_descr = file.metadata.file_metadata().schema_descr();
    let num_cols = schema_descr.num_columns();

    let mut rows = column![
        schema_header_row(),
        container(Space::new())
            .width(Length::Fill)
            .height(Length::Fixed(1.0))
            .style(|_: &Theme| ContainerStyle {
                background: Some(Background::Color(ui_theme::palette::border_strong())),
                ..ContainerStyle::default()
            }),
    ]
    .spacing(0);
    for i in 0..num_cols {
        let col_desc = schema_descr.column(i);
        let arrow_field = file.schema.fields().get(i);

        let name = col_desc.path().string();
        let physical = format!("{:?}", col_desc.physical_type());
        let logical = col_desc
            .logical_type_ref()
            .map(|l| format!("{l:?}"))
            .unwrap_or_else(|| format!("{:?}", col_desc.converted_type()));
        let arrow_dt = arrow_field.map(|f| f.data_type().clone());
        let nullable = arrow_field
            .map(|f| if f.is_nullable() { "yes" } else { "no" }.to_string())
            .unwrap_or_else(|| "—".into());
        let max_def = col_desc.max_def_level().to_string();
        let max_rep = col_desc.max_rep_level().to_string();

        let zebra = i % 2 == 1;
        let expandable = arrow_dt.as_ref().map(is_expandable_type).unwrap_or(false);
        let is_open = expanded.contains(&i);

        rows = rows.push(schema_data_row(
            &name,
            &physical,
            &logical,
            arrow_dt.as_ref(),
            &nullable,
            &max_def,
            &max_rep,
            zebra,
            expandable,
            is_open,
            i,
        ));
        rows = rows.push(
            container(Space::new())
                .width(Length::Fill)
                .height(Length::Fixed(1.0))
                .style(|_: &Theme| ContainerStyle {
                    background: Some(Background::Color(ui_theme::palette::border_subtle())),
                    ..ContainerStyle::default()
                }),
        );

        if expandable
            && is_open
            && let Some(dt) = arrow_dt.as_ref()
        {
            for child_row in schema_child_rows(dt, 1) {
                rows = rows.push(child_row);
            }
        }
    }

    rows.into()
}

fn is_expandable_type(dt: &DataType) -> bool {
    matches!(
        dt,
        DataType::Struct(_)
            | DataType::List(_)
            | DataType::LargeList(_)
            | DataType::FixedSizeList(_, _)
            | DataType::Map(_, _)
    )
}

fn schema_child_rows<'a>(dt: &DataType, depth: usize) -> Vec<Element<'a, Message>> {
    let mut out = Vec::new();
    match dt {
        DataType::Struct(fields) => {
            for f in fields.iter() {
                out.extend(child_row_for_field(f, depth));
            }
        }
        DataType::List(f) | DataType::LargeList(f) | DataType::FixedSizeList(f, _) => {
            out.extend(child_row_for_field(f, depth));
        }
        DataType::Map(entry, _) => {
            if let DataType::Struct(fs) = entry.data_type() {
                for f in fs.iter() {
                    out.extend(child_row_for_field(f, depth));
                }
            }
        }
        _ => {}
    }
    out
}

fn child_row_for_field<'a>(f: &Field, depth: usize) -> Vec<Element<'a, Message>> {
    let mut out = Vec::new();
    let indent = "  ".repeat(depth);
    let name = format!("{indent}↳ {}", f.name());
    let type_str = type_label(f.data_type());
    let full = type_label_full(f.data_type());
    let nullable = if f.is_nullable() { "yes" } else { "no" }.to_string();
    out.push(schema_child_row(
        &name,
        &type_str,
        &full,
        &nullable,
        depth,
        Some(f.data_type()),
    ));
    if is_expandable_type(f.data_type()) {
        out.extend(schema_child_rows(f.data_type(), depth + 1));
    }
    out
}

fn schema_header_row() -> Element<'static, Message> {
    let mut r = row![].spacing(0);
    for (i, h) in SCHEMA_HEADERS.iter().enumerate() {
        r = r.push(schema_header_cell(h, SCHEMA_COL_WIDTHS[i]));
    }
    container(r).style(schema_header_row_style).into()
}

#[allow(clippy::too_many_arguments)]
fn schema_data_row<'a>(
    name: &str,
    physical: &str,
    logical: &str,
    arrow_dt: Option<&DataType>,
    nullable: &str,
    max_def: &str,
    max_rep: &str,
    zebra: bool,
    expandable: bool,
    is_open: bool,
    row_index: usize,
) -> Element<'a, Message> {
    let arrow_compact = arrow_dt.map(type_label).unwrap_or_else(|| "—".into());
    let arrow_full = arrow_dt.map(type_label_full).unwrap_or_else(|| "—".into());

    let mut r = row![].spacing(0);
    r = r.push(schema_body_cell(name.to_string(), SCHEMA_COL_WIDTHS[0]));
    r = r.push(schema_body_cell(physical.to_string(), SCHEMA_COL_WIDTHS[1]));
    r = r.push(schema_body_cell(logical.to_string(), SCHEMA_COL_WIDTHS[2]));
    r = r.push(arrow_type_cell(
        arrow_compact,
        arrow_full,
        expandable,
        is_open,
        row_index,
        SCHEMA_COL_WIDTHS[3],
        arrow_dt,
    ));
    r = r.push(schema_body_cell(nullable.to_string(), SCHEMA_COL_WIDTHS[4]));
    r = r.push(schema_body_cell(max_def.to_string(), SCHEMA_COL_WIDTHS[5]));
    r = r.push(schema_body_cell(max_rep.to_string(), SCHEMA_COL_WIDTHS[6]));
    container(r)
        .style(move |theme: &Theme| schema_body_row_style(theme, zebra))
        .into()
}

fn schema_child_row<'a>(
    name: &str,
    type_str: &str,
    type_full: &str,
    nullable: &str,
    depth: usize,
    child_dt: Option<&DataType>,
) -> Element<'a, Message> {
    let mut r = row![].spacing(0);
    r = r.push(schema_body_cell(name.to_string(), SCHEMA_COL_WIDTHS[0]));
    r = r.push(schema_body_cell(String::new(), SCHEMA_COL_WIDTHS[1]));
    r = r.push(schema_body_cell(String::new(), SCHEMA_COL_WIDTHS[2]));
    r = r.push(arrow_type_cell(
        type_str.to_string(),
        type_full.to_string(),
        false,
        false,
        0,
        SCHEMA_COL_WIDTHS[3],
        child_dt,
    ));
    r = r.push(schema_body_cell(nullable.to_string(), SCHEMA_COL_WIDTHS[4]));
    r = r.push(schema_body_cell(String::new(), SCHEMA_COL_WIDTHS[5]));
    r = r.push(schema_body_cell(String::new(), SCHEMA_COL_WIDTHS[6]));
    let _ = depth;
    container(r).style(schema_child_row_style).into()
}

fn arrow_type_cell<'a>(
    compact: String,
    full: String,
    expandable: bool,
    is_open: bool,
    row_index: usize,
    width: f32,
    arrow_dt: Option<&DataType>,
) -> Element<'a, Message> {
    let chevron: Element<'a, Message> = if expandable {
        let arrow_glyph = if is_open { "▾" } else { "▸" };
        button(text(arrow_glyph).size(11).style(|_: &Theme| text::Style {
            color: Some(ui_theme::palette::fg_muted()),
        }))
        .style(button::text)
        .padding([0, 4])
        .on_press(FileMessage::ToggleSchemaRow(row_index).into())
        .into()
    } else {
        Space::new().width(Length::Fixed(14.0)).into()
    };

    let colors = match arrow_dt {
        Some(dt) if crate::format::is_nested(dt) => ui_theme::pill_colors_nested(),
        Some(dt) => ui_theme::pill_colors_for(crate::wrangle::insights::classify(dt)),
        None => ui_theme::PillColors {
            bg: ui_theme::palette::bg_surface_2(),
            fg: ui_theme::palette::fg_muted(),
        },
    };
    let pill = container(
        text(compact.clone())
            .font(ui_theme::FONT_MONO)
            .size(10)
            .wrapping(Wrapping::None),
    )
    .padding([1, 6])
    .style(ui_theme::pill_style(colors));

    let body = row![chevron, pill]
        .spacing(4)
        .align_y(iced::Alignment::Center);

    let inner = container(body)
        .width(Length::Fixed(width))
        .padding([4, 10])
        .clip(true);

    let needs_tooltip = compact != full && !full.is_empty() && full != "—";
    let with_tip: Element<'a, Message> = if needs_tooltip {
        tooltip(inner, type_tooltip(full.clone()), tooltip::Position::Bottom).into()
    } else {
        inner.into()
    };

    mouse_area(with_tip)
        .on_press(FileMessage::CopyCell(compact).into())
        .into()
}

fn type_tooltip<'a>(content: String) -> Element<'a, Message> {
    container(ui_theme::mono_sm(content).style(|_: &Theme| text::Style {
        color: Some(ui_theme::palette::fg_primary()),
    }))
    .padding([6, 10])
    .max_width(640.0)
    .style(|_: &Theme| ContainerStyle {
        background: Some(Background::Color(ui_theme::palette::bg_surface_2())),
        text_color: Some(ui_theme::palette::fg_primary()),
        border: Border {
            color: ui_theme::palette::border_strong(),
            width: 1.0,
            radius: 4.0.into(),
        },
        ..ContainerStyle::default()
    })
    .into()
}

fn schema_child_row_style(_theme: &Theme) -> ContainerStyle {
    ContainerStyle {
        background: Some(Background::Color(ui_theme::palette::bg_surface())),
        text_color: Some(ui_theme::palette::fg_muted()),
        border: Border {
            color: ui_theme::palette::border_subtle(),
            width: 0.0,
            radius: 0.0.into(),
        },
        ..ContainerStyle::default()
    }
}

fn schema_header_cell<'a>(s: &str, width: f32) -> Element<'a, Message> {
    container(ui_theme::label_text(s).wrapping(Wrapping::None))
        .width(Length::Fixed(width))
        .padding([8, 12])
        .clip(true)
        .into()
}

fn schema_body_cell<'a>(value: String, width: f32) -> Element<'a, Message> {
    let label = ui_theme::mono(value.clone()).wrapping(Wrapping::None);
    let inner = container(label)
        .width(Length::Fixed(width))
        .padding([4, 12])
        .clip(true);
    mouse_area(inner)
        .on_press(FileMessage::CopyCell(value).into())
        .into()
}

fn schema_header_row_style(_theme: &Theme) -> ContainerStyle {
    ContainerStyle {
        background: Some(Background::Color(ui_theme::palette::bg_surface())),
        text_color: Some(ui_theme::palette::fg_muted()),
        border: Border {
            color: ui_theme::palette::border_subtle(),
            width: 0.0,
            radius: 0.0.into(),
        },
        ..ContainerStyle::default()
    }
}

fn schema_body_row_style(_theme: &Theme, _zebra: bool) -> ContainerStyle {
    ContainerStyle {
        background: Some(Background::Color(ui_theme::palette::bg_deep())),
        text_color: Some(ui_theme::palette::fg_primary()),
        border: Border {
            color: ui_theme::palette::border_subtle(),
            width: 0.0,
            radius: 0.0.into(),
        },
        ..ContainerStyle::default()
    }
}
