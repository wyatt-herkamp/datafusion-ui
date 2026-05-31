use iced::widget::container::Style as ContainerStyle;
use iced::widget::text::Wrapping;
use iced::widget::{button, column, container, mouse_area, row, text};
use iced::{Background, Border, Element, Length, Theme};
use parquet::file::statistics::Statistics;

use crate::app::{FileMessage, Message};
use crate::format::human_bytes;
use crate::parquet_io::FileSummary;
use crate::views::overview::format_sorting_columns;

pub fn view(file: &FileSummary, selected: Option<usize>) -> Element<'_, Message> {
    let mut col = column![summary_header()].spacing(0);

    for (i, rg) in file.metadata.row_groups().iter().enumerate() {
        let compressed: i64 = rg.columns().iter().map(|c| c.compressed_size()).sum();
        let toggle_label = if selected == Some(i) { "▾" } else { "▸" };
        let zebra = i % 2 == 1;

        let summary = row![
            container(
                button(text(toggle_label.to_string()))
                    .on_press(FileMessage::RowGroupToggled(i).into())
                    .style(button::secondary),
            )
            .width(Length::Fixed(40.0))
            .padding([2, 4]),
            body_cell(format!("Group {i}"), 110.0, zebra),
            body_cell(format!("{}", rg.num_rows()), 110.0, zebra),
            body_cell(
                human_bytes(rg.total_byte_size().max(0) as u64),
                140.0,
                zebra
            ),
            body_cell(human_bytes(compressed.max(0) as u64), 140.0, zebra),
            body_cell(format!("{}", rg.num_columns()), 90.0, zebra),
        ]
        .spacing(0)
        .align_y(iced::Alignment::Center);

        let styled_summary =
            container(summary).style(move |theme: &Theme| body_row_style(theme, zebra));
        col = col.push(styled_summary);

        if selected == Some(i) {
            col = col.push(column_chunk_table(file, i));
        }
    }

    col.into()
}

fn summary_header() -> Element<'static, Message> {
    let r = row![
        container(text(" ")).width(Length::Fixed(40.0)),
        header_cell("Index", 110.0),
        header_cell("Rows", 110.0),
        header_cell("Uncompressed", 140.0),
        header_cell("Compressed", 140.0),
        header_cell("Columns", 90.0),
    ]
    .spacing(0);
    container(r).style(header_row_style).into()
}

fn column_chunk_table(file: &FileSummary, rg_idx: usize) -> Element<'_, Message> {
    let rg = file.metadata.row_group(rg_idx);
    let sort_label = match rg.sorting_columns() {
        Some(cols) if !cols.is_empty() => format_sorting_columns(file, cols),
        Some(_) => "(empty)".into(),
        None => "(not specified)".into(),
    };
    let sort_row = row![text("Sort order:").size(13), text(sort_label).size(13),]
        .spacing(8)
        .padding([0, 0]);
    let mut col = column![sort_row, chunk_header()].spacing(0);

    for (idx, cc) in rg.columns().iter().enumerate() {
        let encodings: Vec<String> = cc.encodings().map(|e| format!("{e:?}")).collect();
        let stats_str = cc
            .statistics()
            .map(format_stats)
            .unwrap_or_else(|| "—".into());
        let zebra = idx % 2 == 1;

        let r = row![
            body_cell(cc.column_path().string(), 240.0, zebra),
            body_cell(format!("{:?}", cc.compression()), 120.0, zebra),
            body_cell(encodings.join(", "), 220.0, zebra),
            body_cell(format!("{}", cc.num_values()), 90.0, zebra),
            body_cell(
                human_bytes(cc.uncompressed_size().max(0) as u64),
                120.0,
                zebra
            ),
            body_cell(
                human_bytes(cc.compressed_size().max(0) as u64),
                120.0,
                zebra
            ),
            body_cell(stats_str, 360.0, zebra),
        ]
        .spacing(0);
        let styled = container(r).style(move |theme: &Theme| body_row_style(theme, zebra));
        col = col.push(styled);
    }

    container(col).padding([4, 40]).into()
}

fn chunk_header() -> Element<'static, Message> {
    let r = row![
        header_cell("Column", 240.0),
        header_cell("Compression", 120.0),
        header_cell("Encodings", 220.0),
        header_cell("Values", 90.0),
        header_cell("Uncompressed", 120.0),
        header_cell("Compressed", 120.0),
        header_cell("Min / Max / Nulls", 360.0),
    ]
    .spacing(0);
    container(r).style(header_row_style).into()
}

fn header_cell<'a>(label: &str, width: f32) -> Element<'a, Message> {
    container(text(label.to_string()).size(13).wrapping(Wrapping::None))
        .width(Length::Fixed(width))
        .padding([6, 10])
        .clip(true)
        .into()
}

fn body_cell<'a>(value: String, width: f32, _zebra: bool) -> Element<'a, Message> {
    let label = text(value.clone()).size(13).wrapping(Wrapping::None);
    let inner = container(label)
        .width(Length::Fixed(width))
        .padding([4, 10])
        .clip(true);

    mouse_area(inner)
        .on_press(FileMessage::CopyCell(value).into())
        .into()
}

fn header_row_style(theme: &Theme) -> ContainerStyle {
    let p = theme.extended_palette();
    ContainerStyle {
        background: Some(Background::Color(p.background.strong.color)),
        text_color: Some(p.background.strong.text),
        border: Border::default(),
        ..ContainerStyle::default()
    }
}

fn body_row_style(theme: &Theme, zebra: bool) -> ContainerStyle {
    let p = theme.extended_palette();
    let bg = if zebra {
        p.background.weak.color
    } else {
        p.background.base.color
    };
    ContainerStyle {
        background: Some(Background::Color(bg)),
        text_color: Some(p.background.base.text),
        border: Border::default(),
        ..ContainerStyle::default()
    }
}

fn format_stats(stats: &Statistics) -> String {
    let nulls = stats
        .null_count_opt()
        .map(|n| n.to_string())
        .unwrap_or_else(|| "?".into());

    let (min, max) = match stats {
        Statistics::Boolean(s) => (opt_dbg(s.min_opt()), opt_dbg(s.max_opt())),
        Statistics::Int32(s) => (opt_dbg(s.min_opt()), opt_dbg(s.max_opt())),
        Statistics::Int64(s) => (opt_dbg(s.min_opt()), opt_dbg(s.max_opt())),
        Statistics::Int96(s) => (opt_dbg(s.min_opt()), opt_dbg(s.max_opt())),
        Statistics::Float(s) => (opt_dbg(s.min_opt()), opt_dbg(s.max_opt())),
        Statistics::Double(s) => (opt_dbg(s.min_opt()), opt_dbg(s.max_opt())),
        Statistics::ByteArray(s) => (
            s.min_opt()
                .map(|b| String::from_utf8_lossy(b.data()).to_string())
                .unwrap_or_else(|| "—".into()),
            s.max_opt()
                .map(|b| String::from_utf8_lossy(b.data()).to_string())
                .unwrap_or_else(|| "—".into()),
        ),
        Statistics::FixedLenByteArray(s) => (
            s.min_opt()
                .map(|b| format!("{:?}", b.data()))
                .unwrap_or_else(|| "—".into()),
            s.max_opt()
                .map(|b| format!("{:?}", b.data()))
                .unwrap_or_else(|| "—".into()),
        ),
    };

    format!("min={min} · max={max} · nulls={nulls}")
}

fn opt_dbg<T: std::fmt::Debug>(v: Option<&T>) -> String {
    match v {
        Some(v) => format!("{v:?}"),
        None => "—".into(),
    }
}
