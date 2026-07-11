use chrono::NaiveDate;

use crate::calendar::{CalendarEvent, EventCategory};

const DAY_LABEL_FORMAT: &str = "%a %b %-d";
const TIME_LABEL_FORMAT: &str = "%H:%M";
const ANSI_RESET: &str = "\u{1b}[0m";
const ANSI_BOLD: &str = "1";
const ANSI_ITALIC: &str = "3";

pub const FG: RgbColor = RgbColor::new(218, 218, 218);
pub const MUTED: RgbColor = RgbColor::new(120, 126, 135);
pub const DIM: RgbColor = RgbColor::new(88, 93, 101);
pub const ACCENT: RgbColor = RgbColor::new(122, 162, 247);
pub const TODAY: RgbColor = RgbColor::new(180, 220, 140);
pub const HOLIDAY: RgbColor = RgbColor::new(245, 194, 129);
pub const BIRTHDAY: RgbColor = RgbColor::new(203, 166, 247);
pub const TRAVEL: RgbColor = RgbColor::new(137, 180, 250);
pub const FOCUS: RgbColor = RgbColor::new(166, 227, 161);
pub const OUT_OF_OFFICE: RgbColor = RgbColor::new(243, 139, 168);
pub const MEETING: RgbColor = RgbColor::new(122, 162, 247);
pub const ALL_DAY: RgbColor = RgbColor::new(186, 194, 222);

#[derive(Debug, Clone, Copy, PartialEq, Eq, clap::ValueEnum)]
#[value(rename_all = "kebab-case")]
pub enum Theme {
    Default,
    Evangelion,
    Nerv,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Palette {
    pub foreground: RgbColor,
    pub muted: RgbColor,
    pub dim: RgbColor,
    pub accent: RgbColor,
    pub today: RgbColor,
    pub holiday: RgbColor,
    pub birthday: RgbColor,
    pub travel: RgbColor,
    pub focus: RgbColor,
    pub out_of_office: RgbColor,
    pub meeting: RgbColor,
    pub all_day: RgbColor,
}

impl Theme {
    pub const fn palette(self) -> Palette {
        match self {
            Self::Default => Palette::DEFAULT,
            Self::Evangelion => Palette::EVANGELION,
            Self::Nerv => Palette::NERV,
        }
    }

    pub fn parse_name(value: &str) -> Option<Self> {
        match value.trim().to_ascii_lowercase().as_str() {
            "default" => Some(Self::Default),
            "evangelion" => Some(Self::Evangelion),
            "nerv" => Some(Self::Nerv),
            _ => None,
        }
    }
}

impl std::fmt::Display for Theme {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter.write_str(match self {
            Self::Default => "default",
            Self::Evangelion => "evangelion",
            Self::Nerv => "nerv",
        })
    }
}

impl Palette {
    pub const DEFAULT: Self = Self {
        foreground: FG,
        muted: MUTED,
        dim: DIM,
        accent: ACCENT,
        today: TODAY,
        holiday: HOLIDAY,
        birthday: BIRTHDAY,
        travel: TRAVEL,
        focus: FOCUS,
        out_of_office: OUT_OF_OFFICE,
        meeting: MEETING,
        all_day: ALL_DAY,
    };

    pub const EVANGELION: Self = Self {
        foreground: RgbColor::new(215, 95, 255),
        muted: RgbColor::new(255, 135, 0),
        dim: RgbColor::new(135, 135, 175),
        accent: RgbColor::new(135, 255, 0),
        today: RgbColor::new(135, 255, 0),
        holiday: RgbColor::new(255, 135, 0),
        birthday: RgbColor::new(215, 95, 255),
        travel: RgbColor::new(135, 95, 255),
        focus: RgbColor::new(135, 255, 0),
        out_of_office: RgbColor::new(255, 135, 0),
        meeting: RgbColor::new(135, 95, 255),
        all_day: RgbColor::new(135, 135, 175),
    };

    pub const NERV: Self = Self {
        foreground: RgbColor::new(255, 175, 0),
        muted: RgbColor::new(215, 135, 0),
        dim: RgbColor::new(102, 102, 102),
        accent: RgbColor::new(0, 255, 135),
        today: RgbColor::new(0, 255, 135),
        holiday: RgbColor::new(255, 135, 0),
        birthday: RgbColor::new(255, 175, 0),
        travel: RgbColor::new(215, 135, 0),
        focus: RgbColor::new(0, 255, 135),
        out_of_office: RgbColor::new(255, 0, 0),
        meeting: RgbColor::new(255, 135, 0),
        all_day: RgbColor::new(102, 102, 102),
    };

    pub const fn category_color(self, category: EventCategory) -> RgbColor {
        match category {
            EventCategory::Holiday => self.holiday,
            EventCategory::Birthday => self.birthday,
            EventCategory::Travel => self.travel,
            EventCategory::Focus => self.focus,
            EventCategory::OutOfOffice => self.out_of_office,
            EventCategory::Meeting => self.meeting,
            EventCategory::AllDay => self.all_day,
            EventCategory::Other => self.foreground,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct RgbColor {
    pub red: u8,
    pub green: u8,
    pub blue: u8,
}

impl RgbColor {
    pub const fn new(red: u8, green: u8, blue: u8) -> Self {
        Self { red, green, blue }
    }
}

pub fn day_label(date: NaiveDate, today: NaiveDate) -> String {
    if date == today {
        "Today".to_string()
    } else if Some(date) == today.succ_opt() {
        "Tomorrow".to_string()
    } else {
        date.format(DAY_LABEL_FORMAT).to_string()
    }
}

pub fn compact_day_label(date: NaiveDate) -> String {
    date.format(DAY_LABEL_FORMAT).to_string()
}

pub fn time_label(event: &CalendarEvent) -> String {
    if event.all_day {
        if event.is_multi_day() {
            "multi".to_string()
        } else {
            "all-day".to_string()
        }
    } else {
        event.start.format(TIME_LABEL_FORMAT).to_string()
    }
}

pub fn format_duration(minutes: i64) -> String {
    if minutes < 60 {
        format!("{minutes}m")
    } else {
        let hours = minutes / 60;
        let mins = minutes % 60;
        if mins == 0 {
            format!("{hours}h")
        } else {
            format!("{hours}h{mins:02}")
        }
    }
}

pub fn stdout_colors_enabled(disabled_by_cli: bool) -> bool {
    !disabled_by_cli && std::env::var_os("NO_COLOR").is_none()
}

pub fn colorize_day_label(
    label: &str,
    date: NaiveDate,
    today: NaiveDate,
    enabled: bool,
    palette: Palette,
) -> String {
    let color = if date == today {
        palette.today
    } else {
        palette.accent
    };
    ansi_color(label, color, true, false, enabled)
}

pub fn colorize_time_label(
    label: &str,
    category: EventCategory,
    enabled: bool,
    palette: Palette,
) -> String {
    let color = match category {
        EventCategory::Other => palette.muted,
        _ => palette.category_color(category),
    };
    ansi_color(label, color, false, false, enabled)
}

pub fn colorize_title(
    label: &str,
    category: EventCategory,
    enabled: bool,
    palette: Palette,
) -> String {
    ansi_color(
        label,
        palette.category_color(category),
        matches!(category, EventCategory::Holiday),
        matches!(category, EventCategory::OutOfOffice),
        enabled,
    )
}

pub fn colorize_details(label: &str, enabled: bool, palette: Palette) -> String {
    ansi_color(label, palette.dim, false, false, enabled)
}

fn ansi_color(label: &str, color: RgbColor, bold: bool, italic: bool, enabled: bool) -> String {
    if !enabled {
        return label.to_string();
    }

    let mut codes = Vec::new();
    if bold {
        codes.push(ANSI_BOLD.to_string());
    }
    if italic {
        codes.push(ANSI_ITALIC.to_string());
    }
    codes.push(format!("38;2;{};{};{}", color.red, color.green, color.blue));

    format!("\u{1b}[{}m{label}{ANSI_RESET}", codes.join(";"))
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::Mutex;

    static ENV_LOCK: Mutex<()> = Mutex::new(());

    #[test]
    fn day_label_uses_relative_names_for_today_and_tomorrow() {
        let today = NaiveDate::from_ymd_opt(2026, 6, 7).expect("valid date");

        assert_eq!(day_label(today, today), "Today");
        assert_eq!(
            day_label(today.succ_opt().expect("next day"), today),
            "Tomorrow"
        );
        assert_eq!(
            day_label(
                NaiveDate::from_ymd_opt(2026, 6, 9).expect("valid date"),
                today
            ),
            "Tue Jun 9"
        );
    }

    #[test]
    fn format_duration_compacts_hours_and_minutes() {
        assert_eq!(format_duration(45), "45m");
        assert_eq!(format_duration(60), "1h");
        assert_eq!(format_duration(95), "1h35");
    }

    #[test]
    fn colorize_title_can_emit_or_suppress_ansi() {
        let colored = colorize_title("Holiday", EventCategory::Holiday, true, Palette::DEFAULT);

        assert!(colored.starts_with("\u{1b}[1;38;2;245;194;129m"));
        assert!(colored.ends_with(ANSI_RESET));
        assert_eq!(
            colorize_title("Holiday", EventCategory::Holiday, false, Palette::DEFAULT),
            "Holiday"
        );
    }

    #[test]
    fn no_color_env_disables_stdout_colors() {
        let _guard = ENV_LOCK.lock().expect("env lock");
        let previous = std::env::var_os("NO_COLOR");

        unsafe {
            std::env::set_var("NO_COLOR", "1");
        }
        assert!(!stdout_colors_enabled(false));

        unsafe {
            if let Some(previous) = previous {
                std::env::set_var("NO_COLOR", previous);
            } else {
                std::env::remove_var("NO_COLOR");
            }
        }
    }

    #[test]
    fn named_theme_palettes_use_expected_key_colors() {
        assert_eq!(Theme::Default.palette().holiday, HOLIDAY);
        assert_eq!(
            Theme::Evangelion.palette().foreground,
            RgbColor::new(215, 95, 255)
        );
        assert_eq!(
            Theme::Evangelion.palette().accent,
            RgbColor::new(135, 255, 0)
        );
        assert_eq!(Theme::Nerv.palette().foreground, RgbColor::new(255, 175, 0));
        assert_eq!(
            Theme::Nerv.palette().out_of_office,
            RgbColor::new(255, 0, 0)
        );
    }
}
