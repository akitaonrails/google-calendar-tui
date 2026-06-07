# google-calendar-tui

A quiet, read-only terminal agenda for Google Calendar using **GNOME Online Accounts**.

It fetches upcoming appointments once, prints them to stdout, and exits. It does not poll in the background. An optional interactive TUI can render only what fits in the terminal and reveal more with `m`.

## Features

- Uses your existing Google accounts from GNOME Online Accounts.
- No app-specific OAuth client setup or token cache.
- Read-only Google Calendar API access.
- One or more GOA Google accounts.
- Colored plain stdout output by default for quick shell use; disable ANSI colors with `--no-color`.
- Optional responsive Ratatui UI with screen-fitting `more` behavior.
- Minimal default rows: day, time, and appointment title only.
- Subtle category color markers in TUI mode for holidays, birthdays, travel, focus time, out-of-office, meetings, and all-day events.
- Matching category colors in stdout mode for holidays, birthdays, travel, focus time, out-of-office, meetings, and all-day events.
- Optional `--details` mode for duration, video, account, calendar, and location columns.
- Fetches up to 60 days ahead by default; override with `--fetch-days`.
- Duplicate event suppression across calendars/accounts.
- Strips terminal control sequences from Google/GOA display text before printing or rendering.

## Setup

### Arch Linux

Install from AUR:

```sh
yay -S google-calendar-tui-bin      # prebuilt binary from GitHub Releases
yay -S google-calendar-tui          # builds from source
```

### From source

Requires Rust 1.88 or newer.

```sh
cargo install --git https://github.com/akitaonrails/google-calendar-tui
```

### GNOME account

Add your Google account in GNOME:

```text
GNOME Settings > Online Accounts > Google
```

Make sure Calendar is enabled for that account.

Then run:

```sh
google-calendar-tui
```

When developing from a checkout, use `cargo run` instead.

Default output is plain text:

```text
Today
  09:00    Standup
  all-day  Some holiday

Tomorrow
  14:00    Dentist
```

## Accounts

List Google Calendar accounts known to GOA:

```sh
cargo run -- --list-accounts
```

Use all accounts by default, or filter by email/display/id:

```sh
cargo run -- --account personal@example.com
cargo run -- --account personal@example.com --account work@example.com
cargo run -- --account personal,work
```

Show extra source/details columns:

```sh
cargo run -- --details
```

Disable ANSI colors in stdout mode:

```sh
cargo run -- --no-color
NO_COLOR=1 cargo run --
```

Use the interactive TUI with screen-fitting `more` behavior:

```sh
cargo run -- --tui
```

## TUI commands

Inside `--tui` mode:

- `q` / Esc: quit
- `m` / Space / Down: more, when hidden appointments remain in the current day/week
- `0` / Home: return to the first page after using more

## Category colors

In stdout mode, appointment rows are colored by category. In `--tui` mode, each appointment row gets a small colored marker and titles stay mostly neutral to avoid visual noise.

- amber: holidays
- violet: birthdays
- blue: travel
- green: focus time
- red: out-of-office / vacation
- soft blue: meetings with video links
- slate: all-day events

Holiday detection uses Google holiday calendar IDs/names plus common holiday terms, including `holiday`, `feriado`, and `festivo`.

## Options

```text
--list-accounts            List usable GOA Google Calendar accounts and exit
--account NAME             Filter GOA accounts; repeat or comma-separate
--all-calendars            Include hidden/unselected calendars
--details                  Show duration, Meet, account/calendar, and location columns
--tui                      Use the interactive TUI with the `more` command
--no-color                 Disable ANSI colors in plain stdout output
--fetch-days DAYS          Future days fetched once at startup; default 60
--max-results-per-calendar N
```

## Notes

GOA returns short-lived access tokens over D-Bus. The app does not persist Google tokens. If GOA says the account needs attention, fix it in GNOME Settings and run the app again.

By default, only your primary and selected Google calendars are read. Use `--all-calendars` only when you also want calendars hidden/unselected in Google Calendar.
