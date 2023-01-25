use crate::config::{NotifierConfig, WebhookNotifierConfig};
use crate::data::MonitorEvent;
use async_trait::async_trait;
use chrono::{DateTime, Duration, Utc};
use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::ops::Sub;
use tokio::sync::mpsc::{channel, Receiver, Sender};
use tokio::task::JoinHandle;

/// Newtype
pub(crate) type NotifierId = String;

/// Body type for sending webhook messages
#[derive(Serialize, Deserialize)]
struct WebhookBody {
    text: String,
}

/// Messages the notifier task listens for
#[derive(Debug)]
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
#[async_trait]
pub(crate) trait BackEnd {
    async fn notify_event(&self, ev: &MonitorEvent, skipped_notifications: usize);
    async fn notify_message(&self, message: &str);
}

/// Slack/Mattermost webhook
pub struct WebhookBackEnd {
    pub(crate) config: WebhookNotifierConfig,
}

#[async_trait]
impl BackEnd for WebhookBackEnd {
    async fn notify_event(&self, ev: &MonitorEvent, skipped_notifications: usize) {
        let client = reqwest::Client::new();
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
            .send()
            .await;
        match res {
            Ok(_res) => {
                println!("Sent event notification");
            }
            Err(e) => {
                println!("Failed to send event notification: {:?}", e);
            }
        };
    }

    async fn notify_message(&self, message: &str) {
        let client = reqwest::Client::new();
        let body = WebhookBody {
            text: message.to_owned(),
        };
        let res = client
            .post(self.config.url.as_str())
            .body(serde_json::to_string(&body).expect("Failed to build JSON"))
            .send()
            .await;
        match res {
            Ok(_res) => {
                println!("Sent message notification");
            }
            Err(e) => {
                println!("Failed to send message notification {:?}", e);
            }
        };
    }
}

/// Send an event notification if and when appropriate
pub(crate) async fn notify_event(mut notifier: &mut Notifier, ev_clone: &MonitorEvent) {
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
    notifier.last_notify = Utc::now();
    // Send notification
    notifier.back_end.notify_event(ev_clone, num_skipped).await;
}

/// Check whether the minimum interval between notifications has elapsed
fn skip_if_inside_minimum_interval(
    mut notifier: &mut Notifier,
    minimum_interval_option: Option<usize>,
) -> bool {
    if let Some(minimum_interval) = minimum_interval_option {
        let now = Utc::now();
        if now.sub(Duration::seconds(minimum_interval as i64)) <= notifier.last_notify {
            notifier.skipped_notifications += 1;
            return true;
        }
    };
    false
}

/// Start the notifier task. Listens for NotifierMessages
pub(crate) async fn start_task(
    mut notifiers: HashMap<NotifierId, Notifier>,
) -> (Sender<NotifierMessage>, JoinHandle<()>) {
    let (tx, mut rx): (Sender<NotifierMessage>, Receiver<NotifierMessage>) = channel(32);
    let join_handle = tokio::spawn(async move {
        println!("Started notifier task");
        while let Some(message) = rx.recv().await {
            match message {
                NotifierMessage::NotifyEvent(notifier_ids, ev_clone) => {
                    for notifier_id in &notifier_ids {
                        notify_event(
                            notifiers
                                .get_mut(notifier_id)
                                .unwrap_or_else(|| panic!("Invalid notifier ID {:?}", notifier_id)),
                            &ev_clone,
                        )
                        .await;
                    }
                }
                NotifierMessage::NotifyMessage(notifier_ids, message) => {
                    for notifier_id in &notifier_ids {
                        notifiers
                            .get_mut(notifier_id)
                            .unwrap_or_else(|| panic!("Invalid notifier ID {:?}", notifier_id))
                            .back_end
                            .notify_message(&message)
                            .await;
                    }
                }
                NotifierMessage::Shutdown => break,
            };
        }
        println!("Notifier task exiting...");
    });
    (tx, join_handle)
}
