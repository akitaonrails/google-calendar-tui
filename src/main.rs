mod calendar;
mod ui;

use anyhow::{Context, Result};
use calendar::CalendarEvent;
use chrono::{Local, NaiveDate};
use clap::Parser;

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
    #[arg(long, default_value_t = 365, value_name = "DAYS")]
    fetch_days: i64,

    /// Maximum events to request per calendar page.
    #[arg(long, default_value_t = 2500, value_name = "N")]
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
                println!("{}\t{}\t{}", account.label(), account.id, account.path());
            }
        }
        return Ok(());
    }

    let fetched_at = Local::now();
    let events = calendar::fetch_accounts(calendar::FetchOptions {
        account_filters: &account_filters,
        fetch_days: cli.fetch_days.max(1),
        max_results_per_calendar: cli.max_results_per_calendar.clamp(1, 2500),
        all_calendars: cli.all_calendars,
    })
    .await
    .context("calendar fetch failed")?;

    let mut events = calendar::dedupe_events(events);
    events.retain(|event| !event.is_past(fetched_at));
    events.sort_by(calendar::sort_events);

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

fn day_label(date: NaiveDate, today: NaiveDate) -> String {
    if date == today {
        "Today".to_string()
    } else if Some(date) == today.succ_opt() {
        "Tomorrow".to_string()
    } else {
        date.format("%a %b %-d").to_string()
    }
}

fn time_label(event: &CalendarEvent) -> String {
    if event.all_day {
        if event.is_multi_day() {
            "multi".to_string()
        } else {
            "all-day".to_string()
        }
    } else {
        event.start.format("%H:%M").to_string()
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

fn format_duration(minutes: i64) -> String {
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
