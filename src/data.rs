use chrono::{DateTime, Duration, Timelike, Utc};
use linemux::Line;
use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::ops::Sub;
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};

#[derive(Serialize, Deserialize, Debug)]
pub struct FileSetData {
    pub monitor_data: HashMap<String, MonitorData>,
}

impl Default for FileSetData {
    fn default() -> Self {
        FileSetData {
            monitor_data: HashMap::new(),
        }
    }
}

#[derive(Serialize, Deserialize, Default, Debug)]
pub struct MonitorData {
    pub counts: EventCounts,
    pub recent_events: Vec<Arc<Mutex<MonitorEvent>>>,
}

impl MonitorData {
    /// A line was received on a file that the associated monitor monitors,
    /// we receive it here in case there are previous events still awaiting subsequent lines
    pub(crate) fn receive_line(&mut self, line: &Line, source: &Path) {
        self.recent_events
            .iter_mut()
            // Get read locks
            .map(|lock| lock.lock().expect("unpoisoned lock"))
            // Find events awaiting lines from this source
            .filter(|ev| ev.awaiting_lines > 0 && ev.awaiting_lines_from == source)
            .for_each(|mut ev| {
                ev.lines.push(LogLine {
                    date: chrono::offset::Utc::now(),
                    line: line.line().to_string(),
                    is_event_line: false,
                });
                ev.awaiting_lines -= 1;
                println!("Received line from {:?}", source);
            });
    }

    /// A Monitor matched a line so we receive it for storage
    pub(crate) fn receive_event(
        &mut self,
        evlock: Arc<Mutex<MonitorEvent>>,
        keep_num_events: Option<usize>,
    ) {
        let keep_num_events = match keep_num_events {
            None => 0,
            Some(keep_events) => {
                // Store
                self.recent_events.push(evlock);
                keep_events
            }
        };
        self.trim(keep_num_events);
        self.counts.increment();
    }

    /// Remove older events if we have more than keep_num_events
    fn trim(&mut self, keep_num_events: usize) {
        if self.recent_events.len() > keep_num_events {
            self.recent_events
                .drain(0..=(self.recent_events.len() - keep_num_events));
        }
        self.counts.trim_all();
    }
}

#[derive(Serialize, Deserialize, Default, Debug)]
pub struct EventCounts {
    pub seconds: HashMap<DateTime<Utc>, usize>,
    pub minutes: HashMap<DateTime<Utc>, usize>,
    pub hours: HashMap<DateTime<Utc>, usize>,
    pub days: HashMap<DateTime<Utc>, usize>,
    pub weeks: HashMap<DateTime<Utc>, usize>,
    pub months: HashMap<DateTime<Utc>, usize>,
    pub years: HashMap<DateTime<Utc>, usize>,
}

impl EventCounts {
    const KEEP_SECONDS: usize = 60 * 60;
    const KEEP_MINUTES: usize = 60 * 24;
    const KEEP_HOURS: usize = 24 * 7;
    const KEEP_DAYS: usize = 7 * 4;
    const KEEP_WEEKS: usize = 52;
    const KEEP_MONTHS: usize = 48;
    const KEEP_YEARS: usize = 10;

    // Trim all event count types
    fn trim_all(&mut self) {
        EventCounts::trim_older(&mut self.seconds, EventCounts::KEEP_SECONDS);
        EventCounts::trim_older(&mut self.minutes, EventCounts::KEEP_MINUTES * 60);
        EventCounts::trim_older(&mut self.hours, EventCounts::KEEP_HOURS * 60 * 60);
        EventCounts::trim_older(&mut self.days, EventCounts::KEEP_DAYS * 60 * 60 * 24);
        EventCounts::trim_older(&mut self.weeks, EventCounts::KEEP_WEEKS * 60 * 60 * 24 * 7);
        EventCounts::trim_older(
            &mut self.months,
            EventCounts::KEEP_MONTHS * 60 * 60 * 24 * 31,
        );
        EventCounts::trim_older(
            &mut self.years,
            EventCounts::KEEP_YEARS * 60 * 60 * 24 * 365,
        );
    }

    /// Trim old event counts of a particular type
    fn trim_older(items: &mut HashMap<DateTime<Utc>, usize>, keep_seconds: usize) {
        let now = chrono::offset::Utc::now();
        items.retain(|k, _v| *k >= now.sub(Duration::seconds(keep_seconds as i64)));
    }

    /// Increment all current counters
    fn increment(&mut self) {
        let now = chrono::offset::Utc::now();

        let seconds = now.date().and_hms(now.hour(), now.minute(), now.second());
        match self.seconds.get_mut(&seconds) {
            Some(seconds_count) => {
                *seconds_count += 1;
            }
            None => {
                self.seconds.insert(seconds, 1);
            }
        };

        let minutes = now.date().and_hms(now.hour(), now.minute(), 0);
        match self.minutes.get_mut(&minutes) {
            Some(minutes_count) => {
                *minutes_count += 1;
            }
            None => {
                self.minutes.insert(minutes, 1);
            }
        };

        let hours = now.date().and_hms(now.hour(), 0, 0);
        match self.hours.get_mut(&hours) {
            Some(hours_count) => {
                *hours_count += 1;
            }
            None => {
                self.hours.insert(hours, 1);
            }
        };

        let days = now.date().and_hms(0, 0, 0);
        match self.days.get_mut(&days) {
            Some(days_count) => {
                *days_count += 1;
            }
            None => {
                self.days.insert(days, 1);
            }
        };

        let weeks = now.date().and_hms(now.hour(), 0, 0);
        match self.weeks.get_mut(&weeks) {
            Some(weeks_count) => {
                *weeks_count += 1;
            }
            None => {
                self.weeks.insert(weeks, 1);
            }
        };

        let months = now.date().and_hms(now.hour(), 0, 0);
        match self.months.get_mut(&months) {
            Some(months_count) => {
                *months_count += 1;
            }
            None => {
                self.months.insert(months, 1);
            }
        };

        let years = now.date().and_hms(now.hour(), 0, 0);
        match self.years.get_mut(&years) {
            Some(years_count) => {
                *years_count += 1;
            }
            None => {
                self.years.insert(years, 1);
            }
        };
    }
}

#[derive(Serialize, Deserialize, Debug)]
pub struct MonitorEvent {
    pub lines: Vec<LogLine>,
    pub awaiting_lines: usize,
    pub awaiting_lines_from: PathBuf,
}

impl MonitorEvent {
    /// Get all stored lines for this event as markdown, highlighting the line containing the event itself
    pub(crate) fn get_lines_as_markdown(&self) -> String {
        self.lines
            .clone()
            .into_iter()
            .map(|i| {
                if i.is_event_line {
                    "\n\n**".to_string() + i.to_string().as_str() + "**\n\n"
                } else {
                    i.to_string()
                }
            })
            .collect::<Vec<String>>()
            .join("\n")
    }
}

#[derive(Serialize, Deserialize, Debug, Clone)]
pub struct LogLine {
    pub date: DateTime<Utc>,
    pub line: String,
    pub is_event_line: bool,
}

impl ToString for LogLine {
    fn to_string(&self) -> String {
        self.date.to_string() + " " + self.line.as_str()
    }
}
