use crate::highlight::{self, Colour};
use ratatui::style::{Color, Style};
use std::sync::OnceLock;
use terminal_colorsaurus::{QueryOptions, ThemeMode, color_palette};
use tui_markdown::CodeTheme;

#[derive(Clone, Copy)]
pub(in crate::tui) enum Theme {
    Light,
    Dark,
}

impl Theme {
    pub(in crate::tui) fn detect() -> (Self, Option<Colour>) {
        match color_palette(QueryOptions::default()) {
            Ok(palette) => (
                match palette.theme_mode() {
                    ThemeMode::Light => Self::Light,
                    ThemeMode::Dark => Self::Dark,
                },
                Some(palette.background.scale_to_8bit()),
            ),
            Err(_) => (
                std::env::var("COLORFGBG")
                    .ok()
                    .and_then(|value| Self::from_colorfgbg(&value))
                    .unwrap_or(Self::Light),
                None,
            ),
        }
    }

    pub(super) fn tool_style(self, background: Option<Colour>) -> Style {
        let shift = |channel: u8| match self {
            Self::Light => channel.saturating_sub(12),
            Self::Dark => channel.saturating_add(12),
        };
        let (red, green, blue) = background.unwrap_or(match self {
            Self::Light => (255, 255, 255),
            Self::Dark => (23, 23, 23),
        });
        Style::new().bg(rgb((shift(red), shift(green), shift(blue))))
    }

    pub(super) fn from_colorfgbg(value: &str) -> Option<Self> {
        match value.rsplit(';').next()?.parse::<u8>().ok()? {
            0..=6 | 8 => Some(Self::Dark),
            7 | 9..=15 => Some(Self::Light),
            _ => None,
        }
    }

    pub(super) fn highlight_style(self) -> Style {
        match self {
            Self::Light => Style::new()
                .fg(Color::Rgb(62, 62, 62))
                .bg(Color::Rgb(230, 230, 230)),
            Self::Dark => Style::new()
                .fg(Color::Rgb(220, 220, 220))
                .bg(Color::Rgb(50, 50, 50)),
        }
    }

    pub(in crate::tui) fn subtle_style(self) -> Style {
        self.tool_style(BACKGROUND.get().copied().flatten())
    }

    pub(super) fn code_theme(self) -> CodeTheme {
        static DARK: OnceLock<CodeTheme> = OnceLock::new();
        static LIGHT: OnceLock<CodeTheme> = OnceLock::new();
        let load = |source| CodeTheme::from_textmate(source).expect("bundled theme");
        match self {
            Self::Light => LIGHT.get_or_init(|| load(highlight::LIGHT_THEME)),
            Self::Dark => DARK.get_or_init(|| load(highlight::DARK_THEME)),
        }
        .clone()
    }
}

pub(in crate::tui) static THEME: OnceLock<Theme> = OnceLock::new();
pub(in crate::tui) static BACKGROUND: OnceLock<Option<Colour>> = OnceLock::new();

pub(in crate::tui) fn theme() -> Theme {
    THEME.get().copied().unwrap_or(Theme::Light)
}

pub(super) const fn rgb((red, green, blue): Colour) -> Color {
    Color::Rgb(red, green, blue)
}
