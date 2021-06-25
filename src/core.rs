use crate::config::{FileSetConfig, MonitorConfig, NotifierConfig};
use crate::data::{FileSetData, LogLine, MonitorEvent};
use crate::notifiers::{notify, Notifier, WebhookBackEnd};
use glob::{glob as glob_parser, Paths};
use linemux::{Line, MuxedLines};
use std::collections::{HashMap, VecDeque};
use std::error::Error;
use std::path::PathBuf;
use std::process::exit;
use std::sync::{Arc, Mutex, RwLock};
use std::thread;

pub(crate) struct FileSet {
    pub(crate) config: FileSetConfig,
    pub(crate) monitors: HashMap<String, Monitor>,
    pub(crate) line_buffers_before: HashMap<PathBuf, VecDeque<LogLine>>,
    pub(crate) max_lines_before: usize,
    pub(crate) max_lines_after: usize,
    pub(crate) data: FileSetData,
    // This will potentially be for keeping track of changes to the set of files being monitored
    //    pub(crate) files_by_glob: HashMap<String, Vec<PathBuf>>,
}

impl FileSet {
    pub(crate) fn new_from_config(config: FileSetConfig) -> FileSet {
        let mut set = FileSet {
            config,
            monitors: Default::default(),
            line_buffers_before: Default::default(),
            max_lines_before: 0,
            max_lines_after: 0,
            data: Default::default(),
            //files_by_glob: Default::default(),
        };
        for (monitor_name, monitor_config) in set.config.monitors.clone() {
            set.monitors.insert(
                monitor_name.clone(),
                Monitor::new_from_config(monitor_config),
            );
        }
        set
    }

    pub async fn follow(&mut self, file_set_name: &str) -> Result<(), Box<dyn Error>> {
        let mut line_follower = match MuxedLines::new() {
            Ok(lf) => lf,
            Err(e) => return Err(Box::new(e)),
        };
        for glob in &self.config.file_globs {
            let mut glob_entries = FileSet::get_glob_entries(&glob);
            //let entries_list = self.files_by_glob.entry(glob.clone()).or_insert(Vec::new());
            let mut num_entries = 0;
            for entry in &mut glob_entries {
                match &entry {
                    Ok(path) => {
                        if let Err(e) = line_follower.add_file(path).await {
                            // Typically something like a file perm issue
                            eprintln!("File error for {:?} {}", &path, e);
                            exit(1);
                        }

                        println!("Monitoring file {:?}", path);
                        //entries_list.push(path.clone());
                    }
                    Err(e) => {
                        // Typically something like a directory perm issue
                        eprintln!("File error for {} {}", glob, e);
                        exit(1);
                    }
                };
                num_entries += 1;
            }
            if num_entries < 1 {
                eprintln!("No files found matching glob {}", glob);
                exit(1);
            }
        }

        for monitor in self.monitors.values() {
            // Calculate the number of previous lines to keep per file
            if let Some(keep) = monitor.config.keep_lines_before {
                if keep > self.max_lines_before {
                    self.max_lines_before = keep;
                }
            }
            // Calculate the number of subsequent lines to keep per file
            if let Some(keep) = monitor.config.keep_lines_after {
                if keep > self.max_lines_after {
                    self.max_lines_after = keep;
                }
            }
        }
        self.line_handler(file_set_name, line_follower).await;

        Ok(())
    }

    fn get_glob_entries(glob: &&String) -> Paths {
        let mut glob_entries = match glob_parser(&glob) {
            Ok(entries) => entries,
            Err(err) => {
                eprintln!("Couldn't parse glob {}. Error: {}", glob, err);
                exit(1);
            }
        };
        glob_entries
    }

    /// Watch the lines generated for a set of files
    async fn line_handler(&mut self, file_set_name: &str, mut line_follower: MuxedLines) {
        // For each line received from a set of files
        loop {
            let line = match line_follower.next_line().await {
                Ok(Some(line)) => line,
                Ok(None) => {
                    eprintln!("No files added to file set follower: {}", file_set_name);
                    exit(1);
                }
                Err(err) => {
                    eprintln!("Error: {}", err);
                    exit(1);
                }
            };
            // Check for monitor matches
            for (monitor_name, monitor) in &mut self.monitors {
                // Initialise the data entry for this monitor if necessary
                let monitor_data = self
                    .data
                    .monitor_data
                    .entry(monitor_name.into())
                    .or_insert_with(Default::default);
                // Pass the line to the MonitorData in case there are previous events awaiting subsequent lines
                monitor_data.receive_line(&line, line.source());
                // Pass the line to the monitor for testing and possibly processing
                if let Some(evlock) = monitor
                    .handle_line(&line, self.line_buffers_before.get(line.source()))
                    .await
                {
                    monitor_data.receive_event(evlock, monitor.config.log_recent_events);
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
                is_event_line: false,
            });
            while buf.len() > self.max_lines_before {
                buf.pop_front();
            }
        }
    }
}

pub(crate) struct Monitor {
    pub(crate) config: MonitorConfig,
    pub(crate) notifiers: Vec<Arc<RwLock<Notifier>>>,
}

impl Monitor {
    fn new_from_config(config: MonitorConfig) -> Monitor {
        let mut mon = Monitor {
            config,
            notifiers: Default::default(),
        };
        if let Some(notifiers_config) = mon.config.notifiers.clone() {
            for notifier_config in notifiers_config {
                mon.notifiers.push(match &notifier_config {
                    NotifierConfig::Webhook(wh_config) => Arc::new(RwLock::new(Notifier {
                        config: notifier_config.clone(),
                        back_end: Box::new(WebhookBackEnd {
                            config: wh_config.clone(),
                        }),
                        last_notify: chrono::offset::Utc::now() - chrono::Duration::weeks(52),
                        skipped_notifications: 0,
                    })),
                })
            }
        };
        mon
    }

    async fn handle_line(
        &mut self,
        line: &Line,
        previous_lines: Option<&VecDeque<LogLine>>,
    ) -> Option<Arc<Mutex<MonitorEvent>>> {
        if self.config.regex.is_match(line.line()) {
            // Log line in question
            let log_line = LogLine {
                date: chrono::offset::Utc::now(),
                line: line.line().to_string(),
                is_event_line: true,
            };

            // Get previous lines if appropriate
            let lines = match self.config.keep_lines_before {
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
            let ev = Arc::new(Mutex::new(MonitorEvent {
                lines,
                awaiting_lines: self.config.keep_lines_after.unwrap_or(0),
                awaiting_lines_from: line.source().to_owned(),
            }));
            println!("Generated event for line {:#?}", &line);
            // Notify
            for notifier in &mut self.notifiers {
                let ev_clone = ev.clone();
                let notifier_clone = notifier.clone();
                thread::spawn(|| {
                    let _ = notify(notifier_clone, ev_clone);
                });
            }
            // Return
            Some(ev)
        } else {
            None
        }
    }
}
