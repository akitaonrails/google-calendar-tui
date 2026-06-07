use std::{cmp::Ordering, collections::HashMap, fmt};

use anyhow::{Context, Result, anyhow, bail};
use chrono::{DateTime, Duration, Local, NaiveDate, TimeZone};
use reqwest::{Client, StatusCode};
use serde::Deserialize;
use serde::de::DeserializeOwned;
use zbus::{Connection, Proxy, fdo::ObjectManagerProxy};
use zvariant::OwnedObjectPath;

const API_ROOT: &str = "https://www.googleapis.com/calendar/v3";
const GOA_SERVICE: &str = "org.gnome.OnlineAccounts";
const GOA_ROOT: &str = "/org/gnome/OnlineAccounts";
const IFACE_ACCOUNT: &str = "org.gnome.OnlineAccounts.Account";
const IFACE_CALENDAR: &str = "org.gnome.OnlineAccounts.Calendar";
const IFACE_OAUTH2: &str = "org.gnome.OnlineAccounts.OAuth2Based";
const HOLIDAY_TITLE_TERMS: &[&str] = &[
    "holiday",
    "christmas",
    "new year",
    "thanksgiving",
    "memorial day",
    "labor day",
    "independence day",
    "easter",
    "good friday",
    "carnival",
    "tiradentes",
    "finados",
    "natal",
    "ano novo",
    "confraternização",
    "confraternizacao",
    "proclamação",
    "proclamacao",
    "consciência negra",
    "consciencia negra",
    "corpus christi",
    "paixão de cristo",
    "paixao de cristo",
    "feriado",
];

#[derive(Debug)]
pub struct FetchOptions<'a> {
    pub account_filters: &'a [String],
    pub fetch_days: i64,
    pub max_results_per_calendar: u32,
    pub all_calendars: bool,
}

#[derive(Debug, Clone)]
pub struct GoaAccount {
    path: OwnedObjectPath,
    pub id: String,
    pub identity: String,
    pub presentation_identity: String,
}

impl GoaAccount {
    pub fn label(&self) -> String {
        if !self.presentation_identity.trim().is_empty() {
            self.presentation_identity.clone()
        } else if !self.identity.trim().is_empty() {
            self.identity.clone()
        } else {
            self.id.clone()
        }
    }

    pub fn path(&self) -> &OwnedObjectPath {
        &self.path
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum EventCategory {
    Holiday,
    Birthday,
    Travel,
    Focus,
    OutOfOffice,
    Meeting,
    AllDay,
    Other,
}

#[derive(Debug, Clone)]
pub struct CalendarEvent {
    pub title: String,
    pub start: DateTime<Local>,
    pub end: Option<DateTime<Local>>,
    pub all_day: bool,
    pub calendar_name: String,
    pub account: String,
    pub location: Option<String>,
    pub has_meet: bool,
    pub primary_calendar: bool,
    calendar_id: String,
    event_type: Option<String>,
    ical_uid: Option<String>,
    status: Option<String>,
}

impl CalendarEvent {
    pub fn is_past(&self, now: DateTime<Local>) -> bool {
        self.end.unwrap_or(self.start) < now
    }

    pub fn start_date(&self) -> NaiveDate {
        self.start.date_naive()
    }

    pub fn duration_minutes(&self) -> Option<i64> {
        if self.all_day {
            return None;
        }

        let minutes = self.end?.signed_duration_since(self.start).num_minutes();
        (minutes > 0).then_some(minutes)
    }

    pub fn is_multi_day(&self) -> bool {
        let Some(end) = self.end else {
            return false;
        };

        if self.all_day {
            let one_day_after_start = self.start.date_naive().succ_opt();
            return one_day_after_start
                .map(|next_day| end.date_naive() > next_day)
                .unwrap_or(false);
        }

        end.date_naive() > self.start.date_naive()
    }

    pub fn category(&self) -> EventCategory {
        if self.is_holiday() {
            EventCategory::Holiday
        } else if self.is_birthday() {
            EventCategory::Birthday
        } else if self.is_out_of_office() {
            EventCategory::OutOfOffice
        } else if self.is_focus_time() {
            EventCategory::Focus
        } else if self.is_travel() {
            EventCategory::Travel
        } else if self.has_meet {
            EventCategory::Meeting
        } else if self.all_day {
            EventCategory::AllDay
        } else {
            EventCategory::Other
        }
    }

    fn is_holiday(&self) -> bool {
        contains_any(
            &[self.calendar_id.as_str(), self.calendar_name.as_str()],
            &[
                "#holiday@",
                "holiday@group.v.calendar.google.com",
                "holiday",
                "holidays",
                "feriado",
                "feriados",
                "festivo",
                "festivos",
                "dias festivos",
            ],
        ) || (self.all_day && contains_any(&[self.title.as_str()], HOLIDAY_TITLE_TERMS))
    }

    fn is_birthday(&self) -> bool {
        self.event_type.as_deref() == Some("birthday")
            || contains_any(
                &[self.title.as_str()],
                &["birthday", "aniversário", "aniversario"],
            )
    }

    fn is_out_of_office(&self) -> bool {
        self.event_type.as_deref() == Some("outOfOffice")
            || contains_any(
                &[self.title.as_str(), self.calendar_name.as_str()],
                &[
                    "out of office",
                    "ooo",
                    "vacation",
                    "holiday leave",
                    "férias",
                    "ferias",
                    "ausente",
                ],
            )
    }

    fn is_focus_time(&self) -> bool {
        self.event_type.as_deref() == Some("focusTime")
            || contains_any(
                &[self.title.as_str()],
                &["focus", "deep work", "foco", "concentration"],
            )
    }

    fn is_travel(&self) -> bool {
        contains_any(
            &[
                self.title.as_str(),
                self.location.as_deref().unwrap_or_default(),
                self.calendar_name.as_str(),
            ],
            &[
                "flight",
                "voo",
                "airport",
                "aeroporto",
                "boarding",
                "embarque",
                "hotel",
                "travel",
                "trip",
                "viagem",
                "train",
                "trem",
                "bus",
                "ônibus",
                "onibus",
                "reservation",
                "reserva",
            ],
        )
    }

    fn dedupe_key(&self) -> String {
        let end_timestamp = self.end.map(|end| end.timestamp()).unwrap_or_default();

        if let Some(ical_uid) = self.ical_uid.as_deref().filter(|uid| !uid.is_empty()) {
            return format!(
                "ical:{}|{}|{}",
                normalize_text(ical_uid),
                self.start.timestamp(),
                end_timestamp
            );
        }

        format!(
            "fallback:{}|{}|{}",
            normalize_text(&self.title),
            self.start.timestamp(),
            end_timestamp
        )
    }

    fn rank_for_dedupe(&self) -> (u8, u8, u8, u8) {
        (
            u8::from(self.status.as_deref() == Some("confirmed")),
            u8::from(self.primary_calendar),
            u8::from(self.has_meet || self.location.is_some()),
            u8::from(!self.title.trim().is_empty()),
        )
    }
}

#[derive(Debug, Clone)]
struct CalendarRef {
    id: String,
    name: String,
    primary: bool,
}

#[derive(Debug, Deserialize)]
struct CalendarListResponse {
    #[serde(default)]
    items: Vec<CalendarListEntry>,
    #[serde(rename = "nextPageToken")]
    next_page_token: Option<String>,
}

#[derive(Debug, Deserialize)]
struct CalendarListEntry {
    id: Option<String>,
    summary: Option<String>,
    #[serde(rename = "summaryOverride")]
    summary_override: Option<String>,
    primary: Option<bool>,
    selected: Option<bool>,
    #[serde(rename = "accessRole")]
    access_role: Option<String>,
}

#[derive(Debug, Deserialize)]
struct EventsResponse {
    #[serde(default)]
    items: Vec<GoogleEvent>,
    #[serde(rename = "nextPageToken")]
    next_page_token: Option<String>,
}

#[derive(Debug, Deserialize)]
struct GoogleEvent {
    #[serde(rename = "iCalUID")]
    ical_uid: Option<String>,
    #[serde(rename = "eventType")]
    event_type: Option<String>,
    summary: Option<String>,
    status: Option<String>,
    start: Option<EventDateTime>,
    end: Option<EventDateTime>,
    location: Option<String>,
    #[serde(rename = "hangoutLink")]
    hangout_link: Option<String>,
    #[serde(rename = "conferenceData")]
    conference_data: Option<ConferenceData>,
}

#[derive(Debug, Deserialize)]
struct EventDateTime {
    #[serde(rename = "dateTime")]
    date_time: Option<String>,
    date: Option<String>,
}

#[derive(Debug, Deserialize)]
struct ConferenceData {
    #[serde(rename = "entryPoints", default)]
    entry_points: Vec<EntryPoint>,
}

#[derive(Debug, Deserialize)]
struct EntryPoint {
    #[serde(rename = "entryPointType")]
    entry_point_type: Option<String>,
    uri: Option<String>,
}

#[derive(Debug)]
struct ApiStatusError {
    status: StatusCode,
    body: String,
}

impl fmt::Display for ApiStatusError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        let trimmed = self.body.trim();
        if trimmed.is_empty() {
            write!(formatter, "Google Calendar API returned {}", self.status)
        } else {
            write!(
                formatter,
                "Google Calendar API returned {}: {}",
                self.status, trimmed
            )
        }
    }
}

impl std::error::Error for ApiStatusError {}

pub async fn available_accounts() -> Result<Vec<GoaAccount>> {
    let connection = Connection::session()
        .await
        .context("failed to connect to the D-Bus session bus")?;
    discover_google_calendar_accounts(&connection).await
}

pub async fn fetch_accounts(options: FetchOptions<'_>) -> Result<Vec<CalendarEvent>> {
    let connection = Connection::session()
        .await
        .context("failed to connect to the D-Bus session bus")?;
    let accounts = discover_google_calendar_accounts(&connection).await?;
    let accounts = select_accounts(accounts, options.account_filters)?;

    if accounts.is_empty() {
        bail!(
            "No usable Google Calendar accounts found in GNOME Online Accounts. Add or fix a Google account in GNOME Settings > Online Accounts."
        );
    }

    let client = Client::new();
    let mut all_events = Vec::new();
    let mut successful_accounts = 0usize;
    let mut failures = Vec::new();

    for account in accounts {
        let account_label = account.label();
        match fetch_account_events(&connection, &client, &account, &options).await {
            Ok(events) => {
                successful_accounts += 1;
                all_events.extend(events);
            }
            Err(error) => {
                let message = format!("{account_label}: {error:#}");
                eprintln!("Skipping GOA account {message}");
                failures.push(message);
            }
        }
    }

    if successful_accounts == 0 {
        let detail = if failures.is_empty() {
            "No selected GOA account could be read.".to_string()
        } else {
            format!(
                "No selected GOA account could be read:\n{}",
                failures.join("\n")
            )
        };
        bail!(detail);
    }

    Ok(all_events)
}

async fn fetch_account_events(
    connection: &Connection,
    client: &Client,
    account: &GoaAccount,
    options: &FetchOptions<'_>,
) -> Result<Vec<CalendarEvent>> {
    let account_label = account.label();
    let access_token = get_goa_access_token(connection, account)
        .await
        .with_context(|| format!("failed to get a GOA access token for {account_label}"))?;

    match fetch_account_events_with_token(client, &access_token, &account_label, options).await {
        Ok(events) => Ok(events),
        Err(error) if is_google_auth_error(&error) => {
            let access_token = get_goa_access_token(connection, account)
                .await
                .with_context(|| {
                    format!("failed to refresh GOA access token for {account_label}")
                })?;
            fetch_account_events_with_token(client, &access_token, &account_label, options).await
        }
        Err(error) => Err(error),
    }
}

async fn fetch_account_events_with_token(
    client: &Client,
    access_token: &str,
    account_label: &str,
    options: &FetchOptions<'_>,
) -> Result<Vec<CalendarEvent>> {
    let calendars = list_calendars(client, access_token, options.all_calendars)
        .await
        .with_context(|| format!("failed to list calendars for GOA account {account_label}"))?;
    let mut events = Vec::new();

    for calendar in calendars {
        match list_events(client, access_token, &calendar, account_label, options).await {
            Ok(calendar_events) => events.extend(calendar_events),
            Err(error) if is_google_auth_error(&error) => return Err(error),
            Err(error) => eprintln!(
                "Skipping calendar '{}' for GOA account '{}': {error:#}",
                calendar.name, account_label
            ),
        }
    }

    Ok(events)
}

pub fn dedupe_events(events: Vec<CalendarEvent>) -> Vec<CalendarEvent> {
    let mut by_key: HashMap<String, CalendarEvent> = HashMap::new();

    for event in events {
        let key = event.dedupe_key();
        match by_key.get_mut(&key) {
            Some(existing) if event.rank_for_dedupe() > existing.rank_for_dedupe() => {
                *existing = event;
            }
            Some(_) => {}
            None => {
                by_key.insert(key, event);
            }
        }
    }

    let mut events = by_key.into_values().collect::<Vec<_>>();
    events.sort_by(sort_events);
    events
}

pub fn sort_events(a: &CalendarEvent, b: &CalendarEvent) -> Ordering {
    (
        a.start.date_naive(),
        !a.all_day,
        a.start.time(),
        normalize_text(&a.title),
    )
        .cmp(&(
            b.start.date_naive(),
            !b.all_day,
            b.start.time(),
            normalize_text(&b.title),
        ))
}

async fn discover_google_calendar_accounts(connection: &Connection) -> Result<Vec<GoaAccount>> {
    let object_manager = ObjectManagerProxy::new(connection, GOA_SERVICE, GOA_ROOT)
        .await
        .context("failed to contact GNOME Online Accounts")?;
    let objects = object_manager
        .get_managed_objects()
        .await
        .context("failed to enumerate GNOME Online Accounts")?;
    let mut accounts = Vec::new();

    for (path, interfaces) in objects {
        let has_interface = |name: &str| {
            interfaces
                .keys()
                .any(|interface| interface.as_str() == name)
        };
        if !(has_interface(IFACE_ACCOUNT)
            && has_interface(IFACE_CALENDAR)
            && has_interface(IFACE_OAUTH2))
        {
            continue;
        }

        let path_for_proxy = path.to_string();
        let account = Proxy::new(
            connection,
            GOA_SERVICE,
            path_for_proxy.as_str(),
            IFACE_ACCOUNT,
        )
        .await
        .with_context(|| format!("failed to inspect GOA account at {path}"))?;

        let provider_type: String = account.get_property("ProviderType").await?;
        if provider_type != "google" {
            continue;
        }

        let calendar_disabled = account
            .get_property::<bool>("CalendarDisabled")
            .await
            .unwrap_or(false);
        let attention_needed = account
            .get_property::<bool>("AttentionNeeded")
            .await
            .unwrap_or(false);
        let is_locked = account
            .get_property::<bool>("IsLocked")
            .await
            .unwrap_or(false);

        if calendar_disabled || attention_needed || is_locked {
            continue;
        }

        let id = account.get_property("Id").await.unwrap_or_default();
        let identity = account.get_property("Identity").await.unwrap_or_default();
        let presentation_identity = account
            .get_property("PresentationIdentity")
            .await
            .unwrap_or_default();
        drop(account);

        accounts.push(GoaAccount {
            path,
            id,
            identity,
            presentation_identity,
        });
    }

    accounts.sort_by_key(|account| account.label().to_lowercase());
    Ok(accounts)
}

pub fn select_accounts(accounts: Vec<GoaAccount>, filters: &[String]) -> Result<Vec<GoaAccount>> {
    let filters = filters
        .iter()
        .map(|filter| filter.trim())
        .filter(|filter| !filter.is_empty())
        .collect::<Vec<_>>();

    if filters.is_empty() {
        return Ok(accounts);
    }

    let mut unique = HashMap::<String, GoaAccount>::new();

    for filter in filters {
        let exact = accounts
            .iter()
            .filter(|account| account_matches_exact(account, filter))
            .cloned()
            .collect::<Vec<_>>();
        let matching = if exact.is_empty() {
            accounts
                .iter()
                .filter(|account| account_contains_filter(account, filter))
                .cloned()
                .collect::<Vec<_>>()
        } else {
            exact
        };

        if matching.is_empty() {
            bail!(
                "No GOA Google Calendar account matched '{filter}'. Available: {}",
                available_account_labels(&accounts)
            );
        }

        if matching.len() > 1 {
            bail!(
                "GOA account filter '{filter}' matched multiple accounts: {}. Use the exact email, GOA id, or object path.",
                matching
                    .iter()
                    .map(|account| account.label())
                    .collect::<Vec<_>>()
                    .join(", ")
            );
        }

        if let Some(account) = matching.into_iter().next() {
            unique.insert(account.path().to_string(), account);
        }
    }

    let mut accounts = unique.into_values().collect::<Vec<_>>();
    accounts.sort_by_key(|account| account.label().to_lowercase());
    Ok(accounts)
}

fn account_matches_exact(account: &GoaAccount, filter: &str) -> bool {
    [
        account.id.as_str(),
        account.identity.as_str(),
        account.presentation_identity.as_str(),
        account.path().as_str(),
    ]
    .iter()
    .any(|value| value.eq_ignore_ascii_case(filter))
}

fn account_contains_filter(account: &GoaAccount, filter: &str) -> bool {
    let filter = filter.to_lowercase();
    [
        account.id.as_str(),
        account.identity.as_str(),
        account.presentation_identity.as_str(),
    ]
    .iter()
    .any(|value| value.to_lowercase().contains(&filter))
}

fn available_account_labels(accounts: &[GoaAccount]) -> String {
    if accounts.is_empty() {
        "none".to_string()
    } else {
        accounts
            .iter()
            .map(|account| account.label())
            .collect::<Vec<_>>()
            .join(", ")
    }
}

async fn get_goa_access_token(connection: &Connection, account: &GoaAccount) -> Result<String> {
    let account_proxy = Proxy::new(
        connection,
        GOA_SERVICE,
        account.path().as_str(),
        IFACE_ACCOUNT,
    )
    .await
    .with_context(|| format!("failed to create GOA account proxy for {}", account.label()))?;

    let _: i32 = account_proxy
        .call("EnsureCredentials", &())
        .await
        .with_context(|| format!("GOA credentials need attention for {}", account.label()))?;

    let oauth_proxy = Proxy::new(
        connection,
        GOA_SERVICE,
        account.path().as_str(),
        IFACE_OAUTH2,
    )
    .await
    .with_context(|| format!("failed to create GOA OAuth2 proxy for {}", account.label()))?;
    let (access_token, _expires_in): (String, i32) = oauth_proxy
        .call("GetAccessToken", &())
        .await
        .with_context(|| format!("GOA did not return an access token for {}", account.label()))?;

    if access_token.is_empty() {
        return Err(anyhow!(
            "GOA returned an empty access token for {}",
            account.label()
        ));
    }

    Ok(access_token)
}

async fn list_calendars(
    client: &Client,
    bearer: &str,
    all_calendars: bool,
) -> Result<Vec<CalendarRef>> {
    let mut calendars = Vec::new();
    let mut page_token = None::<String>;

    loop {
        let mut query = vec![
            ("maxResults".to_string(), "250".to_string()),
            ("minAccessRole".to_string(), "reader".to_string()),
        ];

        if let Some(token) = &page_token {
            query.push(("pageToken".to_string(), token.clone()));
        }

        let response: CalendarListResponse = get_json(
            client,
            bearer,
            &format!("{API_ROOT}/users/me/calendarList"),
            &query,
        )
        .await?;

        calendars.extend(response.items.into_iter().filter_map(|item| {
            let id = item.id?;
            let can_read_events = item.access_role.as_deref() != Some("freeBusyReader");
            let selected = all_calendars || item.selected.unwrap_or(true);

            if !can_read_events || !selected {
                return None;
            }

            Some(CalendarRef {
                id,
                name: item
                    .summary_override
                    .or(item.summary)
                    .unwrap_or_else(|| "Calendar".to_string()),
                primary: item.primary.unwrap_or(false),
            })
        }));

        page_token = response.next_page_token;
        if page_token.is_none() {
            break;
        }
    }

    Ok(calendars)
}

async fn list_events(
    client: &Client,
    bearer: &str,
    calendar: &CalendarRef,
    account_label: &str,
    options: &FetchOptions<'_>,
) -> Result<Vec<CalendarEvent>> {
    let now = Local::now();
    let time_max = now + Duration::days(options.fetch_days);
    let encoded_id = urlencoding::encode(&calendar.id);
    let url = format!("{API_ROOT}/calendars/{encoded_id}/events");
    let mut events = Vec::new();
    let mut page_token = None::<String>;

    loop {
        let mut query = vec![
            ("singleEvents".to_string(), "true".to_string()),
            ("orderBy".to_string(), "startTime".to_string()),
            ("showDeleted".to_string(), "false".to_string()),
            ("timeMin".to_string(), now.to_rfc3339()),
            ("timeMax".to_string(), time_max.to_rfc3339()),
            (
                "maxResults".to_string(),
                options.max_results_per_calendar.to_string(),
            ),
        ];

        if let Some(token) = &page_token {
            query.push(("pageToken".to_string(), token.clone()));
        }

        let response: EventsResponse = get_json(client, bearer, &url, &query).await?;

        events.extend(
            response
                .items
                .into_iter()
                .filter_map(|event| convert_event(event, calendar, account_label)),
        );

        page_token = response.next_page_token;
        if page_token.is_none() {
            break;
        }
    }

    Ok(events)
}

async fn get_json<T: DeserializeOwned>(
    client: &Client,
    bearer: &str,
    url: &str,
    query: &[(String, String)],
) -> Result<T> {
    let response = client
        .get(url)
        .bearer_auth(bearer)
        .query(query)
        .send()
        .await?;

    let status = response.status();
    if !status.is_success() {
        let body = response.text().await.unwrap_or_else(|_| String::new());
        return Err(api_error(status, body));
    }

    Ok(response.json().await?)
}

fn api_error(status: StatusCode, body: String) -> anyhow::Error {
    ApiStatusError { status, body }.into()
}

fn is_google_auth_error(error: &anyhow::Error) -> bool {
    error
        .chain()
        .filter_map(|cause| cause.downcast_ref::<ApiStatusError>())
        .any(|error| {
            matches!(
                error.status,
                StatusCode::UNAUTHORIZED | StatusCode::FORBIDDEN
            )
        })
}

fn convert_event(
    event: GoogleEvent,
    calendar: &CalendarRef,
    account_label: &str,
) -> Option<CalendarEvent> {
    if event.status.as_deref() == Some("cancelled") {
        return None;
    }

    let start = event.start.as_ref()?;
    let (start, all_day) = parse_event_time(start)?;
    let end = event
        .end
        .as_ref()
        .and_then(|end| parse_event_time(end).map(|time| time.0));

    let title = event
        .summary
        .filter(|summary| !summary.trim().is_empty())
        .unwrap_or_else(|| "(untitled)".to_string());
    let location = event
        .location
        .filter(|location| !location.trim().is_empty());
    let has_meet = event.hangout_link.is_some()
        || location
            .as_deref()
            .map(|location| location.contains("meet.google.com"))
            .unwrap_or(false)
        || event
            .conference_data
            .as_ref()
            .map(has_video_conference)
            .unwrap_or(false);

    Some(CalendarEvent {
        title: collapse_whitespace(&title),
        start,
        end,
        all_day,
        calendar_name: calendar.name.clone(),
        account: account_label.to_string(),
        location,
        has_meet,
        primary_calendar: calendar.primary,
        calendar_id: calendar.id.clone(),
        event_type: event.event_type,
        ical_uid: event.ical_uid,
        status: event.status,
    })
}

fn parse_event_time(value: &EventDateTime) -> Option<(DateTime<Local>, bool)> {
    if let Some(date_time) = &value.date_time {
        let parsed = DateTime::parse_from_rfc3339(date_time).ok()?;
        return Some((parsed.with_timezone(&Local), false));
    }

    let date = value.date.as_deref()?;
    let date = NaiveDate::parse_from_str(date, "%Y-%m-%d").ok()?;
    let midnight = date.and_hms_opt(0, 0, 0)?;
    let local = Local
        .from_local_datetime(&midnight)
        .earliest()
        .or_else(|| Local.from_local_datetime(&midnight).latest())?;

    Some((local, true))
}

fn has_video_conference(data: &ConferenceData) -> bool {
    data.entry_points.iter().any(|entry| {
        entry.entry_point_type.as_deref() == Some("video")
            || entry
                .uri
                .as_deref()
                .map(|uri| uri.contains("meet.google.com"))
                .unwrap_or(false)
    })
}

fn normalize_text(value: &str) -> String {
    collapse_whitespace(value).to_lowercase()
}

fn contains_any(values: &[&str], terms: &[&str]) -> bool {
    values.iter().any(|value| {
        let normalized = normalize_text(value);
        terms.iter().any(|term| normalized.contains(term))
    })
}

fn collapse_whitespace(value: &str) -> String {
    value.split_whitespace().collect::<Vec<_>>().join(" ")
}
