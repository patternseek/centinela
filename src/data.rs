use crate::fileset::FileSetId;
use crate::monitor::MonitorId;
use crate::notifier::{NotifierId, NotifierMessage};
use chrono::offset::TimeZone;
use chrono::{DateTime, Datelike, Duration, Timelike, Utc, Weekday};
use linemux::Line;
use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::error::Error;
use std::fs;
use std::fs::File;
use std::io::Read;
use std::ops::Sub;
use std::path::{Path, PathBuf};
use std::sync::{Arc, RwLock};
use tokio::sync::mpsc::{channel, Sender};
use tokio::sync::RwLock as RwLock_Tokio;
use log::{error, log, info};


/// Counts and recent events for a single set of monitored files
#[derive(Serialize, Deserialize, Debug, Default)]
pub struct FileSetData {
    pub monitor_data: HashMap<MonitorId, MonitorData>,
}

/// Counts and recent events for a single monitor for a single set of monitored files
#[derive(Serialize, Deserialize, Default, Debug)]
pub struct MonitorData {
    pub counts: EventCounts,
    pub recent_events: Vec<Arc<RwLock<MonitorEvent>>>,
}

impl MonitorData {
    /// A line was received on a file that the associated monitor monitors,
    /// we receive it here in case there are previous events still awaiting subsequent lines
    pub(crate) fn receive_line(&mut self, line: &Line, source: &Path) {
        self.recent_events
            .iter_mut()
            // Get read locks
            .map(|lock| lock.write().expect("unpoisoned lock"))
            // Find events awaiting lines from this source
            .filter(|ev| ev.awaiting_lines > 0 && ev.awaiting_lines_from == source)
            .for_each(|mut ev| {
                ev.lines.push(LogLine {
                    date: Utc::now(),
                    line: line.line().to_string(),
                    is_event_line: false,
                });
                ev.awaiting_lines -= 1;
                //println!("Received line from {:?}", source);
            });
    }

    /// A Monitor matched a line so we receive it for storage
    pub(crate) async fn receive_event(
        &mut self,
        ev: MonitorEvent,
        keep_num_events: Option<usize>,
        notifier_ids: Option<Vec<NotifierId>>,
        notifiers_tx: std::sync::mpsc::SyncSender<NotifierMessage>,
    ) {
        // Optionally store the event
        let keep_num_events = match keep_num_events {
            None => 0,
            Some(keep_events) => {
                // Store
                self.recent_events.push(Arc::new(RwLock::new(ev)));
                keep_events
            }
        };
        self.trim(keep_num_events);
        self.counts.increment();

        // If there are notifiers...
        if let Some(notifier_ids) = notifier_ids {
            // Borrow ev back out of self.recent_events
            let ev_arc_mut = self
                .recent_events
                .last()
                .expect(
                    "Unable to get last element in Monitor.recent_events despite just adding one.",
                )
                .clone();
            // Spawn a thread that will wait for additional lines from the log, if configured, until
            // a timeout is reached, then send an event to the notifiers thread
            std::thread::spawn(move || {
                info!( "Started line waiter thread" );
                let mut done = false;
                while !done {
                    let ev = ev_arc_mut.read().expect("unpoisoned lock");
                    if ev.awaiting_lines > 0 && ev.notify_by > chrono::Utc::now() {
                        //println!("Waiting for {} lines...", &ev.awaiting_lines);
                        drop(ev);
                        std::thread::sleep(std::time::Duration::from_secs(1));
                    } else {
                        let ev_clone = ev.clone();
                        drop(ev);
                        // Notify
                        let _ = notifiers_tx
                            .send(NotifierMessage::NotifyEvent(notifier_ids.clone(), ev_clone));
                        done = true;
                    }
                }
                info!( "Ended line waiter thread" );
            });
        }
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

/// Keeps counts of monitor match events bucketed by various
/// time increments
#[derive(Clone, Serialize, Deserialize, Default, Debug)]
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

    /// Trim all event count types
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
        let week = Utc
            .isoywd(now.year(), now.iso_week().week(), Weekday::Mon)
            .and_hms(0, 0, 0);
        match self.weeks.get_mut(&week) {
            Some(weeks_count) => {
                *weeks_count += 1;
            }
            None => {
                self.weeks.insert(week, 1);
            }
        };

        // let month = now.date().and_hms(now.hour(), 0, 0);
        let month = Utc.ymd(now.year(), now.month(), 1).and_hms(0, 0, 0);
        match self.months.get_mut(&month) {
            Some(months_count) => {
                *months_count += 1;
            }
            None => {
                self.months.insert(month, 1);
            }
        };

        let year = Utc.ymd(now.year(), 1, 1).and_hms(0, 0, 0);
        match self.years.get_mut(&year) {
            Some(years_count) => {
                *years_count += 1;
            }
            None => {
                self.years.insert(year, 1);
            }
        };
    }
}

/// A particular monitor match event
#[derive(Serialize, Deserialize, Debug, Clone)]
pub struct MonitorEvent {
    /// Matching log lines
    pub lines: Vec<LogLine>,
    /// How many additional lines should be collected
    pub awaiting_lines: usize,
    /// Which files the lines we're waiting from should be found
    pub awaiting_lines_from: PathBuf,
    /// Timeout after which a notification will be sent even if we're still waiting for
    /// additional lines
    pub notify_by: DateTime<Utc>,
}

impl MonitorEvent {
    /// Get all stored lines for this event as markdown, highlighting the line containing the event itself
    pub(crate) fn get_lines_as_markdown(&self) -> String {
        "\n```".to_string()
            + self
                .lines
                .clone()
                .into_iter()
                .map(|i| {
                    if i.is_event_line {
                        let wrap = String::from_utf8(vec![
                            b'-';
                            if i.to_string().len() < 100 {
                                i.to_string().len()
                            } else {
                                100
                            }
                        ])
                        .expect("Failed to create wrapper text");
                        "\n".to_string()
                            + wrap.as_str()
                            + "\n"
                            + i.to_string().as_str()
                            + "\n"
                            + wrap.as_str()
                            + "\n"
                    } else {
                        i.to_string()
                    }
                })
                .collect::<Vec<String>>()
                .join("\n")
                .as_str()
            + "\n```\n"
    }
}

/// A single line from a log file
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

/// Messages that the data store task listens for
#[derive(Debug)]
pub(crate) enum DataStoreMessage {
    ReceiveLine(FileSetId, MonitorId, Line),
    ReceiveEvent(
        FileSetId,
        MonitorId,
        MonitorEvent,
        Option<usize>,
        Option<Vec<NotifierId>>,
    ),
    FileSeen(FileSetId, String),
    NotifyFilesSeen(Vec<NotifierId>),
    Persist,
    Shutdown,
}

/// Start the data store task.
/// This loops listening for events until it's instructed to shut down.
pub(crate) fn start_task(
    filesets_data_rwlock: Arc<RwLock_Tokio<HashMap<FileSetId, FileSetData>>>,
    mut files_last_seen_data: HashMap<FileSetId, HashMap<String, DateTime<Utc>>>,
    notifiers_tx: std::sync::mpsc::SyncSender<NotifierMessage>,
    data_file_path: String,
) -> Sender<DataStoreMessage> {
    let (tx, mut rx) = channel(32);
    tokio::spawn(async move {
        while let Some(message) = rx.recv().await {
            match message {
                DataStoreMessage::ReceiveLine(file_set_id, monitor_id, log_line) => {
                    let mut filesets_data = filesets_data_rwlock.write().await;
                    let monitor_data =
                        fetch_monitor_data(&mut filesets_data, &file_set_id, &monitor_id);
                    monitor_data.receive_line(&log_line, log_line.source());
                }
                DataStoreMessage::ReceiveEvent(
                    file_set_id,
                    monitor_id,
                    ev,
                    keep_num_events,
                    notifier_ids,
                ) => {
                    let mut filesets_data = filesets_data_rwlock.write().await;
                    let monitor_data =
                        fetch_monitor_data(&mut filesets_data, &file_set_id, &monitor_id);
                    monitor_data
                        .receive_event(ev, keep_num_events, notifier_ids, notifiers_tx.clone())
                        .await;
                }
                DataStoreMessage::FileSeen(fileset_id, file_path) => {
                    let inner = files_last_seen_data.entry(fileset_id).or_default();
                    inner.insert(file_path, Utc::now());
                }
                DataStoreMessage::NotifyFilesSeen(notifier_ids) => {
                    let message = "Files last seen: \n\n".to_string() + {
                        files_last_seen_data
                            .iter()
                            .map(|(k, v)| {
                                let mut inner_message_lines = v
                                    .iter()
                                    .map(|(inner_k, inner_v)| {
                                        format!(
                                            "\t{} : {}s ago",
                                            inner_k,
                                            (Utc::now().timestamp() - inner_v.timestamp())
                                        )
                                    })
                                    .collect::<Vec<String>>();
                                inner_message_lines.sort();
                                format!(
                                    "{}:\n{}",
                                    k,
                                    inner_message_lines.iter().fold(String::new(), |acc, line| {
                                        acc + line.as_str() + "\n"
                                    })
                                )
                            })
                            .fold(String::new(), |acc, line| acc + line.as_str() + "\n")
                            .as_str()
                    };

                    let _ =
                        notifiers_tx.send(NotifierMessage::NotifyMessage(notifier_ids, message));
                }
                DataStoreMessage::Persist => {
                    persist_data(&filesets_data_rwlock, data_file_path.as_str()).await
                }
                DataStoreMessage::Shutdown => break,
            }
        }
    });
    tx
}

/// Small helper for fetching specific monitor data
fn fetch_monitor_data<'a>(
    filesets_data: &'a mut HashMap<String, FileSetData>,
    file_set_id: &FileSetId,
    monitor_id: &MonitorId,
) -> &'a mut MonitorData {
    let fs_data = filesets_data
        .get_mut(file_set_id)
        .expect("Invalid fileset ID in DataStoreMessage::StoreLine message");
    let monitor_data = fs_data
        .monitor_data
        .get_mut(monitor_id)
        .expect("Invalid monitor ID in DataStoreMessage::StoreLine message");
    monitor_data
}

/// Save counts data to disk
async fn persist_data(
    filesets_data_rwlock: &Arc<RwLock_Tokio<HashMap<FileSetId, FileSetData>>>,
    data_file_path: &str,
) {
    let data = filesets_data_rwlock.read().await;
    let mut save_data: HashMap<FileSetId, HashMap<MonitorId, EventCounts>> = Default::default();
    for (fileset_id, fileset_data) in &data as &HashMap<FileSetId, FileSetData> {
        let mut fileset_counts: HashMap<MonitorId, EventCounts> = Default::default();
        for (monitor_id, monitor_data) in &fileset_data.monitor_data {
            let counts = monitor_data.counts.clone();
            fileset_counts.insert(monitor_id.clone(), counts);
        }
        save_data.insert(fileset_id.clone(), fileset_counts);
    }
    let data_str = serde_json::to_string(&save_data).expect("Failed to encode data-store to JSON");
    // Early drop to release the lock
    drop(data);
    match fs::write(data_file_path, data_str) {
        Ok(_) => println!("Wrote data file: {}", &data_file_path),
        Err(err) => println!("Error writing data file: {}", err),
    };
}

/// Load counts data from disk
pub(crate) fn load_data_from_file(
    data_file_path: &str,
) -> Result<HashMap<FileSetId, HashMap<MonitorId, EventCounts>>, Box<dyn Error>> {
    let mut file = File::open(data_file_path)?;
    let mut contents = String::new();
    file.read_to_string(&mut contents)?;
    let stats = serde_json::from_str(&contents)?;
    Ok(stats)
}
