use crate::config::{NotifierConfig, WebhookNotifierConfig};
use crate::data::MonitorEvent;
use chrono::{DateTime, Duration, Utc};
use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::ops::Sub;
use std::sync::mpsc::{sync_channel, Receiver, SyncSender};

/// Send a notification if and when appropriate
pub(crate) fn notify(mut notifier: &mut Notifier, ev_clone: &MonitorEvent) {
    // Limit how often notifications are sent
    let mininum_interval = match &notifier.config {
        NotifierConfig::Webhook(conf) => conf.minimum_interval,
    };
    if notifier.skip_if_inside_minimum_interval(mininum_interval) {
        println!("Skipping notify due to frequency");
        return;
    }
    let num_skipped = notifier.skipped_notifications;
    notifier.skipped_notifications = 0;
    notifier.last_notify = chrono::offset::Utc::now();
    println!("Sending!");
    // Send notification
    notifier.back_end.notify(ev_clone, num_skipped);
}

pub(crate) type NotifierId = String;

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
    fn notify(&self, ev: &MonitorEvent, skipped_notifications: usize);
}

pub struct WebhookBackEnd {
    pub(crate) config: WebhookNotifierConfig,
}

impl BackEnd for WebhookBackEnd {
    fn notify(&self, ev: &MonitorEvent, skipped_notifications: usize) {
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

pub(crate) enum NotifierMessage {
    Notify(Vec<NotifierId>, MonitorEvent),
    Exit,
}

pub(crate) fn start_thread(
    mut notifiers: HashMap<NotifierId, Notifier>,
) -> SyncSender<NotifierMessage> {
    let (tx, rx): (SyncSender<NotifierMessage>, Receiver<NotifierMessage>) = sync_channel(32);
    std::thread::spawn(move || {
        while let NotifierMessage::Notify(notifier_ids, ev_clone) =
            rx.recv().expect("channel not broken")
        {
            for notifier_id in &notifier_ids {
                notify(
                    notifiers
                        .get_mut(notifier_id)
                        .expect(format!("Invalid notifier ID {:?}", notifier_id).as_str()),
                    &ev_clone,
                );
            }
        }
    });
    tx
}
