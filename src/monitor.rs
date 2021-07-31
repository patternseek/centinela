use crate::config::MonitorConfig;
use crate::data::{LogLine, MonitorEvent};
use crate::notifier::NotifierId;
use linemux::Line;
use std::collections::VecDeque;

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

    pub(crate) async fn handle_line(
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
            println!("Generated event for {:#?}", &line);
            // Return
            Some(ev)
        } else {
            None
        }
    }
}