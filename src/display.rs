use chrono::NaiveDate;

use crate::calendar::CalendarEvent;

const DAY_LABEL_FORMAT: &str = "%a %b %-d";
const TIME_LABEL_FORMAT: &str = "%H:%M";

pub fn day_label(date: NaiveDate, today: NaiveDate) -> String {
    if date == today {
        "Today".to_string()
    } else if Some(date) == today.succ_opt() {
        "Tomorrow".to_string()
    } else {
        date.format(DAY_LABEL_FORMAT).to_string()
    }
}

pub fn compact_day_label(date: NaiveDate) -> String {
    date.format(DAY_LABEL_FORMAT).to_string()
}

pub fn time_label(event: &CalendarEvent) -> String {
    if event.all_day {
        if event.is_multi_day() {
            "multi".to_string()
        } else {
            "all-day".to_string()
        }
    } else {
        event.start.format(TIME_LABEL_FORMAT).to_string()
    }
}

pub fn format_duration(minutes: i64) -> String {
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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn day_label_uses_relative_names_for_today_and_tomorrow() {
        let today = NaiveDate::from_ymd_opt(2026, 6, 7).expect("valid date");

        assert_eq!(day_label(today, today), "Today");
        assert_eq!(
            day_label(today.succ_opt().expect("next day"), today),
            "Tomorrow"
        );
        assert_eq!(
            day_label(
                NaiveDate::from_ymd_opt(2026, 6, 9).expect("valid date"),
                today
            ),
            "Tue Jun 9"
        );
    }

    #[test]
    fn format_duration_compacts_hours_and_minutes() {
        assert_eq!(format_duration(45), "45m");
        assert_eq!(format_duration(60), "1h");
        assert_eq!(format_duration(95), "1h35");
    }
}
