use std::{io, time::Duration as StdDuration};

use anyhow::Result;
use chrono::{Datelike, Local, NaiveDate};
use crossterm::{
    event::{self, Event as CrosstermEvent, KeyCode, KeyEventKind},
    execute,
    terminal::{EnterAlternateScreen, LeaveAlternateScreen, disable_raw_mode, enable_raw_mode},
};
use ratatui::{
    Frame, Terminal,
    backend::CrosstermBackend,
    layout::{Constraint, Layout},
    style::{Color, Modifier, Style},
    text::{Line, Span},
    widgets::Paragraph,
};
use unicode_width::{UnicodeWidthChar, UnicodeWidthStr};

use crate::{
    calendar::{CalendarEvent, EventCategory},
    display::{Palette, RgbColor, compact_day_label, day_label, format_duration, time_label},
};

const EVENT_POLL_INTERVAL_MS: u64 = 250;
const MIN_BODY_WIDTH: usize = 10;
const MIN_BODY_ROWS: usize = 2;
const COMPACT_EVENT_WIDTH: usize = 18;
const CATEGORY_MARKER_MIN_WIDTH: usize = 20;
const META_MIN_TITLE_GAP: usize = 12;
const SOURCE_META_MIN_WIDTH: usize = 60;
const LOCATION_META_MIN_WIDTH: usize = 96;
const FOOTER_TINY_WIDTH: usize = 20;
const FOOTER_COMPACT_WIDTH: usize = 36;

#[derive(Debug)]
struct App {
    events: Vec<CalendarEvent>,
    show_details: bool,
    start_index: usize,
    period_end_index: Option<usize>,
    last_plan: PlanSummary,
    palette: Palette,
}

impl App {
    fn new(events: Vec<CalendarEvent>, show_details: bool, palette: Palette) -> Self {
        Self {
            events,
            show_details,
            start_index: 0,
            period_end_index: None,
            last_plan: PlanSummary::default(),
            palette,
        }
    }

    fn more(&mut self) {
        if self.last_plan.can_more
            && let Some(index) = self.last_plan.first_hidden
        {
            self.start_index = index.min(self.events.len());
            self.period_end_index = self.last_plan.more_end_index;
        }
    }

    fn top(&mut self) {
        self.start_index = 0;
        self.period_end_index = None;
    }
}

#[derive(Debug, Clone, Default)]
struct PlanSummary {
    first_hidden: Option<usize>,
    last_visible: Option<usize>,
    can_more: bool,
    more_hidden_count: usize,
    more_label: Option<String>,
    more_end_index: Option<usize>,
}

pub fn run(events: Vec<CalendarEvent>, show_details: bool, palette: Palette) -> Result<()> {
    enable_raw_mode()?;
    let mut guard = TerminalGuard {
        alternate_screen: false,
    };
    let mut stdout = io::stdout();
    execute!(stdout, EnterAlternateScreen)?;
    guard.alternate_screen = true;

    let backend = CrosstermBackend::new(stdout);
    let mut terminal = Terminal::new(backend)?;
    let mut app = App::new(events, show_details, palette);
    let result = run_loop(&mut terminal, &mut app);

    let _ = terminal.show_cursor();

    result
}

struct TerminalGuard {
    alternate_screen: bool,
}

impl Drop for TerminalGuard {
    fn drop(&mut self) {
        let _ = disable_raw_mode();
        if self.alternate_screen {
            let mut stdout = io::stdout();
            let _ = execute!(stdout, LeaveAlternateScreen);
        }
    }
}

fn run_loop(terminal: &mut Terminal<CrosstermBackend<io::Stdout>>, app: &mut App) -> Result<()> {
    loop {
        terminal.draw(|frame| render(frame, app))?;

        if !event::poll(StdDuration::from_millis(EVENT_POLL_INTERVAL_MS))? {
            continue;
        }

        match event::read()? {
            CrosstermEvent::Key(key) if key.kind == KeyEventKind::Press => match key.code {
                KeyCode::Char('q') | KeyCode::Esc => break,
                KeyCode::Char('m') | KeyCode::Char(' ') | KeyCode::Down => app.more(),
                KeyCode::Home | KeyCode::Char('0') => app.top(),
                _ => {}
            },
            CrosstermEvent::Resize(_, _) => {}
            _ => {}
        }
    }

    Ok(())
}

fn render(frame: &mut Frame<'_>, app: &mut App) {
    let area = frame.area();
    let width = area.width as usize;

    if area.height < 2 {
        let line = if app.events.is_empty() {
            "No upcoming appointments. q quit"
        } else {
            "Upcoming appointments. q quit"
        };
        frame.render_widget(Paragraph::new(truncate_to_width(line, width)), area);
        app.last_plan = PlanSummary::default();
        return;
    }

    let chunks = Layout::vertical([Constraint::Min(0), Constraint::Length(1)]).split(area);

    let (body_lines, summary) = build_body_plan(
        &app.events,
        app.start_index,
        app.period_end_index,
        app.show_details,
        chunks[0].height as usize,
        width,
        app.palette,
    );
    app.last_plan = summary;

    frame.render_widget(Paragraph::new(body_lines), chunks[0]);
    frame.render_widget(
        Paragraph::new(footer_text(app, width))
            .style(Style::default().fg(tui_color(app.palette.muted))),
        chunks[1],
    );
}

fn build_body_plan(
    events: &[CalendarEvent],
    start_index: usize,
    period_end_index: Option<usize>,
    show_details: bool,
    body_rows: usize,
    width: usize,
    palette: Palette,
) -> (Vec<Line<'static>>, PlanSummary) {
    if body_rows == 0 {
        return (Vec::new(), PlanSummary::default());
    }

    if width < MIN_BODY_WIDTH || body_rows < MIN_BODY_ROWS {
        return (
            vec![Line::from(Span::styled(
                truncate_to_width("Terminal too small.", width),
                Style::default().fg(tui_color(palette.muted)),
            ))],
            PlanSummary::default(),
        );
    }

    if events.is_empty() {
        let mut lines = vec![Line::from(Span::styled(
            "No upcoming appointments.",
            Style::default().fg(tui_color(palette.foreground)),
        ))];

        if body_rows > 1 {
            lines.push(Line::from(Span::styled(
                "Your calendar is clear.",
                Style::default().fg(tui_color(palette.dim)),
            )));
        }

        return (lines, PlanSummary::default());
    }

    let start_index = start_index.min(events.len().saturating_sub(1));
    let mut lines = Vec::with_capacity(body_rows);
    let mut current_day = None::<NaiveDate>;
    let mut index = start_index;
    let end_index = period_end_index.unwrap_or(events.len()).min(events.len());
    let mut summary = PlanSummary::default();
    let today = Local::now().date_naive();

    while index < end_index {
        let event = &events[index];
        let event_day = event.start_date();
        let needs_day_header = current_day != Some(event_day);
        let needed_rows = usize::from(needs_day_header) + 1;

        if lines.len() + needed_rows > body_rows {
            summary.first_hidden = Some(index);
            break;
        }

        if needs_day_header {
            lines.push(day_header(event_day, today, width, palette));
            current_day = Some(event_day);
        }

        lines.push(event_line(event, show_details, width, palette));
        summary.last_visible = Some(index);
        index += 1;
    }

    fill_more_summary(events, &mut summary, today);
    (lines, summary)
}

fn day_header(date: NaiveDate, today: NaiveDate, width: usize, palette: Palette) -> Line<'static> {
    let label = day_label(date, today);
    let color = if date == today {
        palette.today
    } else {
        palette.accent
    };

    Line::from(Span::styled(
        truncate_to_width(&label, width),
        Style::default()
            .fg(tui_color(color))
            .add_modifier(Modifier::BOLD),
    ))
}

fn event_line(
    event: &CalendarEvent,
    show_details: bool,
    width: usize,
    palette: Palette,
) -> Line<'static> {
    let category = event.category();
    let marker = if width >= CATEGORY_MARKER_MIN_WIDTH {
        "▏ "
    } else {
        ""
    };
    let marker_width = UnicodeWidthStr::width(marker);

    if width < COMPACT_EVENT_WIDTH {
        return Line::from(Span::styled(
            truncate_to_width(&event.title, width),
            title_style(category, palette),
        ));
    }

    let time_label = time_label(event);
    let time_column = format!("{time_label:<7}");
    let prefix_width = marker_width + UnicodeWidthStr::width(time_column.as_str()) + 1;
    let meta = meta_label(event, show_details, width);
    let meta_width = UnicodeWidthStr::width(meta.as_str());
    let show_meta = !meta.is_empty() && width > prefix_width + meta_width + META_MIN_TITLE_GAP;
    let title_width = if show_meta {
        width.saturating_sub(prefix_width + meta_width + 2)
    } else {
        width.saturating_sub(prefix_width)
    };
    let title = truncate_to_width(&event.title, title_width);
    let used = prefix_width + UnicodeWidthStr::width(title.as_str());

    let mut spans = Vec::new();

    if !marker.is_empty() {
        spans.push(Span::styled(
            marker,
            Style::default().fg(tui_color(palette.category_color(category))),
        ));
    }

    spans.extend([
        Span::styled(time_column, time_style(category, palette)),
        Span::raw(" "),
        Span::styled(title, title_style(category, palette)),
    ]);

    if show_meta {
        let padding = width.saturating_sub(used + meta_width).max(1);
        spans.push(Span::raw(" ".repeat(padding)));
        spans.push(Span::styled(
            meta,
            Style::default().fg(tui_color(palette.dim)),
        ));
    }

    Line::from(spans)
}

fn tui_color(color: RgbColor) -> Color {
    Color::Rgb(color.red, color.green, color.blue)
}

fn time_style(category: EventCategory, palette: Palette) -> Style {
    match category {
        EventCategory::Holiday | EventCategory::OutOfOffice => {
            Style::default().fg(tui_color(palette.category_color(category)))
        }
        _ => Style::default().fg(tui_color(palette.muted)),
    }
}

fn title_style(category: EventCategory, palette: Palette) -> Style {
    match category {
        EventCategory::Holiday => Style::default()
            .fg(tui_color(palette.foreground))
            .add_modifier(Modifier::BOLD),
        EventCategory::OutOfOffice => Style::default()
            .fg(tui_color(palette.foreground))
            .add_modifier(Modifier::ITALIC),
        _ => Style::default().fg(tui_color(palette.foreground)),
    }
}

fn meta_label(event: &CalendarEvent, show_details: bool, width: usize) -> String {
    if !show_details {
        return String::new();
    }

    let mut parts = Vec::new();

    if let Some(minutes) = event.duration_minutes() {
        parts.push(format_duration(minutes));
    }

    if event.has_meet {
        parts.push("Meet".to_string());
    }

    if width >= SOURCE_META_MIN_WIDTH {
        parts.push(format!("{} · {}", event.account, event.calendar_name));
    }

    if width >= LOCATION_META_MIN_WIDTH
        && let Some(location) = event
            .location
            .as_deref()
            .filter(|location| !location.is_empty())
    {
        parts.push(location.to_string());
    }

    parts.join(" · ")
}

fn fill_more_summary(events: &[CalendarEvent], summary: &mut PlanSummary, today: NaiveDate) {
    let (Some(first_hidden), Some(last_visible)) = (summary.first_hidden, summary.last_visible)
    else {
        return;
    };

    let hidden_day = events[first_hidden].start_date();
    let visible_day = events[last_visible].start_date();

    if hidden_day == visible_day {
        summary.can_more = true;
        summary.more_hidden_count = events[first_hidden..]
            .iter()
            .take_while(|event| event.start_date() == hidden_day)
            .count();
        summary.more_end_index = Some(first_hidden + summary.more_hidden_count);
        summary.more_label = Some(if hidden_day == today {
            "today".to_string()
        } else {
            "that day".to_string()
        });
        return;
    }

    let hidden_week = hidden_day.iso_week();
    let visible_week = visible_day.iso_week();
    if hidden_week.year() == visible_week.year() && hidden_week.week() == visible_week.week() {
        let today_week = today.iso_week();
        summary.can_more = true;
        summary.more_hidden_count = events[first_hidden..]
            .iter()
            .take_while(|event| {
                let week = event.start_date().iso_week();
                week.year() == hidden_week.year() && week.week() == hidden_week.week()
            })
            .count();
        summary.more_end_index = Some(first_hidden + summary.more_hidden_count);
        summary.more_label = Some(
            if hidden_week.year() == today_week.year() && hidden_week.week() == today_week.week() {
                "this week"
            } else {
                "that week"
            }
            .to_string(),
        );
        return;
    }

    summary.can_more = true;
    summary.more_hidden_count = events.len().saturating_sub(first_hidden);
    summary.more_end_index = None;
    summary.more_label = Some("later".to_string());
}

fn footer_text(app: &App, width: usize) -> String {
    let mut parts = vec!["q quit".to_string()];

    if app.start_index > 0 {
        parts.push("0 top".to_string());
    }

    if app.last_plan.can_more {
        let label = app.last_plan.more_label.as_deref().unwrap_or("this period");
        parts.push(format!(
            "m more · {} hidden {label}",
            app.last_plan.more_hidden_count
        ));
    } else if let Some(last_visible) = app.last_plan.last_visible
        && app.last_plan.first_hidden.is_some()
    {
        let date = app.events[last_visible].start_date();
        parts.push(format!("showing through {}", compact_day_label(date)));
    }

    let full = parts.join(" · ");
    if width < FOOTER_TINY_WIDTH {
        truncate_to_width("q", width)
    } else if width < FOOTER_COMPACT_WIDTH {
        let compact = if app.last_plan.can_more {
            "q · m"
        } else {
            "q"
        };
        truncate_to_width(compact, width)
    } else {
        truncate_to_width(&full, width)
    }
}

fn truncate_to_width(value: &str, max_width: usize) -> String {
    if UnicodeWidthStr::width(value) <= max_width {
        return value.to_string();
    }

    if max_width == 0 {
        return String::new();
    }

    if max_width == 1 {
        return "…".to_string();
    }

    let mut out = String::new();
    let mut used = 0usize;
    let ellipsis_width = 1usize;

    for ch in value.chars() {
        let ch_width = UnicodeWidthChar::width(ch).unwrap_or(0);
        if used + ch_width + ellipsis_width > max_width {
            break;
        }

        out.push(ch);
        used += ch_width;
    }

    out.push('…');
    out
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
    fn truncate_to_width_preserves_display_width_budget() {
        assert_eq!(truncate_to_width("abcdef", 4), "abc…");
        assert_eq!(truncate_to_width("abcdef", 1), "…");
        assert_eq!(truncate_to_width("abcdef", 0), "");
        assert!(UnicodeWidthStr::width(truncate_to_width("a界b", 3).as_str()) <= 3);
    }

    #[test]
    fn more_can_advance_to_later_days_when_next_hidden_event_is_not_same_week() {
        let events = vec![
            crate::calendar::test_event("First", local_datetime(2026, 1, 1, 9, 0)),
            crate::calendar::test_event("Later", local_datetime(2026, 2, 1, 9, 0)),
            crate::calendar::test_event("Latest", local_datetime(2026, 3, 1, 9, 0)),
        ];

        let (_lines, summary) = build_body_plan(&events, 0, None, false, 2, 80, Palette::DEFAULT);

        assert_eq!(summary.first_hidden, Some(1));
        assert!(summary.can_more);
        assert_eq!(summary.more_hidden_count, 2);
        assert_eq!(summary.more_label.as_deref(), Some("later"));
        assert_eq!(summary.more_end_index, None);
    }

    #[test]
    fn body_plan_uses_selected_theme_colors() {
        let events = vec![crate::calendar::test_event(
            "Vacation",
            local_datetime(2026, 8, 1, 9, 0),
        )];

        let (lines, _summary) = build_body_plan(&events, 0, None, false, 2, 80, Palette::NERV);

        assert_eq!(lines[0].spans[0].style.fg, Some(Color::Rgb(0, 255, 135)));
        assert_eq!(lines[1].spans[0].style.fg, Some(Color::Rgb(255, 0, 0)));
        assert_eq!(lines[1].spans[1].style.fg, Some(Color::Rgb(255, 0, 0)));
    }
}
