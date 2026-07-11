# AGENTS.md

## Repo shape
- Single Rust 2024 binary crate (`google-calendar-tui`, rust-version 1.88); real entrypoint is `src/main.rs`.
- Module split: `calendar.rs` handles GOA D-Bus account discovery, Google Calendar HTTP fetch/conversion, filtering, sanitization, dedupe; `ics.rs` handles opt-in private iCal/ICS fetch/parse; `display.rs` handles stdout labels/colors; `ui.rs` handles Ratatui TUI rendering and input.
- The app is intentionally read-only: default GOA mode gets short-lived Google tokens and does not persist tokens; opt-in `--ics` mode treats URLs as bearer read secrets; both modes fetch once, print/render, and exit.

## Commands to trust
- Run locally: `cargo run --` (default colored stdout), `cargo run -- --tui`, or headless/non-GNOME with `cargo run -- --ics '<secret-url>'`.
- GOA-dependent smoke checks: `cargo run -- --list-accounts`, `cargo run -- --account personal@example.com`, `cargo run -- --details`, `cargo run -- --no-color`.
- CI quality order: `cargo fmt --check` → `cargo clippy --locked --all-targets --all-features -- -D warnings` → `cargo test --locked --all-targets` → `cargo audit` → `cargo package --locked`.
- Focused unit test example: `cargo test --locked sanitize_display_text` or `cargo test --locked more_can_advance_to_later_days_when_next_hidden_event_is_not_same_week`.
- CI installs `nasm` because `ring` assembles primitives; if linking/building fails around crypto assembly, check that dependency first.

## Behavior invariants worth preserving
- Default calendar scope is primary + selected Google calendars only; `--all-calendars` may include hidden/unselected calendars, but free/busy-only calendars are still skipped.
- `--ics` must remain opt-in and skip GOA entirely; keep it mutually exclusive with GOA-only `--list-accounts`, `--account`, and `--all-calendars`.
- Never print/log full ICS URLs: Google secret iCal links are bearer read access and may also appear in shell history/process listings.
- Keep external Google/GOA strings sanitized before display/errors; tests cover ANSI, OSC, bidi marks, tabs, and newlines.
- Account filtering accepts exact id/email/display/path first, then case-insensitive substring matches, and must reject ambiguous filters.
- Duplicate events should prefer confirmed primary-calendar events with richer details such as Meet links.
- Stdout colors must respect both `--no-color` and `NO_COLOR`; TUI uses Ratatui colors separately.
- TUI must fit the terminal, truncate by display width (`unicode-width`), and preserve the `m`/Space/Down “more” flow plus `0`/Home reset.

## Release/package notes
- Release workflow requires a semver tag `vX.Y.Z` that matches `Cargo.toml` package version.
- AUR packaging lives in `packaging/aur/`; keep `pkgver` in both `PKGBUILD` and `PKGBUILD-bin` aligned with `Cargo.toml` when bumping versions.
- Source AUR build disables LTO/debug because `ring`/rustls objects and Arch defaults can otherwise cause link/debug-package issues.
- Release builds x86_64 natively with `cargo` and aarch64 with `cross`; native release tests only run for x86_64 in CI.
