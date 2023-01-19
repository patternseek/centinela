use crate::config::FileSetConfig;
use crate::data::{DataStoreMessage, LogLine};
use crate::monitor::{Monitor, MonitorId};
use crate::notifier::NotifierId;
use core::default::Default;
use core::option::Option;
use core::option::Option::{None, Some};
use core::result::Result;
use core::result::Result::{Err, Ok};
use glob::{glob as glob_parser, Paths};
use linemux::{Line, MuxedLines};
use std::collections::{HashMap, VecDeque};
use std::error::Error;
use std::path::PathBuf;
use std::process::exit;
use tokio::sync::mpsc::Sender;

/// Newtype to create an ID for FileSets
pub(crate) type FileSetId = String;

/// Newtype to simplify this type
pub(crate) type MonitorToNotifiersRelation = HashMap<MonitorId, (Monitor, Option<Vec<NotifierId>>)>;

/// Messages the LineHandler loop listens for. Only shutdown currently.
#[derive(Debug, PartialEq)]
pub(crate) enum LineHandlerMessage {
    Shutdown,
}

/// Struct containing in-memory data about a particular set of monitored files
pub(crate) struct FileSet {
    pub(crate) config: FileSetConfig,
    /// Notifiers used for monitors for this FileSet
    pub(crate) monitor_notifier_sets: MonitorToNotifiersRelation,
    /// Buffers of recent lines from monitored files
    pub(crate) line_buffers_before: HashMap<PathBuf, VecDeque<LogLine>>,
    /// Calculated value for the maximum number of previous lines needed by any of
    /// the active monitors for this FileSet
    pub(crate) max_lines_before: usize,
    /// Calculated value for the maximum number of subsequent lines needed by any of
    /// the active monitors for this FileSet
    pub(crate) max_lines_after: usize,
}

impl FileSet {
    /// Create a new FileSet from a FileSetConfig
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

    /// Create a MuxedLines line follower for this FileSet.
    /// Update self.max_lines_before and self.max_lines_after if necessary.
    pub(crate) async fn get_follower(&mut self) -> Result<MuxedLines, Box<dyn Error>> {
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

        // Some runtime configuration based on the monitors' settings
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
        let glob_entries = match glob_parser(glob) {
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
        mut line_handler_rx: tokio::sync::mpsc::Receiver<LineHandlerMessage>,
    ) {
        // For each line received from a set of files
        loop {
            tokio::select! {
                msg_opt = line_handler_rx.recv() => {
                    if  Some(LineHandlerMessage::Shutdown) == msg_opt {
                        break;
                    }
                }
                line_res = line_follower.next_line() => {
                    let line = match line_res {
                        Ok(Some(line)) => line,
                        Ok(None) => {
                            eprintln!("No files added to file set follower: {}", fileset_id);
                            exit(1);
                        }
                        Err(err) => {
                            eprintln!("Error: {}", err);
                            continue;
                        }
                    };
                    // Keep track of when we last received a line from each file
                    let _ = data_store_tx
                        .send(DataStoreMessage::FileSeen(
                            fileset_id.to_string(),
                            line.source()
                                .to_str()
                                .expect("Valid string as filename")
                                .to_string(),
                        ))
                        .await;
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
