//! The FlightSQL connection info / health view: endpoint, a live ping, and the
//! server metadata returned by `GetSqlInfo`.

use iced::widget::{Space, button, column, container, row};
use iced::{Element, Length, Theme};

use crate::app::{ConnInfoMessage, ConnInfoState, Message};
use crate::flightsql::FlightSqlClient;
use crate::theme;

pub fn view<'a>(
    conn: usize,
    client: &'a FlightSqlClient,
    state: Option<&'a ConnInfoState>,
) -> Element<'a, Message> {
    let header = column![
        theme::display_strong(format!("flight: {}", client.label)),
        theme::mono_sm(client.url.clone()).style(muted),
    ]
    .spacing(4);

    column![header, ping_section(conn, state), info_section(state)]
        .spacing(20)
        .width(Length::Fill)
        .into()
}

fn ping_section<'a>(conn: usize, state: Option<&ConnInfoState>) -> Element<'a, Message> {
    let status: Element<'a, Message> = match state {
        Some(s) if s.pinging => theme::mono("pinging…").style(muted).into(),
        Some(s) => {
            if let Some(err) = &s.ping_error {
                theme::mono(format!("error: {err}"))
                    .color(theme::palette::accent_rose())
                    .into()
            } else if let Some(ms) = s.ping_ms {
                theme::mono(format!("{ms} ms"))
                    .color(theme::palette::accent_cool())
                    .into()
            } else {
                theme::mono("—").style(muted).into()
            }
        }
        None => theme::mono("—").style(muted).into(),
    };

    let ping_btn = button(theme::ui_medium("Ping").size(12))
        .style(theme::accent_button)
        .padding([6, 14])
        .on_press(ConnInfoMessage::Ping(conn).into());

    column![
        theme::label_text("Health"),
        row![ping_btn, Space::new().width(Length::Fixed(12.0)), status]
            .align_y(iced::Alignment::Center),
    ]
    .spacing(8)
    .into()
}

fn info_section<'a>(state: Option<&ConnInfoState>) -> Element<'a, Message> {
    let body: Element<'a, Message> = match state {
        Some(s) if s.info_loading => theme::mono_sm("loading server metadata…")
            .style(muted)
            .into(),
        Some(s) if s.info_error.is_some() => {
            theme::mono_sm(format!("error: {}", s.info_error.as_deref().unwrap_or("")))
                .color(theme::palette::accent_rose())
                .into()
        }
        Some(s) if !s.info.is_empty() => {
            let mut col = column![].spacing(6);
            for (k, v) in &s.info {
                col = col.push(
                    row![
                        container(theme::mono_sm(k.clone()).style(muted))
                            .width(Length::Fixed(150.0)),
                        theme::mono(v.clone()),
                    ]
                    .align_y(iced::Alignment::Center),
                );
            }
            container(col).padding(12).style(theme::surface_2).into()
        }
        Some(s) if s.info_loaded => {
            theme::mono_sm("Server reported no metadata (GetSqlInfo unsupported).")
                .style(muted)
                .into()
        }
        _ => theme::mono_sm("…").style(muted).into(),
    };

    column![theme::label_text("Server metadata"), body]
        .spacing(8)
        .into()
}

fn muted(_: &Theme) -> iced::widget::text::Style {
    iced::widget::text::Style {
        color: Some(theme::palette::fg_muted()),
    }
}
