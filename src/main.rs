mod calendar;
mod display;
mod ui;

use anyhow::{Context, Result};
use calendar::CalendarEvent;
use chrono::{Local, NaiveDate};
use clap::Parser;
use display::{day_label, format_duration, time_label};

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

    /// Show extra columns like duration, video indicator, account, and calendar name.
    #[arg(long)]
    details: bool,

    /// Use the interactive screen-fitting TUI with the `more` command.
    #[arg(long)]
    tui: bool,

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
    let events = calendar::fetch_accounts(calendar::FetchOptions {
        account_filters: &account_filters,
        fetch_days: cli.fetch_days.max(1),
        max_results_per_calendar: cli
            .max_results_per_calendar
            .clamp(1, calendar::GOOGLE_EVENTS_MAX_RESULTS_PER_PAGE),
        all_calendars: cli.all_calendars,
    })
    .await
    .context("calendar fetch failed")?;

    let mut events = calendar::dedupe_events(events);
    events.retain(|event| !event.is_past(fetched_at));

    if cli.tui {
        ui::run(events, cli.details)
    } else {
        print_events(&events, cli.details);
        Ok(())
    }
}

fn print_events(events: &[CalendarEvent], show_details: bool) {
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
            println!("{}", day_label(event_day, today));
            current_day = Some(event_day);
        }

        if show_details {
            println!(
                "  {:<7}  {}  {}",
                time_label(event),
                event.title,
                details(event)
            );
        } else {
            println!("  {:<7}  {}", time_label(event), event.title);
        }
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
