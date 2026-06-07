mod calendar;
mod ui;

use anyhow::{Context, Result};
use chrono::Local;
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

    ui::run(events, cli.details)
}
