use crate::config::{NotifierConfig, WebhookNotifierConfig};
use crate::data::MonitorEvent;
use chrono::{DateTime, Duration, Utc};
use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::ops::Sub;
use std::sync::mpsc::{sync_channel, Receiver, SyncSender};
use log::{error, log, info};

/// Newtype
pub(crate) type NotifierId = String;

/// Body type for sending webhook messages
#[derive(Serialize, Deserialize)]
struct WebhookBody {
    text: String,
}

/// Messages the notifier thread listens for
pub(crate) enum NotifierMessage {
    NotifyEvent(Vec<NotifierId>, MonitorEvent),
    NotifyMessage(Vec<NotifierId>, String),
    Shutdown,
}

/// In-memory representation of a Notifier
pub(crate) struct Notifier {
    pub(crate) config: NotifierConfig,
    pub(crate) back_end: Box<dyn BackEnd + Sync + Send>,
    pub(crate) last_notify: DateTime<Utc>,
    pub(crate) skipped_notifications: usize,
}

/// Trait to be implemented by Notifier back-ends.
pub(crate) trait BackEnd {
    fn notify_event(&self, ev: &MonitorEvent, skipped_notifications: usize);
    fn notify_message(&self, message: &str);
}

/// Slack/Mattermost webhook
pub struct WebhookBackEnd {
    pub(crate) config: WebhookNotifierConfig,
}

impl BackEnd for WebhookBackEnd {
    fn notify_event(&self, ev: &MonitorEvent, skipped_notifications: usize) {
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
                + ev.get_lines_as_markdown().as_str()
                + &skipped_str,
        };
        let res = client
            .post(self.config.url.as_str())
            .body(serde_json::to_string(&body).expect("Failed to build JSON"))
            .send();
        match res {
            Ok(_res) => (),
            Err(e) => {
                println!("Failed to send event notification: {:?}", e);
            }
        };
    }

    fn notify_message(&self, message: &str) {
        let client = reqwest::blocking::Client::new();
        let body = WebhookBody {
            text: message.to_owned(),
        };
        let res = client
            .post(self.config.url.as_str())
            .body(serde_json::to_string(&body).expect("Failed to build JSON"))
            .send();
        match res {
            Ok(_res) => (),
            Err(e) => {
                println!("Failed to send message notification {:?}", e);
            }
        };
    }
}

/// Send an event notification if and when appropriate
pub(crate) fn notify_event(mut notifier: &mut Notifier, ev_clone: &MonitorEvent) {
    // Limit how often notifications are sent
    let mininum_interval = match &notifier.config {
        NotifierConfig::Webhook(conf) => conf.minimum_interval,
    };
    if skip_if_inside_minimum_interval(notifier, mininum_interval) {
        //println!("Skipping notify due to frequency");
        return;
    }
    let num_skipped = notifier.skipped_notifications;
    notifier.skipped_notifications = 0;
    notifier.last_notify = chrono::offset::Utc::now();
    // Send notification
    notifier.back_end.notify_event(ev_clone, num_skipped);
}

/// Check whether the minimum interval between notifications has elapsed
fn skip_if_inside_minimum_interval(
    mut notifier: &mut Notifier,
    minimum_interval_option: Option<usize>,
) -> bool {
    if let Some(minimum_interval) = minimum_interval_option {
        let now = chrono::offset::Utc::now();
        if now.sub(Duration::seconds(minimum_interval as i64)) <= notifier.last_notify {
            notifier.skipped_notifications += 1;
            return true;
        }
    };
    false
}

/// Start the notifier thread. Listens for NotifierMessages
pub(crate) fn start_thread(
    mut notifiers: HashMap<NotifierId, Notifier>,
) -> (SyncSender<NotifierMessage>, std::thread::JoinHandle<()>) {
    let (tx, rx): (SyncSender<NotifierMessage>, Receiver<NotifierMessage>) = sync_channel(32);
    let join_handle = std::thread::spawn(move || {
        println!("Started notifier thread");
        loop {
        match rx.recv().expect("channel not broken") {
            NotifierMessage::NotifyEvent(notifier_ids, ev_clone) => {
                for notifier_id in &notifier_ids {
                    notify_event(
                        notifiers
                            .get_mut(notifier_id)
                            .unwrap_or_else(|| panic!("Invalid notifier ID {:?}", notifier_id)),
                        &ev_clone,
                    );
                }
            }
            NotifierMessage::NotifyMessage(notifier_ids, message) => {
                for notifier_id in &notifier_ids {
                    notifiers
                        .get_mut(notifier_id)
                        .unwrap_or_else(|| panic!("Invalid notifier ID {:?}", notifier_id))
                        .back_end
                        .notify_message(&message);
                }
            }
            NotifierMessage::Shutdown => break,
        }
    }});
    (tx, join_handle)
}
