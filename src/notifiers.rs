use crate::config::{NotifierConfig, WebhookNotifierConfig};
use crate::data::MonitorEvent;
use chrono::{DateTime, Duration, Utc};
use serde::{Deserialize, Serialize};
use std::ops::Sub;
use std::sync::{Arc, Mutex, RwLock};
use std::thread::sleep;
use std::time::Duration as StdDuration;

/// Send a notification if and when appropriate
pub(crate) fn notify(notifier_lock: Arc<RwLock<Notifier>>, evlock: Arc<Mutex<MonitorEvent>>) {
    // Limit how often notifications are sent
    let mininum_interval = match &notifier_lock.read().expect("not poisoned").config {
        NotifierConfig::Webhook(conf) => conf.minimum_interval,
    };
    if notifier_lock
        .write()
        .expect("not poisoned")
        .skip_if_inside_minimum_interval(mininum_interval)
    {
        return;
    }

    // If the event is awaiting subsequent log lines then we may want to wait for them to appear
    // Should drop the lock instantly
    if evlock.lock().expect("Non poisoned lock").awaiting_lines > 0 {
        println!("Need lines");
        // If the monitor is configured to wait for additional lines then do so
        if let Some(wait_for_lines) = match &notifier_lock.read().expect("not poisoned").config {
            NotifierConfig::Webhook(conf) => conf.wait_seconds_for_additional_lines,
        } {
            println!("Found requirement to wait for lines");
            let waiting_since = chrono::offset::Utc::now();
            while waiting_since + Duration::seconds(wait_for_lines as i64)
                > chrono::offset::Utc::now()
            {
                println!("Waiting for lines...");
                sleep(StdDuration::from_secs(1));
            }
        }
    }

    let mut notifier = notifier_lock.write().expect("not poisoned");
    let num_skipped = notifier.skipped_notifications;
    notifier.skipped_notifications = 0;
    notifier.last_notify = chrono::offset::Utc::now();

    // Send notification
    notifier.back_end.notify(evlock, num_skipped);
}

pub(crate) struct Notifier {
    pub(crate) config: NotifierConfig,
    pub(crate) back_end: Box<dyn BackEnd + Sync + Send>,
    pub(crate) last_notify: DateTime<Utc>,
    pub(crate) skipped_notifications: usize,
}

impl Notifier {
    /// Check whether the minimum interval between notifications has elapsed
    fn skip_if_inside_minimum_interval(&mut self, minimum_interval_option: Option<usize>) -> bool {
        if let Some(minimum_interval) = minimum_interval_option {
            let now = chrono::offset::Utc::now();
            if now.sub(Duration::seconds(minimum_interval as i64)) <= self.last_notify {
                self.skipped_notifications += 1;
                return true;
            }
        };
        false
    }
}

pub(crate) trait BackEnd {
    fn notify(&self, ev: Arc<Mutex<MonitorEvent>>, skipped_notifications: usize);
}

pub struct WebhookBackEnd {
    pub(crate) config: WebhookNotifierConfig,
}

impl BackEnd for WebhookBackEnd {
    fn notify(&self, ev: Arc<Mutex<MonitorEvent>>, skipped_notifications: usize) {
        let client = reqwest::blocking::Client::new();
        let skipped_str = match skipped_notifications {
            0 => "".to_string(),
            _ => format!(
                "\n\n({} notifications skipped due to high frequency)",
                skipped_notifications
            ),
        };
        let body = WebhookBody {
            text: self.config.template.to_owned()
                + ev.lock()
                    .expect("not poisoned")
                    .get_lines_as_markdown()
                    .as_str()
                + &skipped_str,
        };
        let res = client
            .post(self.config.url.as_str())
            .body(serde_json::to_string(&body).expect("Failed to build JSON"))
            .send();
        match res {
            Ok(res) => {
                // FIXME
                println!("Sent! {:?}", res);
            }
            Err(e) => {
                // FIXME
                println!("Failed to send {:?}", e);
            }
        };
    }
}

#[derive(Serialize, Deserialize)]
struct WebhookBody {
    text: String,
}
