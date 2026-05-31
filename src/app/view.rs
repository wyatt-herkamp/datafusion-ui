//! The `view` layer: the top-level [`App::view`] and its toolbar / content
//! routing helpers. Kept structurally identical to avoid resetting widget state.

use iced::widget::{Space, button, column, container, row, text};
use iced::{Element, Length};

use crate::theme;
use crate::views;

use super::helpers::{flight_pill_style, scroll_body, welcome_view};
use super::message::{FileMessage, FlightMessage, Message, SettingsMessage};
use super::state::{App, FileView, Selection};

impl App {
    pub fn view(&self) -> Element<'_, Message> {
        let toolbar = self.view_toolbar();
        let content = self.view_content();

        let mut top = column![toolbar];
        if let Some(err) = &self.error {
            top = top.push(
                container(text(format!("Error: {err}")).color(theme::palette::error())).padding(8),
            );
        }
        // Persistent global sidebar on the left; routed content on the right.
        let body = row![views::sidebar::view(self), content]
            .spacing(0)
            .height(Length::Fill);
        top = top.push(body);
        let base: Element<'_, Message> = top.spacing(0).into();

        // Connect modal overlays everything when open.
        if let Some(form) = &self.connect_form {
            iced::widget::stack![base, views::sql::connect_modal(form)].into()
        } else {
            base
        }
    }

    /// A slim, file-agnostic top bar: Open / Connect actions, connection pills,
    /// and the transient copy notice. Per-file metadata now lives in Overview
    /// and the sidebar; navigation is driven by the sidebar.
    fn view_toolbar(&self) -> Element<'_, Message> {
        let mut open_btn = button(theme::ui_medium("Open Parquet…").size(12))
            .style(theme::ghost_button)
            .padding([6, 12]);
        if self.loading {
            open_btn = button(theme::ui_medium("Loading…").size(12))
                .style(theme::ghost_button)
                .padding([6, 12]);
        } else {
            open_btn = open_btn.on_press(FileMessage::OpenFilePressed.into());
        }

        let connect_btn = button(theme::ui_medium("Connect FlightSQL…").size(12))
            .style(theme::ghost_button)
            .padding([6, 12])
            .on_press(FlightMessage::OpenConnectForm.into());

        let settings_btn = button(theme::ui_medium("⚙ Settings").size(12))
            .style(theme::ghost_button)
            .padding([6, 12])
            .on_press(SettingsMessage::Open.into());

        // One status pill per active FlightSQL connection (with disconnect).
        let mut conn_pills = row![].spacing(6).align_y(iced::Alignment::Center);
        for (i, c) in self.connections.iter().enumerate() {
            let pill = container(
                row![
                    theme::mono_sm(format!("● {}", c.label)).style(|_: &iced::Theme| {
                        iced::widget::text::Style {
                            color: Some(theme::palette::accent_cool()),
                        }
                    }),
                    button(text("✕").size(10))
                        .style(theme::ghost_button)
                        .padding([0, 4])
                        .on_press(FlightMessage::Disconnect(i).into()),
                ]
                .spacing(4)
                .align_y(iced::Alignment::Center),
            )
            .padding([2, 8])
            .style(flight_pill_style);
            conn_pills = conn_pills.push(pill);
        }

        let notice: Element<'_, Message> = match &self.copy_notice {
            Some(msg) => container(theme::mono_sm(msg.clone()))
                .padding([3, 10])
                .style(theme::notice_pill)
                .into(),
            None => Space::new().width(Length::Fixed(0.0)).into(),
        };

        let line = row![
            theme::label_text("DATAFUSION UI"),
            Space::new().width(Length::Fixed(14.0)),
            notice,
            Space::new().width(Length::Fill),
            conn_pills,
            Space::new().width(Length::Fixed(10.0)),
            connect_btn,
            Space::new().width(Length::Fixed(6.0)),
            open_btn,
            Space::new().width(Length::Fixed(6.0)),
            settings_btn,
        ]
        .align_y(iced::Alignment::Center)
        .spacing(0);

        container(container(line).padding([10, 18]))
            .width(Length::Fill)
            .style(theme::top_bar)
            .into()
    }

    /// Route the main content area on the current [`Selection`].
    fn view_content(&self) -> Element<'_, Message> {
        match self.selection {
            Selection::Sql => container(views::sql::view(self))
                .padding(12)
                .width(Length::Fill)
                .height(Length::Fill)
                .into(),
            Selection::File { id, view } => {
                let Some(ft) = self.file(id) else {
                    return welcome_view(&self.recent_files);
                };
                match view {
                    FileView::Overview => {
                        scroll_body(views::overview::view(&ft.summary, &ft.expanded_schema_rows))
                    }
                    FileView::RowGroups => {
                        scroll_body(views::row_groups::view(&ft.summary, ft.selected_row_group))
                    }
                }
            }
            Selection::ConnInfo { conn } => match self.connections.get(conn) {
                Some(client) => scroll_body(views::conn_info::view(
                    conn,
                    client,
                    self.conn_info.get(conn),
                )),
                None => welcome_view(&self.recent_files),
            },
            Selection::Settings => {
                let draft = self.settings_draft.as_ref().unwrap_or(&self.config);
                scroll_body(views::settings::view(draft))
            }
            Selection::Welcome => welcome_view(&self.recent_files),
        }
    }
}
