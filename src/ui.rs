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

use crate::calendar::{CalendarEvent, EventCategory};

const FG: Color = Color::Rgb(218, 218, 218);
const MUTED: Color = Color::Rgb(120, 126, 135);
const DIM: Color = Color::Rgb(88, 93, 101);
const ACCENT: Color = Color::Rgb(122, 162, 247);
const TODAY: Color = Color::Rgb(180, 220, 140);
const HOLIDAY: Color = Color::Rgb(245, 194, 129);
const BIRTHDAY: Color = Color::Rgb(203, 166, 247);
const TRAVEL: Color = Color::Rgb(137, 180, 250);
const FOCUS: Color = Color::Rgb(166, 227, 161);
const OUT_OF_OFFICE: Color = Color::Rgb(243, 139, 168);
const MEETING: Color = Color::Rgb(122, 162, 247);
const ALL_DAY: Color = Color::Rgb(186, 194, 222);

#[derive(Debug)]
struct App {
    events: Vec<CalendarEvent>,
    show_details: bool,
    start_index: usize,
    period_end_index: Option<usize>,
    last_plan: PlanSummary,
}

impl App {
    fn new(events: Vec<CalendarEvent>, show_details: bool) -> Self {
        Self {
            events,
            show_details,
            start_index: 0,
            period_end_index: None,
            last_plan: PlanSummary::default(),
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

pub fn run(events: Vec<CalendarEvent>, show_details: bool) -> Result<()> {
    enable_raw_mode()?;
    let mut guard = TerminalGuard {
        alternate_screen: false,
    };
    let mut stdout = io::stdout();
    execute!(stdout, EnterAlternateScreen)?;
    guard.alternate_screen = true;

    let backend = CrosstermBackend::new(stdout);
    let mut terminal = Terminal::new(backend)?;
    let mut app = App::new(events, show_details);
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

        if !event::poll(StdDuration::from_millis(250))? {
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
    );
    app.last_plan = summary;

    frame.render_widget(Paragraph::new(body_lines), chunks[0]);
    frame.render_widget(
        Paragraph::new(footer_text(app, width)).style(Style::default().fg(MUTED)),
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
) -> (Vec<Line<'static>>, PlanSummary) {
    if body_rows == 0 {
        return (Vec::new(), PlanSummary::default());
    }

    if width < 10 || body_rows < 2 {
        return (
            vec![Line::from(Span::styled(
                truncate_to_width("Terminal too small.", width),
                Style::default().fg(MUTED),
            ))],
            PlanSummary::default(),
        );
    }

    if events.is_empty() {
        let mut lines = vec![Line::from(Span::styled(
            "No upcoming appointments.",
            Style::default().fg(FG),
        ))];

        if body_rows > 1 {
            lines.push(Line::from(Span::styled(
                "Your calendar is clear.",
                Style::default().fg(DIM),
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
            lines.push(day_header(event_day, today, width));
            current_day = Some(event_day);
        }

        lines.push(event_line(event, show_details, width));
        summary.last_visible = Some(index);
        index += 1;
    }

    fill_more_summary(events, &mut summary, today);
    (lines, summary)
}

fn day_header(date: NaiveDate, today: NaiveDate, width: usize) -> Line<'static> {
    let label = day_label(date, today);
    let color = if date == today { TODAY } else { ACCENT };

    Line::from(Span::styled(
        truncate_to_width(&label, width),
        Style::default().fg(color).add_modifier(Modifier::BOLD),
    ))
}

fn event_line(event: &CalendarEvent, show_details: bool, width: usize) -> Line<'static> {
    let category = event.category();
    let marker = if width >= 20 { "▏ " } else { "" };
    let marker_width = UnicodeWidthStr::width(marker);

    if width < 18 {
        return Line::from(Span::styled(
            truncate_to_width(&event.title, width),
            title_style(category),
        ));
    }

    let time_label = time_label(event);
    let time_column = format!("{time_label:<7}");
    let prefix_width = marker_width + UnicodeWidthStr::width(time_column.as_str()) + 1;
    let meta = meta_label(event, show_details, width);
    let meta_width = UnicodeWidthStr::width(meta.as_str());
    let show_meta = !meta.is_empty() && width > prefix_width + meta_width + 12;
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
            Style::default().fg(category_color(category)),
        ));
    }

    spans.extend([
        Span::styled(time_column, time_style(category)),
        Span::raw(" "),
        Span::styled(title, title_style(category)),
    ]);

    if show_meta {
        let padding = width.saturating_sub(used + meta_width).max(1);
        spans.push(Span::raw(" ".repeat(padding)));
        spans.push(Span::styled(meta, Style::default().fg(DIM)));
    }

    Line::from(spans)
}

fn category_color(category: EventCategory) -> Color {
    match category {
        EventCategory::Holiday => HOLIDAY,
        EventCategory::Birthday => BIRTHDAY,
        EventCategory::Travel => TRAVEL,
        EventCategory::Focus => FOCUS,
        EventCategory::OutOfOffice => OUT_OF_OFFICE,
        EventCategory::Meeting => MEETING,
        EventCategory::AllDay => ALL_DAY,
        EventCategory::Other => MUTED,
    }
}

fn time_style(category: EventCategory) -> Style {
    match category {
        EventCategory::Holiday | EventCategory::OutOfOffice => {
            Style::default().fg(category_color(category))
        }
        _ => Style::default().fg(MUTED),
    }
}

fn title_style(category: EventCategory) -> Style {
    match category {
        EventCategory::Holiday => Style::default().fg(FG).add_modifier(Modifier::BOLD),
        EventCategory::OutOfOffice => Style::default().fg(FG).add_modifier(Modifier::ITALIC),
        _ => Style::default().fg(FG),
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

    if width >= 60 {
        let cal = if event.account == "default" {
            event.calendar_name.clone()
        } else {
            format!("{} · {}", event.account, event.calendar_name)
        };
        parts.push(cal);
    }

    if width >= 96
        && let Some(location) = event
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
    }
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
    if width < 20 {
        truncate_to_width("q", width)
    } else if width < 36 {
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

fn day_label(date: NaiveDate, today: NaiveDate) -> String {
    if date == today {
        "Today".to_string()
    } else if Some(date) == today.succ_opt() {
        "Tomorrow".to_string()
    } else {
        date.format("%a %b %-d").to_string()
    }
}

fn compact_day_label(date: NaiveDate) -> String {
    date.format("%a %b %-d").to_string()
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
