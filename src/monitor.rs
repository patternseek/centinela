use crate::config::MonitorConfig;
use crate::data::{LogLine, MonitorEvent};
// use crate::notifier::NotifierId;
use linemux::Line;
use std::collections::VecDeque;

pub(crate) type MonitorId = String;

#[derive(Clone)]
pub(crate) struct Monitor {
    pub(crate) config: MonitorConfig,
}

impl Monitor {
    pub(crate) fn new_from_config(config: MonitorConfig) -> Monitor {
        Monitor { config }
    }

    /// Process a single logfile line
    pub(crate) async fn handle_line(
        &mut self,
        line: &Line,
        previous_lines: Option<&VecDeque<LogLine>>,
    ) -> Option<MonitorEvent> {
        if self.config.regex.is_match(line.line()) {

            let mut variant = String::new();
            for (_, [variant_tmp]) in self.config.regex.captures_iter(line.line()).map(|c| c.extract()) {
                // This will only run if a capture group matches.
                // Hoewever all regexes must have one and only one capture
                // group, so in order to reach this code it will theoretically
                // always have a matching capture group.
                variant = variant_tmp.to_string();
            }
            
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
                        let range_start = if previous_lines.len() >= keep_lines {
                            previous_lines.len() - keep_lines
                        } else {
                            previous_lines.len()
                        };
                        let mut subset = previous_lines
                            .range(range_start..)
                            .cloned()
                            .collect::<Vec<LogLine>>();
                        subset.push(log_line);
                        subset
                    }
                    _ => vec![log_line],
                },
                None => vec![log_line],
            };

            // Create a new match event
            let ev = MonitorEvent {
                lines,
                variant: variant,
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
