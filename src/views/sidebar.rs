//! The persistent global object-explorer sidebar.
//!
//! Lists every open Parquet file (each expanding to its Overview / Row Groups /
//! Data views plus a schema sub-tree) and every FlightSQL connection (with its
//! lazily-loaded catalog→schema→table→column hierarchy), plus an entry into the
//! shared SQL workspace. Selecting a node drives `App::selection`.

use iced::widget::container::Style as ContainerStyle;
use iced::widget::text::Wrapping;
use iced::widget::{Space, button, column, container, row, scrollable, text};
use iced::{Background, Border, Color, Element, Length, Theme};

use crate::app::{
    App, ConnInfoMessage, ExplorerMessage, FileMessage, FileView, Message, Selection, SourceRef,
    SqlMessage,
};
use crate::explorer::{
    CatalogNode, ExplorerTarget, FileNode, FlightTree, SchemaNode, TableNode, qualify,
};
use crate::theme;
use crate::theme::palette;

const SIDEBAR_WIDTH: f32 = 300.0;
const INDENT: f32 = 12.0;

pub fn view(app: &App) -> Element<'_, Message> {
    if app.explorer.collapsed {
        return container(
            button(theme::label_text("Explorer ▸"))
                .style(theme::ghost_button)
                .padding([6, 8])
                .on_press(ExplorerMessage::PanelToggle.into()),
        )
        .padding(6)
        .height(Length::Fill)
        .style(sidebar_style)
        .into();
    }

    let header = row![
        theme::ui_medium("Explorer").size(13),
        Space::new().width(Length::Fill),
        button(theme::label_text("◂ Hide"))
            .style(theme::ghost_button)
            .padding([4, 6])
            .on_press(ExplorerMessage::PanelToggle.into()),
    ]
    .align_y(iced::Alignment::Center);

    let mut col = column![header].spacing(2);

    // Open files, each with its per-file views + schema.
    for node in &app.explorer.files {
        col = col.push(file_subtree(app, node));
    }

    // The local SQL workspace is for the shared session of open files.
    if !app.files.is_empty() {
        col = col.push(sql_workspace_row(matches!(app.selection, Selection::Sql)));
    }

    // FlightSQL connections, each with its own SQL workspace + info actions.
    for (conn, c) in app.connections.iter().enumerate() {
        if let Some(tree) = app.explorer.flights.get(conn) {
            let info_selected =
                matches!(app.selection, Selection::ConnInfo { conn: c } if c == conn);
            col = col.push(flight_subtree(conn, &c.label, tree, info_selected));
        }
    }

    if app.files.is_empty() && app.connections.is_empty() {
        col = col.push(theme::mono_sm("No sources. Open a file or connect."));
    }

    container(scrollable(container(col).padding([0, 8])).height(Length::Fill))
        .width(Length::Fixed(SIDEBAR_WIDTH))
        .height(Length::Fill)
        .style(sidebar_style)
        .into()
}

// -- File sub-tree ------------------------------------------------------------

fn file_subtree<'a>(app: &'a App, node: &'a FileNode) -> Element<'a, Message> {
    let tri = if node.expanded { "▾" } else { "▸" };
    let toggle = button(
        row![
            indent(0),
            text(tri).size(10).color(theme::palette::fg_muted()),
            Space::new().width(Length::Fixed(4.0)),
            theme::mono_sm(elide(&format!("file: {}", node.name), 24))
                .color(theme::palette::accent_warm())
                .wrapping(Wrapping::None),
        ]
        .align_y(iced::Alignment::Center),
    )
    .style(theme::ghost_button)
    .width(Length::Fill)
    .padding([2, 4])
    .on_press(ExplorerMessage::Toggle(ExplorerTarget::FileRoot { file: node.id }).into());

    // Open a SQL workspace tab seeded with `SELECT * FROM <table> LIMIT 100`.
    let sql_btn = button(text("≫").size(10).color(theme::palette::accent_cool()))
        .style(theme::ghost_button)
        .padding([2, 6])
        .on_press(SqlMessage::NewQueryForSource(SourceRef::File(node.id)).into());

    let close = button(text("✕").size(10))
        .style(theme::ghost_button)
        .padding([2, 6])
        .on_press(FileMessage::CloseFile { file: node.id }.into());

    let mut col = column![row![toggle, sql_btn, close].align_y(iced::Alignment::Center)].spacing(2);

    if node.expanded {
        for (label, view) in [
            ("Overview", FileView::Overview),
            ("Row Groups", FileView::RowGroups),
        ] {
            let selected = matches!(app.selection,
                Selection::File { id, view: v } if id == node.id && v == view);
            col = col.push(nav_row(
                1,
                label,
                selected,
                FileMessage::SelectFileView {
                    file: node.id,
                    view,
                }
                .into(),
            ));
        }

        if let Some(ft) = app.files.iter().find(|f| f.id == node.id) {
            let fields = ft.summary.schema.fields();
            col = col.push(branch_row(
                1,
                node.schema_expanded,
                format!("schema ({} cols)", fields.len()),
                theme::palette::fg_muted(),
                ExplorerMessage::Toggle(ExplorerTarget::FileSchema { file: node.id }).into(),
            ));
            if node.schema_expanded {
                for field in fields {
                    col = col.push(column_row(2, field.name(), &field.data_type().to_string()));
                }
            }
        }
    }
    col.into()
}

fn nav_row<'a>(depth: u16, label: &str, selected: bool, msg: Message) -> Element<'a, Message> {
    let (dot, fg) = if selected {
        (theme::palette::accent_warm(), theme::palette::fg_primary())
    } else {
        (theme::palette::fg_dim(), theme::palette::fg_muted())
    };
    let content = row![
        indent(depth),
        text("•").size(10).color(dot),
        Space::new().width(Length::Fixed(4.0)),
        theme::mono_sm(label.to_string())
            .color(fg)
            .wrapping(Wrapping::None),
    ]
    .align_y(iced::Alignment::Center);
    button(content)
        .style(theme::ghost_button)
        .width(Length::Fill)
        .padding([2, 4])
        .on_press(msg)
        .into()
}

fn sql_workspace_row<'a>(selected: bool) -> Element<'a, Message> {
    let fg = if selected {
        theme::palette::fg_primary()
    } else {
        theme::palette::accent_cool()
    };
    button(
        row![
            indent(0),
            text("≫").size(10).color(theme::palette::accent_cool()),
            Space::new().width(Length::Fixed(4.0)),
            theme::mono_sm("Local SQL Workspace")
                .color(fg)
                .wrapping(Wrapping::None),
        ]
        .align_y(iced::Alignment::Center),
    )
    .style(theme::ghost_button)
    .width(Length::Fill)
    .padding([2, 4])
    .on_press(FileMessage::SelectSql.into())
    .into()
}

// -- FlightSQL sub-tree -------------------------------------------------------

fn flight_subtree<'a>(
    conn: usize,
    label: &str,
    tree: &'a FlightTree,
    info_selected: bool,
) -> Element<'a, Message> {
    let tri = if tree.expanded { "▾" } else { "▸" };
    let toggle = button(
        row![
            indent(0),
            text(tri).size(10).color(theme::palette::fg_muted()),
            Space::new().width(Length::Fixed(4.0)),
            theme::mono_sm(elide(&format!("flight: {label}"), 22))
                .color(theme::palette::accent_cool())
                .wrapping(Wrapping::None),
        ]
        .align_y(iced::Alignment::Center),
    )
    .style(theme::ghost_button)
    .width(Length::Fill)
    .padding([2, 4])
    .on_press(ExplorerMessage::Toggle(ExplorerTarget::FlightRoot { conn }).into());
    // Open a SQL workspace tab bound to this connection.
    let sql_btn = button(text("≫").size(10).color(theme::palette::accent_cool()))
        .style(theme::ghost_button)
        .padding([2, 6])
        .on_press(SqlMessage::NewQueryForSource(SourceRef::Flight(conn)).into());
    let info_color = if info_selected {
        theme::palette::fg_primary()
    } else {
        theme::palette::fg_muted()
    };
    let info_btn = button(text("ⓘ").size(10).color(info_color))
        .style(theme::ghost_button)
        .padding([2, 6])
        .on_press(ConnInfoMessage::Open(conn).into());
    let header = row![toggle, sql_btn, info_btn].align_y(iced::Alignment::Center);

    let mut col = column![header].spacing(2);

    if tree.expanded {
        if tree.loading {
            col = col.push(status_row(1, "loading catalogs…"));
        } else if let Some(e) = &tree.error {
            col = col.push(error_row(1, e));
        } else if tree.catalogs.is_empty() {
            col = col.push(status_row(1, "(no catalogs)"));
        } else {
            for cat in &tree.catalogs {
                col = col.push(catalog_subtree(conn, cat));
            }
        }
    }
    col.into()
}

fn catalog_subtree<'a>(conn: usize, cat: &'a CatalogNode) -> Element<'a, Message> {
    let target = ExplorerTarget::Catalog {
        conn,
        catalog: cat.name.clone(),
    };
    let mut col = column![branch_row(
        1,
        cat.expanded,
        cat.name.clone(),
        theme::palette::accent_violet(),
        ExplorerMessage::Toggle(target).into(),
    )]
    .spacing(2);

    if cat.expanded {
        if cat.loading {
            col = col.push(status_row(2, "loading schemas…"));
        } else if let Some(e) = &cat.error {
            col = col.push(error_row(2, e));
        } else if cat.schemas.is_empty() {
            col = col.push(status_row(2, "(no schemas)"));
        } else {
            for sch in &cat.schemas {
                col = col.push(schema_subtree(conn, &cat.name, sch));
            }
        }
    }
    col.into()
}

fn schema_subtree<'a>(conn: usize, catalog: &str, sch: &'a SchemaNode) -> Element<'a, Message> {
    let target = ExplorerTarget::Schema {
        conn,
        catalog: catalog.to_string(),
        schema: sch.name.clone(),
    };
    let label = sch.name.clone().unwrap_or_else(|| "(default)".into());
    let mut col = column![branch_row(
        2,
        sch.expanded,
        label,
        theme::palette::fg_muted(),
        ExplorerMessage::Toggle(target).into(),
    )]
    .spacing(2);

    if sch.expanded {
        if sch.loading {
            col = col.push(status_row(3, "loading tables…"));
        } else if let Some(e) = &sch.error {
            col = col.push(error_row(3, e));
        } else if sch.tables.is_empty() {
            col = col.push(status_row(3, "(no tables)"));
        } else {
            for tbl in &sch.tables {
                col = col.push(table_subtree(conn, catalog, &sch.name, tbl));
            }
        }
    }
    col.into()
}

fn table_subtree<'a>(
    conn: usize,
    catalog: &str,
    schema: &Option<String>,
    tbl: &'a TableNode,
) -> Element<'a, Message> {
    let qualified = qualify(catalog, schema, &tbl.name);
    let toggle = ExplorerMessage::Toggle(ExplorerTarget::Table {
        conn,
        catalog: catalog.to_string(),
        schema: schema.clone(),
        table: tbl.name.clone(),
    })
    .into();
    let mut col = column![table_row(
        3,
        conn,
        tbl.expanded,
        &tbl.name,
        &tbl.table_type,
        qualified,
        toggle
    )]
    .spacing(2);

    if tbl.expanded {
        if tbl.loading {
            col = col.push(status_row(4, "loading columns…"));
        } else if let Some(e) = &tbl.error {
            col = col.push(error_row(4, e));
        } else {
            for c in &tbl.columns {
                col = col.push(column_row(4, &c.name, &c.data_type));
            }
        }
    }
    col.into()
}

// -- Shared row builders ------------------------------------------------------

fn indent(depth: u16) -> Space {
    Space::new().width(Length::Fixed(depth as f32 * INDENT))
}

/// A collapsible branch row (file/connection/catalog/schema): triangle + label.
fn branch_row<'a>(
    depth: u16,
    expanded: bool,
    label: String,
    accent: Color,
    toggle: Message,
) -> Element<'a, Message> {
    let tri = if expanded { "▾" } else { "▸" };
    let content = row![
        indent(depth),
        text(tri).size(10).color(theme::palette::fg_muted()),
        Space::new().width(Length::Fixed(4.0)),
        theme::mono_sm(elide(&label, 30))
            .color(accent)
            .wrapping(Wrapping::None),
    ]
    .align_y(iced::Alignment::Center);
    button(content)
        .style(theme::ghost_button)
        .width(Length::Fill)
        .padding([2, 4])
        .on_press(toggle)
        .into()
}

/// A table row: triangle toggles columns, the name inserts a starter query
/// into an editor bound to this table's connection.
fn table_row<'a>(
    depth: u16,
    conn: usize,
    expanded: bool,
    name: &str,
    table_type: &str,
    qualified: String,
    toggle: Message,
) -> Element<'a, Message> {
    let tri = if expanded { "▾" } else { "▸" };
    let toggle_btn = button(text(tri).size(10).color(theme::palette::fg_muted()))
        .style(theme::ghost_button)
        .padding([2, 2])
        .on_press(toggle);
    let name_btn = button(
        theme::mono_sm(elide(name, 22))
            .color(theme::palette::fg_primary())
            .wrapping(Wrapping::None),
    )
    .style(theme::ghost_button)
    .width(Length::Fill)
    .padding([2, 4])
    .on_press(ExplorerMessage::InsertTable { conn, qualified }.into());
    let mut r = row![indent(depth), toggle_btn, name_btn].align_y(iced::Alignment::Center);
    if !table_type.is_empty() && !table_type.eq_ignore_ascii_case("table") {
        r = r.push(
            theme::mono_sm(elide(table_type, 8))
                .size(9)
                .style(muted)
                .wrapping(Wrapping::None),
        );
    }
    r.into()
}

/// A leaf column row: name + type.
fn column_row<'a>(depth: u16, name: &str, data_type: &str) -> Element<'a, Message> {
    row![
        indent(depth),
        text("·").size(10).color(theme::palette::fg_dim()),
        Space::new().width(Length::Fixed(4.0)),
        theme::mono_sm(elide(name, 24)).wrapping(Wrapping::None),
        Space::new().width(Length::Fixed(6.0)),
        theme::mono_sm(elide(data_type, 16))
            .size(10)
            .style(muted)
            .wrapping(Wrapping::None),
    ]
    .padding([1, 4])
    .align_y(iced::Alignment::Center)
    .into()
}

fn status_row<'a>(depth: u16, msg: &str) -> Element<'a, Message> {
    row![
        indent(depth),
        theme::mono_sm(msg.to_string()).size(10).style(muted)
    ]
    .padding([1, 4])
    .into()
}

fn error_row<'a>(depth: u16, msg: &str) -> Element<'a, Message> {
    row![
        indent(depth),
        text(elide_oneline(msg, 36))
            .size(10)
            .color(theme::palette::accent_rose())
            .wrapping(Wrapping::None),
    ]
    .padding([1, 4])
    .into()
}

// -- helpers ------------------------------------------------------------------

fn muted(_: &Theme) -> iced::widget::text::Style {
    iced::widget::text::Style {
        color: Some(theme::palette::fg_muted()),
    }
}

fn elide(s: &str, max: usize) -> String {
    if s.chars().count() <= max {
        return s.to_string();
    }
    let mut out: String = s.chars().take(max.saturating_sub(1)).collect();
    out.push('…');
    out
}

fn elide_oneline(s: &str, max: usize) -> String {
    let flat: String = s.split_whitespace().collect::<Vec<_>>().join(" ");
    elide(&flat, max)
}

fn sidebar_style(_theme: &Theme) -> ContainerStyle {
    ContainerStyle {
        background: Some(Background::Color(palette::bg_surface())),
        text_color: Some(palette::fg_primary()),
        border: Border {
            color: palette::border_subtle(),
            width: 1.0,
            radius: 0.0.into(),
        },
        ..ContainerStyle::default()
    }
}
