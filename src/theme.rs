//! "Editorial data instrument" theme — custom fonts and shared style helpers.
//!
//! The palette is no longer a fixed set of constants: it is a runtime value
//! (see [`palette::Palette`]) selected by a global "active palette" so the whole
//! UI can switch between the Instrument (dark), Light, and One Dark themes at
//! runtime. Style helpers read [`palette::active`] at draw time, so they react
//! to a theme change on the next frame.
//!
//! Some palette tokens and helpers are exposed for downstream use even if not
//! consumed yet (e.g. `surface_2`, `accent_button`, `FONT_MONO_MEDIUM`).
#![allow(dead_code)]

use iced::font::{Family, Weight};
use iced::widget::{button, container, text};
use iced::{Background, Border, Color, Font, Theme};

use crate::wrangle::insights::ColumnKind;

// -- Fonts --------------------------------------------------------------------

pub const GEIST_REGULAR_BYTES: &[u8] = include_bytes!("../assets/fonts/Geist-Regular.ttf");
pub const GEIST_MEDIUM_BYTES: &[u8] = include_bytes!("../assets/fonts/Geist-Medium.ttf");
pub const GEIST_SEMIBOLD_BYTES: &[u8] = include_bytes!("../assets/fonts/Geist-SemiBold.ttf");
pub const JETBRAINS_MONO_REGULAR_BYTES: &[u8] =
    include_bytes!("../assets/fonts/JetBrainsMono-Regular.ttf");
pub const JETBRAINS_MONO_MEDIUM_BYTES: &[u8] =
    include_bytes!("../assets/fonts/JetBrainsMono-Medium.ttf");

pub const FONT_UI: Font = Font {
    family: Family::Name("Geist"),
    weight: Weight::Normal,
    ..Font::DEFAULT
};

pub const FONT_UI_MEDIUM: Font = Font {
    family: Family::Name("Geist"),
    weight: Weight::Medium,
    ..Font::DEFAULT
};

pub const FONT_UI_SEMIBOLD: Font = Font {
    family: Family::Name("Geist"),
    weight: Weight::Semibold,
    ..Font::DEFAULT
};

pub const FONT_MONO: Font = Font {
    family: Family::Name("JetBrains Mono"),
    weight: Weight::Normal,
    ..Font::DEFAULT
};

pub const FONT_MONO_MEDIUM: Font = Font {
    family: Family::Name("JetBrains Mono"),
    weight: Weight::Medium,
    ..Font::DEFAULT
};

// -- Palette ------------------------------------------------------------------

pub mod palette {
    use iced::Color;
    use std::sync::atomic::{AtomicU8, Ordering};

    const fn rgb(r: u8, g: u8, b: u8) -> Color {
        Color {
            r: r as f32 / 255.0,
            g: g as f32 / 255.0,
            b: b as f32 / 255.0,
            a: 1.0,
        }
    }
    const fn rgba(r: u8, g: u8, b: u8, a: f32) -> Color {
        Color {
            r: r as f32 / 255.0,
            g: g as f32 / 255.0,
            b: b as f32 / 255.0,
            a,
        }
    }

    /// The full set of semantic color tokens for one theme. All UI styling is
    /// resolved against the currently [`active`] palette.
    #[derive(Debug, Clone, Copy)]
    pub struct Palette {
        pub bg_deep: Color,
        pub bg_surface: Color,
        pub bg_surface_2: Color,
        pub bg_hover: Color,
        pub border_subtle: Color,
        pub border_strong: Color,
        pub fg_primary: Color,
        pub fg_muted: Color,
        pub fg_dim: Color,
        pub accent_warm: Color,
        pub accent_warm_soft: Color,
        pub accent_cool: Color,
        pub accent_cool_soft: Color,
        pub accent_violet: Color,
        pub accent_violet_soft: Color,
        pub accent_rose: Color,
        pub diff_changed_bg: Color,
        pub diff_changed_bar: Color,
        pub backdrop_dim: Color,
        pub error: Color,
        pub warning: Color,
    }

    /// Signature dark theme. Warm-orange + teal + violet accents on near-black.
    pub const DARK: Palette = Palette {
        bg_deep: rgb(0x0E, 0x0E, 0x10),
        bg_surface: rgb(0x17, 0x17, 0x1A),
        bg_surface_2: rgb(0x1F, 0x1F, 0x23),
        bg_hover: rgb(0x22, 0x22, 0x27),
        border_subtle: rgb(0x27, 0x27, 0x2D),
        border_strong: rgb(0x3A, 0x3A, 0x42),
        fg_primary: rgb(0xE8, 0xE6, 0xE0),
        fg_muted: rgb(0x8A, 0x8A, 0x93),
        fg_dim: rgb(0x5A, 0x5A, 0x63),
        accent_warm: rgb(0xE6, 0xA3, 0x3A),
        accent_warm_soft: rgba(0xE6, 0xA3, 0x3A, 0.18),
        accent_cool: rgb(0x3F, 0xB6, 0xA8),
        accent_cool_soft: rgba(0x3F, 0xB6, 0xA8, 0.16),
        accent_violet: rgb(0x9B, 0x8C, 0xF7),
        accent_violet_soft: rgba(0x9B, 0x8C, 0xF7, 0.16),
        accent_rose: rgb(0xE3, 0x64, 0x64),
        diff_changed_bg: rgba(0xE6, 0xA3, 0x3A, 0.14),
        diff_changed_bar: rgb(0xE6, 0xA3, 0x3A),
        backdrop_dim: rgba(0, 0, 0, 0.55),
        error: rgb(0xE5, 0x4D, 0x4D),
        warning: rgb(0xE6, 0x80, 0x4D),
    };

    /// One Dark (Mark Skelton / Atom One Dark) ported into our token set.
    pub const ONE_DARK: Palette = Palette {
        bg_deep: rgb(0x21, 0x25, 0x2B),
        bg_surface: rgb(0x28, 0x2C, 0x34),
        bg_surface_2: rgb(0x2C, 0x31, 0x3A),
        bg_hover: rgb(0x2C, 0x31, 0x3C),
        border_subtle: rgb(0x3B, 0x40, 0x48),
        border_strong: rgb(0x4B, 0x52, 0x63),
        fg_primary: rgb(0xAB, 0xB2, 0xBF),
        fg_muted: rgb(0x82, 0x89, 0x97),
        fg_dim: rgb(0x5C, 0x63, 0x70),
        accent_warm: rgb(0xD1, 0x9A, 0x66),
        accent_warm_soft: rgba(0xD1, 0x9A, 0x66, 0.18),
        accent_cool: rgb(0x56, 0xB6, 0xC2),
        accent_cool_soft: rgba(0x56, 0xB6, 0xC2, 0.16),
        accent_violet: rgb(0xC6, 0x78, 0xDD),
        accent_violet_soft: rgba(0xC6, 0x78, 0xDD, 0.16),
        accent_rose: rgb(0xE0, 0x6C, 0x75),
        diff_changed_bg: rgba(0xD1, 0x9A, 0x66, 0.14),
        diff_changed_bar: rgb(0xD1, 0x9A, 0x66),
        backdrop_dim: rgba(0, 0, 0, 0.55),
        error: rgb(0xE0, 0x6C, 0x75),
        warning: rgb(0xD1, 0x9A, 0x66),
    };

    /// Light theme. Paper/white surfaces; keeps the app's accent identity.
    pub const LIGHT: Palette = Palette {
        bg_deep: rgb(0xEC, 0xEC, 0xEE),
        bg_surface: rgb(0xFF, 0xFF, 0xFF),
        bg_surface_2: rgb(0xF4, 0xF4, 0xF6),
        bg_hover: rgb(0xE9, 0xE9, 0xEC),
        border_subtle: rgb(0xE2, 0xE2, 0xE6),
        border_strong: rgb(0xCF, 0xCF, 0xD6),
        fg_primary: rgb(0x1C, 0x1C, 0x1F),
        fg_muted: rgb(0x6A, 0x6A, 0x73),
        fg_dim: rgb(0x9A, 0x9A, 0xA3),
        accent_warm: rgb(0xC8, 0x86, 0x1E),
        accent_warm_soft: rgba(0xE6, 0xA3, 0x3A, 0.20),
        accent_cool: rgb(0x1F, 0x8A, 0x7E),
        accent_cool_soft: rgba(0x3F, 0xB6, 0xA8, 0.18),
        accent_violet: rgb(0x6E, 0x5C, 0xD8),
        accent_violet_soft: rgba(0x9B, 0x8C, 0xF7, 0.18),
        accent_rose: rgb(0xC8, 0x3D, 0x3D),
        diff_changed_bg: rgba(0xE6, 0xA3, 0x3A, 0.16),
        diff_changed_bar: rgb(0xC8, 0x86, 0x1E),
        backdrop_dim: rgba(0, 0, 0, 0.35),
        error: rgb(0xC0, 0x39, 0x2B),
        warning: rgb(0xB7, 0x79, 0x1F),
    };

    /// Which palette is currently active. Mirrors `config::ThemeChoice` minus
    /// the `System` variant (which resolves to `Dark` or `Light` at boot).
    #[derive(Debug, Clone, Copy, PartialEq, Eq)]
    pub enum PaletteKind {
        Dark = 0,
        Light = 1,
        OneDark = 2,
    }

    static ACTIVE: AtomicU8 = AtomicU8::new(PaletteKind::Dark as u8);

    /// Set the globally active palette. Cheap; called from the theme callback
    /// every frame and from the settings update handler on a live switch.
    pub fn set_active(kind: PaletteKind) {
        ACTIVE.store(kind as u8, Ordering::Relaxed);
    }

    /// The currently active palette. Style helpers read this at draw time.
    pub fn active() -> &'static Palette {
        match ACTIVE.load(Ordering::Relaxed) {
            x if x == PaletteKind::Light as u8 => &LIGHT,
            x if x == PaletteKind::OneDark as u8 => &ONE_DARK,
            _ => &DARK,
        }
    }

    // Per-token accessors over the active palette. These keep call sites terse
    // (`palette::bg_surface()`) and resolve the live theme each time.
    macro_rules! tokens {
        ($($name:ident),* $(,)?) => {
            $(pub fn $name() -> Color { active().$name })*
        };
    }
    tokens!(
        bg_deep,
        bg_surface,
        bg_surface_2,
        bg_hover,
        border_subtle,
        border_strong,
        fg_primary,
        fg_muted,
        fg_dim,
        accent_warm,
        accent_warm_soft,
        accent_cool,
        accent_cool_soft,
        accent_violet,
        accent_violet_soft,
        accent_rose,
        diff_changed_bg,
        diff_changed_bar,
        backdrop_dim,
        error,
        warning,
    );
}

// -- Theme --------------------------------------------------------------------

fn theme_from(p: &palette::Palette, name: &str) -> Theme {
    Theme::custom(
        name.to_string(),
        iced::theme::Palette {
            background: p.bg_deep,
            text: p.fg_primary,
            primary: p.accent_warm,
            success: p.accent_cool,
            warning: p.accent_warm,
            danger: p.accent_rose,
        },
    )
}

pub fn dark_theme() -> Theme {
    theme_from(&palette::DARK, "Dark")
}

pub fn light_theme() -> Theme {
    theme_from(&palette::LIGHT, "Light")
}

pub fn one_dark_theme() -> Theme {
    theme_from(&palette::ONE_DARK, "One Dark")
}

// -- Text helpers -------------------------------------------------------------

pub fn ui<'a>(s: impl text::IntoFragment<'a>) -> text::Text<'a> {
    text(s).font(FONT_UI).size(13)
}

pub fn ui_medium<'a>(s: impl text::IntoFragment<'a>) -> text::Text<'a> {
    text(s).font(FONT_UI_MEDIUM).size(13)
}

pub fn mono<'a>(s: impl text::IntoFragment<'a>) -> text::Text<'a> {
    text(s).font(FONT_MONO).size(12)
}

pub fn mono_sm<'a>(s: impl text::IntoFragment<'a>) -> text::Text<'a> {
    text(s).font(FONT_MONO).size(11)
}

pub fn display_strong<'a>(s: impl text::IntoFragment<'a>) -> text::Text<'a> {
    text(s).font(FONT_UI_SEMIBOLD).size(18)
}

/// Small caps label: uppercases + dim color + tight tracking-ish via wider size↓.
pub fn label_text(s: &str) -> text::Text<'static> {
    text(s.to_ascii_uppercase())
        .font(FONT_UI_MEDIUM)
        .size(10)
        .style(|_: &Theme| text::Style {
            color: Some(palette::fg_muted()),
        })
}

pub fn muted<'a, T: text::IntoFragment<'a>>(t: text::Text<'a>) -> text::Text<'a> {
    let _ = std::marker::PhantomData::<T>;
    t.style(|_: &Theme| text::Style {
        color: Some(palette::fg_muted()),
    })
}

// -- Container styles ---------------------------------------------------------

pub fn surface(_: &Theme) -> container::Style {
    container::Style {
        background: Some(Background::Color(palette::bg_surface())),
        text_color: Some(palette::fg_primary()),
        border: Border {
            color: palette::border_subtle(),
            width: 1.0,
            radius: 0.0.into(),
        },
        ..container::Style::default()
    }
}

pub fn surface_2(_: &Theme) -> container::Style {
    container::Style {
        background: Some(Background::Color(palette::bg_surface_2())),
        text_color: Some(palette::fg_primary()),
        border: Border {
            color: palette::border_strong(),
            width: 1.0,
            radius: 4.0.into(),
        },
        ..container::Style::default()
    }
}

pub fn backdrop(_: &Theme) -> container::Style {
    container::Style {
        background: Some(Background::Color(palette::backdrop_dim())),
        ..container::Style::default()
    }
}

pub fn top_bar(_: &Theme) -> container::Style {
    container::Style {
        background: Some(Background::Color(palette::bg_surface())),
        text_color: Some(palette::fg_primary()),
        border: Border {
            color: palette::border_subtle(),
            width: 0.0,
            radius: 0.0.into(),
        },
        ..container::Style::default()
    }
}

pub fn top_bar_divider(_: &Theme) -> container::Style {
    container::Style {
        background: Some(Background::Color(palette::border_subtle())),
        ..container::Style::default()
    }
}

pub fn tab_underline(_: &Theme) -> container::Style {
    container::Style {
        background: Some(Background::Color(palette::accent_warm())),
        ..container::Style::default()
    }
}

pub fn notice_pill(_: &Theme) -> container::Style {
    container::Style {
        background: Some(Background::Color(palette::accent_warm_soft())),
        text_color: Some(palette::accent_warm()),
        border: Border {
            color: palette::accent_warm(),
            width: 1.0,
            radius: 999.0.into(),
        },
        ..container::Style::default()
    }
}

// -- Button styles ------------------------------------------------------------

pub fn ghost_button(_: &Theme, status: button::Status) -> button::Style {
    let (bg, fg) = match status {
        button::Status::Active => (Color::TRANSPARENT, palette::fg_muted()),
        button::Status::Hovered => (palette::bg_hover(), palette::fg_primary()),
        button::Status::Pressed => (palette::bg_surface_2(), palette::accent_warm()),
        button::Status::Disabled => (Color::TRANSPARENT, palette::fg_dim()),
    };
    button::Style {
        background: Some(Background::Color(bg)),
        text_color: fg,
        border: Border {
            color: palette::border_subtle(),
            width: 0.0,
            radius: 4.0.into(),
        },
        ..button::Style::default()
    }
}

/// Nudge each RGB channel toward white (`amt > 0`) or black (`amt < 0`).
fn shade(c: Color, amt: f32) -> Color {
    Color {
        r: (c.r + amt).clamp(0.0, 1.0),
        g: (c.g + amt).clamp(0.0, 1.0),
        b: (c.b + amt).clamp(0.0, 1.0),
        a: c.a,
    }
}

pub fn accent_button(_: &Theme, status: button::Status) -> button::Style {
    let accent = palette::accent_warm();
    let bg = match status {
        button::Status::Hovered => shade(accent, 0.10),
        button::Status::Pressed => shade(accent, -0.12),
        button::Status::Disabled => Color { a: 0.4, ..accent },
        _ => accent,
    };
    button::Style {
        background: Some(Background::Color(bg)),
        text_color: on_color(accent),
        border: Border {
            color: Color::TRANSPARENT,
            width: 0.0,
            radius: 4.0.into(),
        },
        ..button::Style::default()
    }
}

/// Pick a readable foreground (near-black or near-white) for text drawn on top
/// of `bg`, based on its perceived luminance.
fn on_color(bg: Color) -> Color {
    let luminance = 0.299 * bg.r + 0.587 * bg.g + 0.114 * bg.b;
    if luminance > 0.6 {
        Color::from_rgb(0.07, 0.07, 0.08)
    } else {
        Color::from_rgb(0.96, 0.96, 0.94)
    }
}

pub fn tab_button(active: bool) -> impl Fn(&Theme, button::Status) -> button::Style + Copy {
    move |_: &Theme, status: button::Status| {
        let fg = if active {
            palette::fg_primary()
        } else {
            match status {
                button::Status::Hovered | button::Status::Pressed => palette::fg_primary(),
                _ => palette::fg_muted(),
            }
        };
        button::Style {
            background: Some(Background::Color(Color::TRANSPARENT)),
            text_color: fg,
            border: Border {
                color: Color::TRANSPARENT,
                width: 0.0,
                radius: 0.0.into(),
            },
            ..button::Style::default()
        }
    }
}

// -- Type-pill colors ---------------------------------------------------------

#[derive(Debug, Clone, Copy)]
pub struct PillColors {
    pub bg: Color,
    pub fg: Color,
}

pub fn pill_colors_for(kind: ColumnKind) -> PillColors {
    match kind {
        ColumnKind::Numeric => PillColors {
            bg: palette::accent_cool_soft(),
            fg: palette::accent_cool(),
        },
        ColumnKind::Temporal => PillColors {
            bg: palette::accent_cool_soft(),
            fg: palette::accent_cool(),
        },
        ColumnKind::String => PillColors {
            bg: Color {
                a: 0.10,
                ..palette::fg_primary()
            },
            fg: palette::fg_primary(),
        },
        ColumnKind::Boolean => PillColors {
            bg: palette::accent_warm_soft(),
            fg: palette::accent_warm(),
        },
        ColumnKind::Other => PillColors {
            bg: palette::accent_violet_soft(),
            fg: palette::accent_violet(),
        },
    }
}

/// Heuristic: is the data type nested (overrides ColumnKind for pill color)?
pub fn pill_colors_nested() -> PillColors {
    PillColors {
        bg: palette::accent_violet_soft(),
        fg: palette::accent_violet(),
    }
}

pub fn pill_style(colors: PillColors) -> impl Fn(&Theme) -> container::Style + Copy {
    move |_: &Theme| container::Style {
        background: Some(Background::Color(colors.bg)),
        text_color: Some(colors.fg),
        border: Border {
            color: Color::TRANSPARENT,
            width: 0.0,
            radius: 3.0.into(),
        },
        ..container::Style::default()
    }
}
