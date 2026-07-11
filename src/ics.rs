use anyhow::{Context, Result, anyhow, bail};
use chrono::{DateTime, Duration, Local, LocalResult, NaiveDate, NaiveDateTime, TimeZone};
use icalendar::rrule::{Frequency, RRuleSet};
use icalendar::{Calendar, CalendarDateTime, Component, DatePerhapsTime, EventLike, EventStatus};
use reqwest::Client;

use crate::calendar::{CalendarEvent, ImportedEvent};

const HTTP_CONNECT_TIMEOUT_SECS: u64 = 10;
const HTTP_REQUEST_TIMEOUT_SECS: u64 = 30;
const MAX_ICS_BODY_BYTES: usize = 10 * 1024 * 1024;
const MAX_OCCURRENCES_PER_EVENT: u16 = 10_000;
const MAX_EVENTS_PER_SOURCE: usize = 10_000;
const HIGH_FREQUENCY_LOOKBACK_DAYS: i64 = 366;

pub async fn fetch_sources(urls: &[String], fetch_days: i64) -> Result<Vec<CalendarEvent>> {
    if urls.is_empty() {
        return Ok(Vec::new());
    }

    let client = Client::builder()
        .connect_timeout(std::time::Duration::from_secs(HTTP_CONNECT_TIMEOUT_SECS))
        .timeout(std::time::Duration::from_secs(HTTP_REQUEST_TIMEOUT_SECS))
        .build()
        .context("failed to build ICS HTTP client")?;

    let mut all_events = Vec::new();
    let mut successes = 0usize;
    let mut failures = Vec::new();

    for (index, url) in urls.iter().enumerate() {
        let label = source_label(index);
        match fetch_source(&client, url, &label, fetch_days).await {
            Ok(events) => {
                successes += 1;
                all_events.extend(events);
            }
            Err(error) => {
                let message = format!("{label}: {error:#}");
                eprintln!("Skipping {message}");
                failures.push(message);
            }
        }
    }

    if successes == 0 {
        if failures.is_empty() {
            bail!("No ICS source could be read.");
        }
        bail!("No ICS source could be read:\n{}", failures.join("\n"));
    }

    Ok(all_events)
}

async fn fetch_source(
    client: &Client,
    url: &str,
    source_label: &str,
    fetch_days: i64,
) -> Result<Vec<CalendarEvent>> {
    let response = client.get(url).send().await.map_err(|error| {
        anyhow!(
            "failed to send request for {source_label}: {}",
            error.without_url()
        )
    })?;

    let status = response.status();
    if !status.is_success() {
        bail!("{source_label} returned HTTP {status}");
    }

    if response
        .content_length()
        .is_some_and(|length| length > MAX_ICS_BODY_BYTES as u64)
    {
        bail!("{source_label} body is larger than {MAX_ICS_BODY_BYTES} bytes");
    }

    let body = read_limited_body(response, source_label).await?;
    let body = std::str::from_utf8(&body)
        .with_context(|| format!("{source_label} response is not valid UTF-8"))?;

    parse_source(body, source_label, fetch_days)
}

async fn read_limited_body(mut response: reqwest::Response, source_label: &str) -> Result<Vec<u8>> {
    let mut body = Vec::new();
    while let Some(chunk) = response.chunk().await.map_err(|error| {
        anyhow!(
            "failed to read response body for {source_label}: {}",
            error.without_url()
        )
    })? {
        if body.len() + chunk.len() > MAX_ICS_BODY_BYTES {
            bail!("{source_label} body is larger than {MAX_ICS_BODY_BYTES} bytes");
        }
        body.extend_from_slice(&chunk);
    }
    Ok(body)
}

fn source_label(index: usize) -> String {
    format!("ICS #{}", index + 1)
}

pub(crate) fn parse_source(
    body: &str,
    source_label: &str,
    fetch_days: i64,
) -> Result<Vec<CalendarEvent>> {
    let calendar: Calendar = body
        .parse()
        .map_err(|error| anyhow!("failed to parse {source_label}: {error}"))?;
    parse_calendar(calendar, source_label, fetch_days, Local::now())
}

fn parse_calendar(
    calendar: Calendar,
    source_label: &str,
    fetch_days: i64,
    window_start: DateTime<Local>,
) -> Result<Vec<CalendarEvent>> {
    let calendar_name = calendar
        .property_value("X-WR-CALNAME")
        .filter(|name| !name.trim().is_empty())
        .unwrap_or(source_label);
    let window_end = window_start + Duration::days(fetch_days.max(1));
    let mut events = Vec::new();

    for calendar_event in calendar.calendar_events() {
        if events.len() >= MAX_EVENTS_PER_SOURCE {
            break;
        }

        let event = calendar_event.event();
        if event.get_status() == Some(EventStatus::Cancelled) {
            continue;
        }

        let converted = convert_event(
            calendar_event,
            source_label,
            calendar_name,
            window_start,
            window_end,
        )?;
        for event in converted {
            if events.len() >= MAX_EVENTS_PER_SOURCE {
                break;
            }
            events.push(event);
        }
    }

    Ok(events)
}

fn convert_event(
    calendar_event: icalendar::CalendarEvent<'_>,
    source_label: &str,
    calendar_name: &str,
    window_start: DateTime<Local>,
    window_end: DateTime<Local>,
) -> Result<Vec<CalendarEvent>> {
    let event = calendar_event.event();
    let start = event
        .get_start()
        .ok_or_else(|| anyhow!("{source_label} event is missing DTSTART"))?;
    let end = event.get_end();
    let uid = event.get_uid();
    let summary = event.get_summary().unwrap_or("");
    let location = event.get_location();
    let status = event
        .get_status()
        .map(|status| format!("{status:?}").to_lowercase());
    let instance = normalize_dates(start, end)?;
    let duration = instance
        .end
        .map(|end| end.signed_duration_since(instance.start));
    let mut starts = recurrence_starts(
        calendar_event,
        instance.start,
        duration,
        window_start,
        window_end,
    )?;

    if starts.is_empty() {
        starts.push(instance.start);
    }

    let mut out = Vec::new();
    for start in starts
        .into_iter()
        .take(usize::from(MAX_OCCURRENCES_PER_EVENT))
    {
        let end = duration.map(|duration| start + duration);
        if !overlaps_window(start, end, window_start, window_end) {
            continue;
        }
        out.push(CalendarEvent::new_imported(ImportedEvent {
            title: summary,
            start,
            end,
            all_day: instance.all_day,
            calendar_name,
            account: source_label,
            location,
            calendar_id: source_label,
            ical_uid: uid,
            status: status.as_deref().or(Some("confirmed")),
        }));
    }

    Ok(out)
}

#[derive(Debug, Clone, Copy)]
struct NormalizedDate {
    start: DateTime<Local>,
    end: Option<DateTime<Local>>,
    all_day: bool,
}

fn normalize_dates(start: DatePerhapsTime, end: Option<DatePerhapsTime>) -> Result<NormalizedDate> {
    let (start, all_day) = convert_date(start)?;
    let end = end
        .map(convert_date)
        .transpose()?
        .map(|(end, _)| end)
        .or_else(|| all_day.then_some(start + Duration::days(1)));
    Ok(NormalizedDate {
        start,
        end,
        all_day,
    })
}

fn convert_date(value: DatePerhapsTime) -> Result<(DateTime<Local>, bool)> {
    match value {
        DatePerhapsTime::Date(date) => Ok((local_midnight(date)?, true)),
        DatePerhapsTime::DateTime(datetime) => Ok((convert_datetime(datetime)?, false)),
    }
}

fn convert_datetime(value: CalendarDateTime) -> Result<DateTime<Local>> {
    match value {
        CalendarDateTime::Utc(datetime) => Ok(datetime.with_timezone(&Local)),
        CalendarDateTime::Floating(datetime) => local_datetime(datetime),
        CalendarDateTime::WithTimezone { date_time, tzid } => {
            let tz: chrono_tz::Tz = tzid
                .parse()
                .map_err(|_| anyhow!("unsupported ICS timezone {}", safe_error_text(&tzid)))?;
            Ok(resolve_local_result(tz.from_local_datetime(&date_time))?.with_timezone(&Local))
        }
    }
}

fn local_midnight(date: NaiveDate) -> Result<DateTime<Local>> {
    let datetime = date
        .and_hms_opt(0, 0, 0)
        .ok_or_else(|| anyhow!("invalid all-day date"))?;
    local_datetime(datetime)
}

fn local_datetime(datetime: NaiveDateTime) -> Result<DateTime<Local>> {
    resolve_local_result(Local.from_local_datetime(&datetime))
}

fn resolve_local_result<Tz: TimeZone>(result: LocalResult<DateTime<Tz>>) -> Result<DateTime<Tz>> {
    match result {
        LocalResult::Single(datetime) => Ok(datetime),
        LocalResult::Ambiguous(earliest, _) => Ok(earliest),
        LocalResult::None => Err(anyhow!("local time does not exist")),
    }
}

fn recurrence_starts(
    calendar_event: icalendar::CalendarEvent<'_>,
    original_start: DateTime<Local>,
    duration: Option<Duration>,
    window_start: DateTime<Local>,
    window_end: DateTime<Local>,
) -> Result<Vec<DateTime<Local>>> {
    let recurrence = calendar_event
        .get_recurrence()
        .with_context(|| "failed to parse recurrence")?;
    validate_recurrence_cost(&recurrence, window_start)?;

    let expansion_start = duration
        .filter(|duration| *duration > Duration::zero())
        .map(|duration| window_start - duration)
        .unwrap_or(window_start);
    let from = icalendar::rrule::Tz::UTC
        .timestamp_opt(expansion_start.timestamp(), 0)
        .single()
        .ok_or_else(|| anyhow!("invalid recurrence start bound"))?;
    let to = icalendar::rrule::Tz::UTC
        .timestamp_opt(window_end.timestamp(), 0)
        .single()
        .ok_or_else(|| anyhow!("invalid recurrence end bound"))?;
    let dates = recurrence
        .after(from)
        .before(to)
        .all(MAX_OCCURRENCES_PER_EVENT)
        .dates;

    let mut starts = dates
        .into_iter()
        .map(|date| date.with_timezone(&Local))
        .collect::<Vec<_>>();
    if overlaps_window(original_start, None, window_start, window_end)
        && !starts.iter().any(|start| start == &original_start)
    {
        starts.push(original_start);
    }
    starts.sort();
    starts.dedup();
    Ok(starts)
}

fn validate_recurrence_cost(recurrence: &RRuleSet, window_start: DateTime<Local>) -> Result<()> {
    let has_unbounded_high_frequency = recurrence.get_rrule().iter().any(|rule| {
        matches!(
            rule.get_freq(),
            Frequency::Secondly | Frequency::Minutely | Frequency::Hourly
        ) && rule.get_count().is_none()
            && rule
                .get_until()
                .map(|until| until.with_timezone(&Local) > window_start)
                .unwrap_or(true)
    });

    if has_unbounded_high_frequency {
        let days_since_start = window_start
            .signed_duration_since(recurrence.get_dt_start().with_timezone(&Local))
            .num_days();
        if days_since_start > HIGH_FREQUENCY_LOOKBACK_DAYS {
            bail!(
                "skipping high-frequency unbounded recurrence older than {HIGH_FREQUENCY_LOOKBACK_DAYS} days"
            );
        }
    }

    Ok(())
}

fn safe_error_text(value: &str) -> String {
    let value = crate::calendar::sanitize_display_text(value);
    if value.chars().count() > 80 {
        let mut shortened = value.chars().take(80).collect::<String>();
        shortened.push('…');
        shortened
    } else {
        value
    }
}

fn overlaps_window(
    start: DateTime<Local>,
    end: Option<DateTime<Local>>,
    window_start: DateTime<Local>,
    window_end: DateTime<Local>,
) -> bool {
    let effective_end = end.unwrap_or(start);
    effective_end >= window_start && start <= window_end
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::calendar;

    fn parse(body: &str) -> Vec<CalendarEvent> {
        let calendar: Calendar = body.parse().expect("valid ICS parse");
        parse_calendar(calendar, "ICS #1", 3650, local_datetime(2026, 7, 11, 0, 0))
            .expect("valid ICS")
    }

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
    fn parses_timed_utc_tzid_and_floating_events() {
        let events = parse(
            "BEGIN:VCALENDAR\nVERSION:2.0\nX-WR-CALNAME:Work\nBEGIN:VEVENT\nUID:utc\nSUMMARY:UTC\nDTSTART:20260712T120000Z\nDTEND:20260712T130000Z\nEND:VEVENT\nBEGIN:VEVENT\nUID:tz\nSUMMARY;TZID=America/New_York:TZID\nDTSTART;TZID=America/New_York:20260713T090000\nDTEND;TZID=America/New_York:20260713T100000\nEND:VEVENT\nBEGIN:VEVENT\nUID:floating\nSUMMARY:Floating\nDTSTART:20260714T090000\nDTEND:20260714T100000\nEND:VEVENT\nEND:VCALENDAR",
        );

        assert_eq!(events.len(), 3);
        assert!(events.iter().all(|event| !event.all_day));
        assert!(events.iter().any(|event| event.title == "UTC"));
        assert!(events.iter().any(|event| event.title == "TZID"));
        assert!(events.iter().any(|event| event.title == "Floating"));
    }

    #[test]
    fn parses_all_day_exclusive_dtend() {
        let events = parse(
            "BEGIN:VCALENDAR\nVERSION:2.0\nBEGIN:VEVENT\nUID:all-day\nSUMMARY:Trip\nDTSTART;VALUE=DATE:20260712\nDTEND;VALUE=DATE:20260715\nEND:VEVENT\nEND:VCALENDAR",
        );

        assert_eq!(events.len(), 1);
        assert!(events[0].all_day);
        assert_eq!(
            events[0].start.date_naive(),
            NaiveDate::from_ymd_opt(2026, 7, 12).unwrap()
        );
        assert_eq!(
            events[0].end.unwrap().date_naive(),
            NaiveDate::from_ymd_opt(2026, 7, 15).unwrap()
        );
        assert!(events[0].is_multi_day());
    }

    #[test]
    fn all_day_without_dtend_gets_implicit_one_day_end() {
        let events = parse(
            "BEGIN:VCALENDAR\nVERSION:2.0\nBEGIN:VEVENT\nUID:all-day\nSUMMARY:Holiday\nDTSTART;VALUE=DATE:20260712\nEND:VEVENT\nEND:VCALENDAR",
        );

        assert_eq!(events.len(), 1);
        assert!(events[0].all_day);
        assert_eq!(
            events[0].end.unwrap().date_naive(),
            NaiveDate::from_ymd_opt(2026, 7, 13).unwrap()
        );
    }

    #[test]
    fn missing_summary_uses_fallback_and_sanitizes_location() {
        let events = parse(
            "BEGIN:VCALENDAR\nVERSION:2.0\nBEGIN:VEVENT\nUID:missing\nDTSTART:20260712T120000Z\nDTEND:20260712T130000Z\nLOCATION:Room\\nA\u{1b}[31m\nEND:VEVENT\nEND:VCALENDAR",
        );

        assert_eq!(events[0].title, "(untitled)");
        assert_eq!(events[0].location.as_deref(), Some("Room A"));
    }

    #[test]
    fn skips_cancelled_events() {
        let events = parse(
            "BEGIN:VCALENDAR\nVERSION:2.0\nBEGIN:VEVENT\nUID:cancelled\nSUMMARY:Nope\nSTATUS:CANCELLED\nDTSTART:20260712T120000Z\nDTEND:20260712T130000Z\nEND:VEVENT\nEND:VCALENDAR",
        );

        assert!(events.is_empty());
    }

    #[test]
    fn redacted_errors_do_not_include_secret_url() {
        let error = parse_source("not calendar", "ICS #1", 30)
            .unwrap_err()
            .to_string();

        assert!(error.contains("ICS #1"));
        assert!(!error.contains("secret"));
        assert!(!error.contains("http"));
    }

    #[test]
    fn uid_dedupe_collapses_identical_imported_events() {
        let events = parse(
            "BEGIN:VCALENDAR\nVERSION:2.0\nBEGIN:VEVENT\nUID:same\nSUMMARY:A\nDTSTART:20260712T120000Z\nDTEND:20260712T130000Z\nEND:VEVENT\nBEGIN:VEVENT\nUID:same\nSUMMARY:A duplicate\nDTSTART:20260712T120000Z\nDTEND:20260712T130000Z\nEND:VEVENT\nEND:VCALENDAR",
        );
        let deduped = calendar::dedupe_events(events);

        assert_eq!(deduped.len(), 1);
    }

    #[test]
    fn expands_rrule_rdate_and_exdate_within_bound() {
        let events = parse(
            "BEGIN:VCALENDAR\nVERSION:2.0\nBEGIN:VEVENT\nUID:recur\nSUMMARY:Daily\nDTSTART:20260712T120000Z\nDTEND:20260712T130000Z\nRRULE:FREQ=DAILY;COUNT=3\nRDATE:20260720T120000Z\nEXDATE:20260713T120000Z\nEND:VEVENT\nEND:VCALENDAR",
        );

        let dates = events
            .iter()
            .map(|event| event.start.date_naive())
            .collect::<Vec<_>>();
        assert!(dates.contains(&NaiveDate::from_ymd_opt(2026, 7, 12).unwrap()));
        assert!(!dates.contains(&NaiveDate::from_ymd_opt(2026, 7, 13).unwrap()));
        assert!(dates.contains(&NaiveDate::from_ymd_opt(2026, 7, 14).unwrap()));
        assert!(dates.contains(&NaiveDate::from_ymd_opt(2026, 7, 20).unwrap()));
        assert!(events.len() <= usize::from(MAX_OCCURRENCES_PER_EVENT));
    }

    #[test]
    fn unsupported_timezone_error_is_sanitized() {
        let error = parse_source(
            "BEGIN:VCALENDAR\nVERSION:2.0\nBEGIN:VEVENT\nUID:bad-tz\nSUMMARY:Bad\nDTSTART;TZID=Bad\u{1b}[31mZone:20260712T120000\nDTEND;TZID=Bad\u{1b}[31mZone:20260712T130000\nEND:VEVENT\nEND:VCALENDAR",
            "ICS #1",
            30,
        )
        .unwrap_err()
        .to_string();

        assert!(error.contains("unsupported ICS timezone"));
        assert!(!error.contains('\u{1b}'));
    }

    #[test]
    fn rejects_old_unbounded_high_frequency_recurrence() {
        let error = parse_source(
            "BEGIN:VCALENDAR\nVERSION:2.0\nBEGIN:VEVENT\nUID:fast\nSUMMARY:Fast\nDTSTART:20200101T000000Z\nDTEND:20200101T000100Z\nRRULE:FREQ=SECONDLY\nEND:VEVENT\nEND:VCALENDAR",
            "ICS #1",
            30,
        )
        .unwrap_err()
        .to_string();

        assert!(error.contains("high-frequency unbounded recurrence"));
    }
}
