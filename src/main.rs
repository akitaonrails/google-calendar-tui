mod calendar;
mod display;
mod ics;
mod ui;

use anyhow::{Context, Result};
use calendar::CalendarEvent;
use chrono::{Local, NaiveDate};
use clap::Parser;
use display::{
    colorize_day_label, colorize_details, colorize_time_label, colorize_title, day_label,
    format_duration, stdout_colors_enabled, time_label,
};

/// Default lookahead chosen to keep startup fast while still covering near-term planning.
const DEFAULT_FETCH_DAYS: i64 = 60;

#[derive(Debug, Parser)]
#[command(
    name = "google-calendar-tui",
    about = "A quiet, read-only terminal agenda for Google Calendar via GNOME Online Accounts",
    version
)]
struct Cli {
    /// GNOME Online Accounts filter. Repeat or comma-separate; matches id, email, or display name.
    #[arg(short, long, value_delimiter = ',')]
    account: Vec<String>,

    /// List usable Google Calendar accounts from GNOME Online Accounts and exit.
    #[arg(long)]
    list_accounts: bool,

    /// Include calendars hidden/unselected in Google Calendar.
    #[arg(long)]
    all_calendars: bool,

    /// Fetch events from a private ICS/iCal URL instead of GNOME Online Accounts. Repeat for multiple calendars.
    #[arg(long, value_name = "URL", conflicts_with_all = ["list_accounts", "account", "all_calendars"])]
    ics: Vec<String>,

    /// Show extra columns like duration, video indicator, account, and calendar name.
    #[arg(long)]
    details: bool,

    /// Use the interactive screen-fitting TUI with the `more` command.
    #[arg(long)]
    tui: bool,

    /// Disable ANSI colors in plain stdout output. Also disabled by NO_COLOR.
    #[arg(long)]
    no_color: bool,

    /// Number of future days to fetch once at startup.
    #[arg(long, default_value_t = DEFAULT_FETCH_DAYS, value_name = "DAYS")]
    fetch_days: i64,

    /// Maximum events to request per calendar page. Google Calendar caps this at 2500.
    #[arg(long, default_value_t = calendar::GOOGLE_EVENTS_MAX_RESULTS_PER_PAGE, value_name = "N")]
    max_results_per_calendar: u32,
}

#[tokio::main]
async fn main() -> Result<()> {
    let cli = Cli::parse();
    let account_filters = cli
        .account
        .iter()
        .map(|account| account.trim())
        .filter(|account| !account.is_empty())
        .map(ToOwned::to_owned)
        .collect::<Vec<_>>();

    if cli.list_accounts {
        let accounts =
            calendar::select_accounts(calendar::available_accounts().await?, &account_filters)?;
        if accounts.is_empty() {
            println!(
                "No Google Calendar account candidates found in GNOME Online Accounts. Add or fix one in GNOME Settings > Online Accounts."
            );
        } else {
            for account in accounts {
                println!(
                    "{}\t{}\t{}",
                    account.label(),
                    calendar::sanitize_display_text(&account.id),
                    account.path()
                );
            }
        }
        return Ok(());
    }

    let fetched_at = Local::now();
    let fetch_days = cli.fetch_days.max(1);
    let events = if cli.ics.is_empty() {
        calendar::fetch_accounts(calendar::FetchOptions {
            account_filters: &account_filters,
            fetch_days,
            max_results_per_calendar: cli
                .max_results_per_calendar
                .clamp(1, calendar::GOOGLE_EVENTS_MAX_RESULTS_PER_PAGE),
            all_calendars: cli.all_calendars,
        })
        .await
        .context("calendar fetch failed")?
    } else {
        ics::fetch_sources(&cli.ics, fetch_days)
            .await
            .context("ICS fetch failed")?
    };

    let mut events = calendar::dedupe_events(events);
    events.retain(|event| !event.is_past(fetched_at));

    if cli.tui {
        ui::run(events, cli.details)
    } else {
        print_events(&events, cli.details, stdout_colors_enabled(cli.no_color));
        Ok(())
    }
}

fn print_events(events: &[CalendarEvent], show_details: bool, use_color: bool) {
    if events.is_empty() {
        println!("No upcoming appointments.");
        return;
    }

    let today = Local::now().date_naive();
    let mut current_day = None::<NaiveDate>;

    for event in events {
        let event_day = event.start_date();
        if current_day != Some(event_day) {
            if current_day.is_some() {
                println!();
            }
            let label = day_label(event_day, today);
            println!(
                "{}",
                colorize_day_label(&label, event_day, today, use_color)
            );
            current_day = Some(event_day);
        }

        println!("{}", event_line(event, show_details, use_color));
    }
}

fn event_line(event: &CalendarEvent, show_details: bool, use_color: bool) -> String {
    let category = event.category();
    let time = format!("{:<7}", time_label(event));
    let time = colorize_time_label(&time, category, use_color);
    let title = colorize_title(&event.title, category, use_color);

    if show_details {
        let details = colorize_details(&details(event), use_color);
        format!("  {time}  {title}  {details}")
    } else {
        format!("  {time}  {title}")
    }
}

fn details(event: &CalendarEvent) -> String {
    let mut parts = Vec::new();

    if let Some(minutes) = event.duration_minutes() {
        parts.push(format_duration(minutes));
    }

    if event.has_meet {
        parts.push("Meet".to_string());
    }

    parts.push(format!("{} · {}", event.account, event.calendar_name));

    if let Some(location) = event
        .location
        .as_deref()
        .filter(|location| !location.is_empty())
    {
        parts.push(location.to_string());
    }

    parts.join(" · ")
}

#[cfg(test)]
mod tests {
    use super::*;
    use chrono::{DateTime, Local, LocalResult, TimeZone};

    fn local_datetime(year: i32, month: u32, day: u32, hour: u32, minute: u32) -> DateTime<Local> {
        match Local.with_ymd_and_hms(year, month, day, hour, minute, 0) {
            LocalResult::Single(datetime) => datetime,
            LocalResult::Ambiguous(earliest, _) => earliest,
            LocalResult::None => Local
                .with_ymd_and_hms(year, month, day, hour + 1, minute, 0)
                .earliest()
                .expect("valid local datetime"),
        }
    }

    #[test]
    fn event_line_stays_plain_when_color_is_disabled() {
        let mut event = calendar::test_event("Some holiday", local_datetime(2026, 12, 25, 9, 0));
        event.all_day = true;

        assert_eq!(event_line(&event, false, false), "  all-day  Some holiday");
    }

    #[test]
    fn event_line_colors_category_when_enabled() {
        let mut event = calendar::test_event("Some holiday", local_datetime(2026, 12, 25, 9, 0));
        event.all_day = true;

        let line = event_line(&event, false, true);

        assert!(line.contains("\u{1b}[38;2;245;194;129m"));
        assert!(line.contains("\u{1b}[1;38;2;245;194;129mSome holiday\u{1b}[0m"));
    }

    #[test]
    fn cli_rejects_ics_with_goa_specific_options() {
        assert!(
            Cli::try_parse_from([
                "app",
                "--ics",
                "https://example.invalid/a.ics",
                "--list-accounts"
            ])
            .is_err()
        );
        assert!(
            Cli::try_parse_from([
                "app",
                "--ics",
                "https://example.invalid/a.ics",
                "--account",
                "work"
            ])
            .is_err()
        );
        assert!(
            Cli::try_parse_from([
                "app",
                "--ics",
                "https://example.invalid/a.ics",
                "--all-calendars"
            ])
            .is_err()
        );
    }

    #[test]
    fn cli_allows_multiple_ics_sources() {
        let cli = Cli::try_parse_from([
            "app",
            "--ics",
            "https://example.invalid/a.ics",
            "--ics",
            "https://example.invalid/b.ics",
        ])
        .expect("valid ICS CLI");

        assert_eq!(cli.ics.len(), 2);
    }
}
