use std::{
    cmp::Ordering, collections::HashMap, fmt, iter::Peekable, time::Duration as StdDuration,
};

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
/// Maximum accepted by the Google Calendar Events API for one page.
pub const GOOGLE_EVENTS_MAX_RESULTS_PER_PAGE: u32 = 2500;
/// Maximum accepted by the Google Calendar CalendarList API for one page.
const GOOGLE_CALENDAR_LIST_MAX_RESULTS: u32 = 250;
const HTTP_CONNECT_TIMEOUT_SECS: u64 = 10;
const HTTP_REQUEST_TIMEOUT_SECS: u64 = 30;
const HTTP_MAX_ATTEMPTS: usize = 3;
const HTTP_RETRY_BASE_DELAY_MS: u64 = 250;
const API_ERROR_BODY_CHAR_LIMIT: usize = 2048;
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
        let raw = if !self.presentation_identity.trim().is_empty() {
            self.presentation_identity.as_str()
        } else if !self.identity.trim().is_empty() {
            self.identity.as_str()
        } else {
            self.id.as_str()
        };

        let label = sanitize_display_text(raw);
        if label.is_empty() {
            "GOA account".to_string()
        } else {
            label
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
    body_summary: String,
}

impl fmt::Display for ApiStatusError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        let trimmed = self.body_summary.trim();
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

    let client = build_http_client()?;
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
    let mut successful_calendars = 0usize;
    let mut failures = Vec::new();

    for calendar in calendars {
        match list_events(client, access_token, &calendar, account_label, options).await {
            Ok(calendar_events) => {
                successful_calendars += 1;
                events.extend(calendar_events);
            }
            Err(error) if is_google_auth_error(&error) => return Err(error),
            Err(error) => {
                let message = format!("{}: {error:#}", calendar.name);
                eprintln!(
                    "Skipping calendar '{}' for GOA account '{}': {error:#}",
                    calendar.name, account_label
                );
                failures.push(message);
            }
        }
    }

    if successful_calendars == 0 && !failures.is_empty() {
        bail!(
            "No selected calendar could be read for GOA account {account_label}:\n{}",
            failures.join("\n")
        );
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

fn build_http_client() -> Result<Client> {
    Client::builder()
        .connect_timeout(StdDuration::from_secs(HTTP_CONNECT_TIMEOUT_SECS))
        .timeout(StdDuration::from_secs(HTTP_REQUEST_TIMEOUT_SECS))
        .build()
        .context("failed to build Google Calendar HTTP client")
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
            (
                "maxResults".to_string(),
                GOOGLE_CALENDAR_LIST_MAX_RESULTS.to_string(),
            ),
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

        calendars.extend(
            response
                .items
                .into_iter()
                .filter_map(|item| calendar_ref_from_entry(item, all_calendars)),
        );

        page_token = response.next_page_token;
        if page_token.is_none() {
            break;
        }
    }

    Ok(calendars)
}

fn calendar_ref_from_entry(item: CalendarListEntry, all_calendars: bool) -> Option<CalendarRef> {
    let id = item.id?;
    let primary = item.primary.unwrap_or(false);
    let can_read_events = item.access_role.as_deref() != Some("freeBusyReader");
    let selected = all_calendars || primary || item.selected.unwrap_or(false);

    if !can_read_events || !selected {
        return None;
    }

    let name = item
        .summary_override
        .or(item.summary)
        .map(|name| sanitize_display_text(&name))
        .filter(|name| !name.is_empty())
        .unwrap_or_else(|| "Calendar".to_string());

    Some(CalendarRef { id, name, primary })
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
    let mut attempt = 1usize;

    loop {
        let response = client
            .get(url)
            .bearer_auth(bearer)
            .query(query)
            .send()
            .await
            .context("failed to send Google Calendar API request")?;

        let status = response.status();
        if status.is_success() {
            return response
                .json()
                .await
                .context("failed to parse Google Calendar API response");
        }

        let body = response.text().await.unwrap_or_else(|_| String::new());
        if is_retryable_status(status) && attempt < HTTP_MAX_ATTEMPTS {
            tokio::time::sleep(StdDuration::from_millis(
                HTTP_RETRY_BASE_DELAY_MS * attempt as u64,
            ))
            .await;
            attempt += 1;
            continue;
        }

        return Err(api_error(status, body));
    }
}

fn api_error(status: StatusCode, body: String) -> anyhow::Error {
    ApiStatusError {
        status,
        body_summary: summarize_api_error_body(&body),
    }
    .into()
}

fn is_retryable_status(status: StatusCode) -> bool {
    status == StatusCode::TOO_MANY_REQUESTS || status.is_server_error()
}

fn summarize_api_error_body(body: &str) -> String {
    truncate_chars(&sanitize_display_text(body), API_ERROR_BODY_CHAR_LIMIT)
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

    let raw_title = event
        .summary
        .filter(|summary| !summary.trim().is_empty())
        .unwrap_or_else(|| "(untitled)".to_string());
    let title = sanitize_display_text(&raw_title);
    let title = if title.is_empty() {
        "(untitled)".to_string()
    } else {
        title
    };
    let raw_location = event.location;
    let has_meet = event.hangout_link.is_some()
        || raw_location
            .as_deref()
            .map(|location| location.contains("meet.google.com"))
            .unwrap_or(false)
        || event
            .conference_data
            .as_ref()
            .map(has_video_conference)
            .unwrap_or(false);
    let location = raw_location
        .map(|location| sanitize_display_text(&location))
        .filter(|location| !location.is_empty());

    Some(CalendarEvent {
        title,
        start,
        end,
        all_day,
        calendar_name: sanitize_display_text(&calendar.name),
        account: sanitize_display_text(account_label),
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
    sanitize_display_text(value).to_lowercase()
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

pub(crate) fn sanitize_display_text(value: &str) -> String {
    collapse_whitespace(&strip_terminal_controls(value))
}

fn strip_terminal_controls(value: &str) -> String {
    let mut chars = value.chars().peekable();
    let mut out = String::with_capacity(value.len());

    while let Some(ch) = chars.next() {
        if ch == '\u{1b}' {
            strip_escape_sequence(&mut chars);
        } else if is_bidi_control(ch) {
            continue;
        } else if ch.is_control() {
            if ch.is_whitespace() {
                out.push(' ');
            }
        } else {
            out.push(ch);
        }
    }

    out
}

fn strip_escape_sequence<I>(chars: &mut Peekable<I>)
where
    I: Iterator<Item = char>,
{
    match chars.peek().copied() {
        Some('[') => {
            chars.next();
            for ch in chars.by_ref() {
                if ('@'..='~').contains(&ch) {
                    break;
                }
            }
        }
        Some(']') | Some('P') | Some('_') | Some('^') | Some('X') => {
            chars.next();
            strip_until_string_terminator(chars);
        }
        Some(_) => {
            chars.next();
        }
        None => {}
    }
}

fn strip_until_string_terminator<I>(chars: &mut Peekable<I>)
where
    I: Iterator<Item = char>,
{
    let mut saw_escape = false;

    for ch in chars.by_ref() {
        if ch == '\u{7}' {
            break;
        }

        if saw_escape && ch == '\\' {
            break;
        }

        saw_escape = ch == '\u{1b}';
    }
}

fn is_bidi_control(ch: char) -> bool {
    matches!(
        ch,
        '\u{061c}'
            | '\u{200e}'
            | '\u{200f}'
            | '\u{202a}'..='\u{202e}'
            | '\u{2066}'..='\u{2069}'
    )
}

fn truncate_chars(value: &str, max_chars: usize) -> String {
    let mut chars = value.chars();
    let mut truncated = chars.by_ref().take(max_chars).collect::<String>();
    if chars.next().is_some() {
        truncated.push('…');
    }
    truncated
}

#[cfg(test)]
pub(crate) fn test_event(title: &str, start: DateTime<Local>) -> CalendarEvent {
    CalendarEvent {
        title: title.to_string(),
        start,
        end: Some(start + Duration::hours(1)),
        all_day: false,
        calendar_name: "Work".to_string(),
        account: "work@example.com".to_string(),
        location: None,
        has_meet: false,
        primary_calendar: false,
        calendar_id: "calendar-id".to_string(),
        event_type: None,
        ical_uid: None,
        status: Some("confirmed".to_string()),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use chrono::{LocalResult, TimeZone};

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

    fn goa_account(path: &str, id: &str, identity: &str, presentation: &str) -> GoaAccount {
        GoaAccount {
            path: OwnedObjectPath::try_from(path).expect("valid GOA object path"),
            id: id.to_string(),
            identity: identity.to_string(),
            presentation_identity: presentation.to_string(),
        }
    }

    fn calendar_entry(
        selected: Option<bool>,
        primary: Option<bool>,
        access_role: Option<&str>,
    ) -> CalendarListEntry {
        CalendarListEntry {
            id: Some("calendar-id".to_string()),
            summary: Some("Calendar".to_string()),
            summary_override: None,
            primary,
            selected,
            access_role: access_role.map(ToOwned::to_owned),
        }
    }

    #[test]
    fn sanitize_display_text_strips_terminal_controls_and_bidi_marks() {
        let value = "Title\x1b[31m red\x1b[0m \x1b]52;c;secret\x07\u{202e}\nnext";

        assert_eq!(sanitize_display_text(value), "Title red next");
    }

    #[test]
    fn calendar_list_filter_hides_unselected_calendars_by_default() {
        assert!(
            calendar_ref_from_entry(calendar_entry(None, Some(false), Some("reader")), false)
                .is_none()
        );
        assert!(
            calendar_ref_from_entry(
                calendar_entry(Some(false), Some(false), Some("owner")),
                false
            )
            .is_none()
        );
        assert!(
            calendar_ref_from_entry(
                calendar_entry(Some(true), Some(false), Some("reader")),
                false
            )
            .is_some()
        );
        assert!(
            calendar_ref_from_entry(calendar_entry(None, Some(true), Some("owner")), false)
                .is_some()
        );
        assert!(
            calendar_ref_from_entry(
                calendar_entry(Some(false), Some(false), Some("reader")),
                true
            )
            .is_some()
        );
        assert!(
            calendar_ref_from_entry(
                calendar_entry(Some(true), Some(false), Some("freeBusyReader")),
                true
            )
            .is_none()
        );
    }

    #[test]
    fn select_accounts_prefers_exact_matches_and_rejects_ambiguous_filters() {
        let accounts = vec![
            goa_account(
                "/org/gnome/OnlineAccounts/Accounts/account_1",
                "account_1",
                "personal@example.com",
                "Personal",
            ),
            goa_account(
                "/org/gnome/OnlineAccounts/Accounts/account_2",
                "account_2",
                "work@example.com",
                "Work",
            ),
        ];

        let exact = select_accounts(accounts.clone(), &["work@example.com".to_string()])
            .expect("exact filter should match");
        assert_eq!(exact.len(), 1);
        assert_eq!(exact[0].identity, "work@example.com");

        let ambiguous = select_accounts(accounts, &["example".to_string()]);
        assert!(ambiguous.is_err());
    }

    #[test]
    fn convert_event_sanitizes_external_display_strings() {
        let event = GoogleEvent {
            ical_uid: Some("uid-1".to_string()),
            event_type: None,
            summary: Some("Team\x1b[31m Red\x1b[0m\u{202e}\nSync".to_string()),
            status: Some("confirmed".to_string()),
            start: Some(EventDateTime {
                date_time: Some("2026-06-07T09:00:00Z".to_string()),
                date: None,
            }),
            end: Some(EventDateTime {
                date_time: Some("2026-06-07T10:00:00Z".to_string()),
                date: None,
            }),
            location: Some("Room\tA\x1b]52;c;secret\x07".to_string()),
            hangout_link: None,
            conference_data: Some(ConferenceData {
                entry_points: vec![EntryPoint {
                    entry_point_type: Some("video".to_string()),
                    uri: None,
                }],
            }),
        };
        let calendar = CalendarRef {
            id: "calendar-id".to_string(),
            name: "Work\x1b[31m".to_string(),
            primary: true,
        };

        let converted = convert_event(event, &calendar, "work@example.com\x1b[0m")
            .expect("valid event should convert");

        assert_eq!(converted.title, "Team Red Sync");
        assert_eq!(converted.calendar_name, "Work");
        assert_eq!(converted.account, "work@example.com");
        assert_eq!(converted.location.as_deref(), Some("Room A"));
        assert!(converted.has_meet);
    }

    #[test]
    fn parse_event_time_handles_timed_and_all_day_values() {
        let timed = EventDateTime {
            date_time: Some("2026-06-07T09:30:00Z".to_string()),
            date: None,
        };
        let (timed_start, timed_all_day) = parse_event_time(&timed).expect("valid RFC3339");
        assert!(!timed_all_day);
        let expected = DateTime::parse_from_rfc3339("2026-06-07T09:30:00Z")
            .expect("valid RFC3339")
            .timestamp();
        assert_eq!(timed_start.timestamp(), expected);

        let all_day = EventDateTime {
            date_time: None,
            date: Some("2026-06-07".to_string()),
        };
        let (all_day_start, all_day_flag) = parse_event_time(&all_day).expect("valid all-day date");
        assert!(all_day_flag);
        assert_eq!(
            all_day_start.date_naive(),
            NaiveDate::from_ymd_opt(2026, 6, 7).expect("valid date")
        );
    }

    #[test]
    fn dedupe_events_prefers_confirmed_primary_events_with_details() {
        let start = local_datetime(2026, 6, 7, 9, 0);
        let mut lower_ranked = test_event("Team Sync", start);
        lower_ranked.ical_uid = Some("uid-1".to_string());
        lower_ranked.status = Some("tentative".to_string());

        let mut higher_ranked = test_event("Team Sync", start);
        higher_ranked.ical_uid = Some("uid-1".to_string());
        higher_ranked.primary_calendar = true;
        higher_ranked.has_meet = true;
        higher_ranked.calendar_name = "Primary".to_string();

        let deduped = dedupe_events(vec![lower_ranked, higher_ranked]);

        assert_eq!(deduped.len(), 1);
        assert_eq!(deduped[0].calendar_name, "Primary");
        assert!(deduped[0].has_meet);
        assert!(deduped[0].primary_calendar);
    }

    #[test]
    fn api_error_body_is_sanitized_and_bounded() {
        let body = format!(
            "{}\x1b[31msecret\x1b[0m",
            "x".repeat(API_ERROR_BODY_CHAR_LIMIT + 10)
        );
        let summary = summarize_api_error_body(&body);

        assert!(summary.chars().count() <= API_ERROR_BODY_CHAR_LIMIT + 1);
        assert!(!summary.contains("\x1b"));
        assert!(summary.ends_with('…'));
    }
}
