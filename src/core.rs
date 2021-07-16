use glob::{glob as glob_parser, Paths};
use linemux::{Line, MuxedLines};

use crate::config::{FileSetConfig, MonitorConfig};
use crate::data::{DataStoreMessage, LogLine, MonitorEvent};
use crate::notifiers::NotifierId;
use std::collections::{HashMap, VecDeque};
use std::error::Error;
use std::path::PathBuf;
use std::process::exit;
use tokio::sync::mpsc::Sender;

pub(crate) type FileSetId = String;

pub(crate) type MonitorNotifierSet = HashMap<MonitorId, (Monitor, Option<Vec<NotifierId>>)>;

pub(crate) struct FileSet {
    pub(crate) config: FileSetConfig,
    pub(crate) monitor_notifier_sets: MonitorNotifierSet,
    pub(crate) line_buffers_before: HashMap<PathBuf, VecDeque<LogLine>>,
    pub(crate) max_lines_before: usize,
    pub(crate) max_lines_after: usize,
    // This will potentially be for keeping track of changes to the set of files being monitored
    //    pub(crate) files_by_glob: HashMap<String, Vec<PathBuf>>,
}

impl FileSet {
    pub(crate) fn new_from_config(
        config: FileSetConfig,
        monitors: &HashMap<MonitorId, Monitor>,
    ) -> FileSet {
        let mut set = FileSet {
            config,
            monitor_notifier_sets: Default::default(),
            line_buffers_before: Default::default(),
            max_lines_before: 0,
            max_lines_after: 0,
            //files_by_glob: Default::default(),
        };
        for (monitor_id, notifier_ids) in &set.config.monitor_notifier_sets {
            let monitor = monitors
                .get(monitor_id)
                .expect("Invalid monitor ID in file_set config.")
                .clone();
            set.monitor_notifier_sets
                .insert(monitor_id.clone(), (monitor, notifier_ids.clone()));
        }
        set
    }

    pub async fn get_follower(&mut self) -> Result<MuxedLines, Box<dyn Error>> {
        let mut line_follower = match MuxedLines::new() {
            Ok(lf) => lf,
            Err(e) => return Err(Box::new(e)),
        };
        for glob in &self.config.file_globs {
            let mut glob_entries = FileSet::get_glob_entries(&glob);
            //let entries_list = self.files_by_glob.entry(glob.clone()).or_insert(Vec::new());
            let mut num_entries: i32 = 0;
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

        for (monitor, _notifiers) in self.monitor_notifier_sets.values() {
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
        Ok(line_follower)
    }

    fn get_glob_entries(glob: &&String) -> Paths {
        let glob_entries = match glob_parser(&glob) {
            Ok(entries) => entries,
            Err(err) => {
                eprintln!("Couldn't parse glob {}. Error: {}", glob, err);
                exit(1);
            }
        };
        glob_entries
    }

    /// Watch the lines generated for a set of files
    pub(crate) async fn line_handler(
        &mut self,
        fileset_id: &FileSetId,
        mut line_follower: MuxedLines,
        data_store_tx: Sender<DataStoreMessage>,
    ) {
        // For each line received from a set of files
        loop {
            let line = match line_follower.next_line().await {
                Ok(Some(line)) => line,
                Ok(None) => {
                    eprintln!("No files added to file set follower: {}", fileset_id);
                    exit(1);
                }
                Err(err) => {
                    eprintln!("Error: {}", err);
                    exit(1);
                }
            };
            // Check for monitor matches
            for (monitor_id, (monitor, notifier_ids)) in &mut self.monitor_notifier_sets {
                // Pass the line to the MonitorData in case there are previous events awaiting subsequent lines
                let _ = data_store_tx
                    .send(DataStoreMessage::ReceiveLine(
                        fileset_id.clone(),
                        monitor_id.clone(),
                        line.clone(),
                    ))
                    .await;
                // Pass the line to the monitor for testing and possibly processing
                if let Some(ev) = monitor
                    .handle_line(&line, self.line_buffers_before.get(line.source()))
                    .await
                {
                    let _ = data_store_tx
                        .send(DataStoreMessage::ReceiveEvent(
                            fileset_id.clone(),
                            monitor_id.clone(),
                            ev,
                            monitor.config.log_recent_events,
                            notifier_ids.clone(),
                        ))
                        .await;
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

pub(crate) type MonitorId = String;

#[derive(Clone)]
pub(crate) struct Monitor {
    pub(crate) config: MonitorConfig,
    pub(crate) notifiers: Vec<NotifierId>,
}

impl Monitor {
    pub(crate) fn new_from_config(config: MonitorConfig) -> Monitor {
        Monitor {
            config,
            notifiers: Default::default(),
        }
    }

    async fn handle_line(
        &mut self,
        line: &Line,
        previous_lines: Option<&VecDeque<LogLine>>,
    ) -> Option<MonitorEvent> {
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
            let ev = MonitorEvent {
                lines,
                awaiting_lines: self.config.keep_lines_after.unwrap_or(0),
                awaiting_lines_from: line.source().to_owned(),
                notify_by: chrono::offset::Utc::now()
                    + chrono::Duration::seconds(self.config.max_wait_before_notify as i64),
            };
            println!("Generated event for line {:#?}", &line);
            // Return
            Some(ev)
        } else {
            None
        }
    }
}
