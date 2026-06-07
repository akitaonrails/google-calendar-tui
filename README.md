# google-calendar-tui

A quiet, read-only terminal agenda for Google Calendar using **GNOME Online Accounts**.

It fetches upcoming appointments once, renders only what fits in the terminal, and exits when you press `q`. It does not poll in the background.

## Features

- Uses your existing Google accounts from GNOME Online Accounts.
- No app-specific OAuth client setup or token cache.
- Read-only Google Calendar API access.
- One or more GOA Google accounts.
- Responsive Ratatui UI that reflows on terminal resize.
- Compact grouped agenda: busy days fill the screen; sparse calendars look further ahead.
- Minimal default rows: category marker, time, and appointment title only.
- Subtle category color markers for holidays, birthdays, travel, focus time, out-of-office, meetings, and all-day events.
- Optional `--details` mode for duration, video, account, calendar, and location columns.
- `m` / space / down-arrow reveals more only when hidden appointments remain in the current day or week.
- Duplicate event suppression across calendars/accounts.

## Setup

### Arch Linux

Install from AUR:

```sh
yay -S google-calendar-tui-bin      # prebuilt binary from GitHub Releases
yay -S google-calendar-tui          # builds from source
```

### From source

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

## Commands

Inside the TUI:

- `q` / Esc: quit
- `m` / Space / Down: more, when hidden appointments remain in the current day/week
- `0` / Home: return to the first page after using more

## Category colors

Each appointment row gets a small colored marker. Titles stay mostly neutral to avoid visual noise.

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
--fetch-days DAYS          Future days fetched once at startup; default 365
--max-results-per-calendar N
```

## Notes

GOA returns short-lived access tokens over D-Bus. The app does not persist Google tokens. If GOA says the account needs attention, fix it in GNOME Settings and run the app again.
