use futures::future::{join_all, BoxFuture};
use futures::prelude::*;
use regex::Regex;
use std::collections::HashMap;

use lib::*;
use std::error::Error;
use url::Url;

mod lib {
    use chrono::{DateTime, Timelike, Utc};
    use glob::glob as glob_parser;
    use linemux::{Line, MuxedLines};
    use regex::Regex;
    use serde::{Deserialize, Serialize};
    use std::collections::{HashMap, VecDeque};
    use std::error::Error;
    use std::path::PathBuf;
    use std::process::exit;
    use url::Url;

    #[derive(Serialize, Deserialize)]
    pub(crate) struct ConfigFile {
        pub(crate) file_sets: HashMap<String, FileSet>,
    }

    #[derive(Serialize, Deserialize)]
    pub(crate) struct FileSet {
        pub(crate) file_globs: Vec<String>,
        pub(crate) monitors: HashMap<String, Monitor>,
        #[serde(default, skip_serializing)]
        pub(crate) line_buffers_before: HashMap<PathBuf, VecDeque<LogLine>>,
        #[serde(default, skip_serializing)]
        pub(crate) max_lines_before: usize,
        #[serde(default, skip_serializing)]
        pub(crate) max_lines_after: usize,
        #[serde(default, skip_serializing)]
        pub(crate) data: FileSetData,
    }

    impl FileSet {
        pub(crate) async fn follow(&mut self, file_set_name: &str) -> Result<(), Box<dyn Error>> {
            let mut line_follower = match MuxedLines::new() {
                Ok(lf) => lf,
                Err(e) => return Err(Box::new(e)),
            };
            for glob in &self.file_globs {
                for entry in glob_parser(&glob)
                    .expect(format!("Failed to read glob pattern: {}", glob).as_ref())
                {
                    match &entry {
                        Ok(path) => {
                            if let Err(e) = line_follower.add_file(path).await {
                                // Typically something like a file perm issue
                                eprintln!("File error for {:?} {}", &path, e);
                                exit(1);
                            };
                        }
                        Err(e) => {
                            // Typically something like a directory perm issue
                            eprintln!("File error for {} {}", glob, e);
                            exit(1);
                        }
                    };
                }
            }
            for (_monitor_name, monitor) in &self.monitors {
                // Calculate the number of previous lines to keep per file
                if let Some(keep) = monitor.keep_lines_before {
                    if keep > self.max_lines_before {
                        self.max_lines_before = keep;
                    }
                }
                // Calculate the number of subsequent lines to keep per file
                if let Some(keep) = monitor.keep_lines_after {
                    if keep > self.max_lines_after {
                        self.max_lines_after = keep;
                    }
                }
            }

            Ok(self.line_handler(line_follower, file_set_name).await)
        }

        /// Watch the lines generated for a set of files
        async fn line_handler(&mut self, mut line_follower: MuxedLines, file_set_name: &str) {
            // For each line received from a set of files
            while let Ok(Some(line)) = line_follower.next_line().await {
                // Check for monitor matches
                for (monitor_name, monitor) in &mut self.monitors {
                    // Initialise the data entry for this monitor if necessary
                    if self.data.monitor_data.get(monitor_name).is_none() {
                        self.data
                            .monitor_data
                            .insert(monitor_name.clone(), Default::default());
                    }
                    // FIXME Want to pass the line the appropriate events that are awaiting responses here.
                    // FIXME previously though i'd have to keep a list of those that are awaiting subsequent lines
                    // FIXME but actually we're unlikely to keep more than 10? 30? 100? events per monitor, in which case
                    // FIXME iterating them is trivial

                    // Pass the line to the monitor for testing and possibly processing
                    if let Some(ev) = monitor.handle_line(
                        &file_set_name,
                        &line,
                        self.line_buffers_before.get(line.source()),
                    ) {
                        // Unwrap is ok because the item is inserted above if it doesn't exist
                        self.data
                            .monitor_data
                            .get_mut(monitor_name)
                            .unwrap()
                            .receive_event(ev);
                        println!("Current data: {:#?}", &self.data);
                    };
                }
                self.buffer_line(&line);
            }
        }

        /// Store a copy of a log line so that it be be used as part of the previous lines for an event
        fn buffer_line(&mut self, line: &Line) {
            if self.line_buffers_before.get(line.source()).is_none() {
                self.line_buffers_before
                    .insert(line.source().to_owned(), VecDeque::new());
            }
            if let Some(buf) = self.line_buffers_before.get_mut(line.source()) {
                buf.push_back(LogLine {
                    date: chrono::offset::Utc::now(),
                    line: line.line().to_string(),
                });
                while buf.len() > self.max_lines_before {
                    buf.pop_front();
                }
            }
        }
    }

    #[derive(Serialize, Deserialize)]
    pub(crate) struct Monitor {
        #[serde(with = "serde_regex")]
        pub(crate) regex: Regex,
        pub(crate) log_recent_events: Option<usize>,
        pub(crate) keep_lines_before: Option<usize>,
        pub(crate) keep_lines_after: Option<usize>,
        pub(crate) log_counts: bool,
        pub(crate) notifiers: Option<Vec<Notifier>>,
    }

    impl Monitor {
        fn handle_line(
            &mut self,
            file_set_name: &&str,
            line: &Line,
            previous_lines: Option<&VecDeque<LogLine>>,
        ) -> Option<MonitorEvent> {
            if self.regex.is_match(line.line()) {
                // Log line in question
                let log_line = LogLine {
                    date: chrono::offset::Utc::now(),
                    line: line.line().to_string(),
                };

                // Get previous lines if appropriate
                let lines = match self.keep_lines_before {
                    Some(keep_lines) => match previous_lines {
                        Some(previous_lines) => {
                            let mut subset = previous_lines
                                .range((previous_lines.len() - keep_lines)..)
                                .cloned()
                                .collect::<Vec<LogLine>>();
                            subset.push(log_line);
                            subset
                        }
                        None => vec![log_line],
                    },
                    None => vec![log_line],
                };

                // Create a new match event
                let ev = MonitorEvent {
                    lines: lines,
                    awaiting_lines: match self.keep_lines_after {
                        Some(num_awaiting) => num_awaiting,
                        None => 0,
                    },
                    awaiting_lines_from: line.source().to_owned(),
                };
                println!("Generated event {:#?}", &ev);
                Some(ev)
            } else {
                // This prints nerp for some and not others because the monitors aren't all watching the same files
                println!("Nerp {}", &file_set_name,);
                None
            }
        }
    }

    #[derive(Serialize, Deserialize)]
    pub(crate) enum Notifier {
        Slack {
            token: String,
            template: String,
        },
        Mattermost {
            url: Url,
            api_key: String,
            template: String,
        },
    }

    #[derive(Serialize, Deserialize, Debug)]
    pub(crate) struct FileSetData {
        pub(crate) monitor_data: HashMap<String, MonitorData>,
    }

    impl Default for FileSetData {
        fn default() -> Self {
            FileSetData {
                monitor_data: HashMap::new(),
            }
        }
    }

    #[derive(Serialize, Deserialize, Default, Debug)]
    pub(crate) struct MonitorData {
        pub(crate) counts: EventCounts,
        pub(crate) recent_events: Vec<MonitorEvent>,
    }

    impl MonitorData {
        fn receive_event(&mut self, ev: MonitorEvent) {
            self.recent_events.push(ev);
            self.counts.increment();
            self.trim(12);
        }

        fn trim(&mut self, keep_num_events: usize) {
            if self.recent_events.len() > keep_num_events {
                self.recent_events
                    .drain(0..=(self.recent_events.len() - keep_num_events));
            }
            self.counts.trim_all();
        }
    }

    #[derive(Serialize, Deserialize, Default, Debug)]
    pub(crate) struct EventCounts {
        pub(crate) seconds: HashMap<DateTime<Utc>, usize>,
        pub(crate) minutes: HashMap<DateTime<Utc>, usize>,
        pub(crate) hours: HashMap<DateTime<Utc>, usize>,
        pub(crate) days: HashMap<DateTime<Utc>, usize>,
        pub(crate) weeks: HashMap<DateTime<Utc>, usize>,
        pub(crate) months: HashMap<DateTime<Utc>, usize>,
        pub(crate) years: HashMap<DateTime<Utc>, usize>,
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
            if self.seconds.len() > EventCounts::KEEP_SECONDS {
                self.seconds =
                    EventCounts::trim_count(&mut self.seconds, EventCounts::KEEP_SECONDS);
            }
            if self.minutes.len() > EventCounts::KEEP_MINUTES {
                self.minutes =
                    EventCounts::trim_count(&mut self.minutes, EventCounts::KEEP_MINUTES);
            }
            if self.hours.len() > EventCounts::KEEP_HOURS {
                self.hours = EventCounts::trim_count(&mut self.hours, EventCounts::KEEP_HOURS);
            }
            if self.days.len() > EventCounts::KEEP_DAYS {
                self.days = EventCounts::trim_count(&mut self.days, EventCounts::KEEP_DAYS);
            }
            if self.weeks.len() > EventCounts::KEEP_WEEKS {
                self.weeks = EventCounts::trim_count(&mut self.weeks, EventCounts::KEEP_WEEKS);
            }
            if self.months.len() > EventCounts::KEEP_MONTHS {
                self.months = EventCounts::trim_count(&mut self.months, EventCounts::KEEP_MONTHS);
            }
            if self.years.len() > EventCounts::KEEP_YEARS {
                self.years = EventCounts::trim_count(&mut self.years, EventCounts::KEEP_YEARS);
            }
        }

        // FIXME: this is based on keeping a number of events. I think it should instead keep items younger than a specific cutoff time.
        // FIXME: there wont be an event every second or even minute in many cases so keeping x number of second counts makes no sense, we should
        // FIXME: instead keep seconds data going back X number of seconds.
        // FIXME: This should also simplofy the code as if should be possible to just filter out older events instead of all this nonsense
        // Trim old event counts of a particular type
        fn trim_count(
            items: &HashMap<DateTime<Utc>, usize>,
            keep: usize,
        ) -> HashMap<DateTime<Utc>, usize> {
            // Convert items into a vec of (k,v)
            let mut items_vec: Vec<(DateTime<Utc>, usize)> =
                items.iter().map(|(k, v)| (*k, *v)).collect();
            // Sort by k (the count event timestamp)
            items_vec.sort_unstable_by(|&(ka, _), &(kb, _)| {
                ka.timestamp().partial_cmp(&kb.timestamp()).unwrap()
            });
            // keep only the most recent how ever many we want
            items_vec.drain(0..=(items_vec.len() - keep));
            // Convert back to HashMap
            items_vec
                .into_iter()
                .collect::<HashMap<DateTime<Utc>, usize>>()
        }

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
    pub(crate) struct MonitorEvent {
        pub(crate) lines: Vec<LogLine>,
        pub(crate) awaiting_lines: usize,
        pub(crate) awaiting_lines_from: PathBuf,
    }

    #[derive(Serialize, Deserialize, Debug, Clone)]
    pub(crate) struct LogLine {
        pub(crate) date: DateTime<Utc>,
        pub(crate) line: String,
    }
}

#[tokio::main]
async fn main() -> std::io::Result<()> {
    let mut file_sets = HashMap::new();
    let mut monitors = HashMap::new();
    monitors.insert(
        "service_start".to_string(),
        Monitor {
            regex: Regex::new(r"Started").unwrap(),
            keep_lines_before: Some(3),
            keep_lines_after: Some(4),
            log_counts: true,
            log_recent_events: Some(5),
            notifiers: None,
        },
    );
    monitors.insert(
        "gnome_events".to_string(),
        Monitor {
            regex: Regex::new(r"gnome").unwrap(),
            keep_lines_before: None,
            keep_lines_after: None,
            log_counts: false,
            log_recent_events: Some(2),
            notifiers: None,
        },
    );
    file_sets.insert(
        "Syslog".to_string(),
        FileSet {
            file_globs: vec!["/var/log/sysl*".to_string()],
            monitors: monitors,
            line_buffers_before: HashMap::new(),
            max_lines_before: 0,
            max_lines_after: 0,
            data: FileSetData {
                monitor_data: HashMap::new(),
            },
        },
    );
    let mut monitors = HashMap::new();
    monitors.insert(
        "apache_404".to_string(),
        Monitor {
            regex: Regex::new(r"404").unwrap(),
            log_counts: true,
            keep_lines_after: None,
            keep_lines_before: None,
            log_recent_events: None,
            notifiers: Some(vec![
                Notifier::Mattermost {
                    url: Url::parse("https://chat.patternseek.net")
                        .expect("Couldn't parse Mattermost URL"),
                    api_key: "icb9pnrizpdf3ed4zibp9btaaw".to_string(),
                    template: "404 from Apache: \n{}".to_string(),
                },
                Notifier::Slack {
                    token: "T029WDLPM/BGX6EH8D7/7ZWjHVAOKjSXnsRqMLQMJOXO".to_string(),
                    template: "404 from Apache: ".to_string(),
                },
            ]),
        },
    );
    file_sets.insert(
        "Apache access".to_string(),
        FileSet {
            file_globs: vec!["/var/log/apache2/access.log".to_string()],
            monitors: monitors,
            line_buffers_before: Default::default(),
            max_lines_before: 0,
            max_lines_after: 0,
            data: FileSetData {
                monitor_data: HashMap::new(),
            },
        },
    );

    let mut conf = ConfigFile {
        file_sets: file_sets,
    };

    let mut file_handler_futures: Vec<BoxFuture<Result<(), Box<dyn Error>>>> = Vec::new();

    for (file_set_name, file_set) in &mut conf.file_sets {
        let fut = file_set.follow(file_set_name);
        file_handler_futures.push(Box::pin(fut.boxed()));
    }

    let _res = join_all(file_handler_futures).await;

    Ok(())
}
