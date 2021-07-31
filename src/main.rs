mod config;
mod data;
mod fileset;
mod monitor;
mod notifier;

use crate::config::{ConfigFile, NotifierConfig};
use crate::data::DataStoreMessage;
use crate::data::FileSetData;
use crate::fileset::{FileSet, FileSetId};
use crate::monitor::{Monitor, MonitorId};
use crate::notifier::{Notifier, NotifierId, WebhookBackEnd};
use chrono::{DateTime, Utc};
use futures::future::{join_all, BoxFuture};
use linemux::MuxedLines;
use std::collections::HashMap;
use std::process::exit;
use structopt::*;
use tokio::time::{sleep, Duration};

// CLI argument config
#[derive(StructOpt, Debug)]
#[structopt(
    name = "Centinela",
    about = r#"
Log statistics and alerts.

"#
)]
struct Args {
    #[structopt(help = "Config file path (YAML)")]
    config_file: String,
}

#[tokio::main]
async fn main() -> std::io::Result<()> {
    // Parse CLI args
    let args = Args::from_args();

    // Load conf
    let config = match config::load(args.config_file) {
        Ok(config) => config,
        Err(err) => {
            eprintln!("Error loading config: {}", err);
            exit(1);
        }
    };

    let notifiers_for_files_last_seen = config.global.notifiers_for_files_last_seen.clone();
    let period_for_files_last_seen = config.global.period_for_files_last_seen.clone();

    // Prep structs and data
    let (mut filesets, filesets_data, _monitors, notifiers) = pop_structs_from_config(config);
    let files_last_seen_data: HashMap<String, DateTime<Utc>> = HashMap::new();

    // Start long-running threads and tasks
    let notifiers_tx = notifier::start_thread(notifiers);
    let data_store_tx = data::start_task(filesets_data, files_last_seen_data, notifiers_tx.clone());

    // Timer task to send a summary of which files have been seen and when
    let data_store_tx_for_timer = data_store_tx.clone();
    tokio::spawn(async move {
        // Wait before first send
        sleep(Duration::from_secs(60)).await;
        loop {
            data_store_tx_for_timer
                .send(DataStoreMessage::NotifyFilesSeen(
                    notifiers_for_files_last_seen.clone(),
                ))
                .await
                .expect("Datastore task not dead");
            sleep(Duration::from_secs(period_for_files_last_seen as u64)).await;
        }
    });

    // Follow the files matched by each FileSet
    let mut file_handler_futures: Vec<BoxFuture<()>> = Vec::new();
    for (fileset_id, file_set) in &mut filesets {
        let line_follower: MuxedLines = match file_set.get_follower().await {
            Ok(lf) => lf,
            Err(e) => {
                eprintln!("Error: {}", e);
                exit(1);
            }
        };
        let fut = file_set.line_handler(fileset_id, line_follower, data_store_tx.clone());
        file_handler_futures.push(Box::pin(fut));
    }

    let _res = join_all(file_handler_futures).await;

    Ok(())
}

fn pop_structs_from_config(
    config: ConfigFile,
) -> (
    HashMap<FileSetId, FileSet>,
    HashMap<FileSetId, FileSetData>,
    HashMap<MonitorId, Monitor>,
    HashMap<NotifierId, Notifier>,
) {
    let mut monitors: HashMap<MonitorId, Monitor> = Default::default();
    for (monitor_id, monitor_config) in config.monitors {
        monitors.insert(monitor_id.clone(), Monitor::new_from_config(monitor_config));
    }

    let mut filesets: HashMap<FileSetId, FileSet> = Default::default();
    let mut filesets_data: HashMap<FileSetId, FileSetData> = Default::default();
    for (fileset_id, fileset_conf) in config.file_sets {
        // Create the FileSet
        let fs = FileSet::new_from_config(fileset_conf, &monitors);
        // Create a FileSetData for the FileSet
        let mut fsd = FileSetData {
            monitor_data: Default::default(),
        };
        // Create a MonitorData for each Monitor that's used by the FileSet
        for (monitor_id, (_, _)) in &fs.monitor_notifier_sets {
            fsd.monitor_data
                .insert(monitor_id.clone(), Default::default());
        }
        filesets.insert(fileset_id.clone(), fs);
        filesets_data.insert(fileset_id.clone(), fsd);
    }

    let mut notifiers: HashMap<NotifierId, Notifier> = Default::default();
    for (notifier_id, notifier_config) in config.notifiers {
        notifiers.insert(
            notifier_id.clone(),
            match &notifier_config {
                NotifierConfig::Webhook(wh_config) => Notifier {
                    config: notifier_config.clone(),
                    back_end: Box::new(WebhookBackEnd {
                        config: wh_config.clone(),
                    }),
                    last_notify: chrono::offset::Utc::now() - chrono::Duration::weeks(52),
                    skipped_notifications: 0,
                },
            },
        );
    }
    (filesets, filesets_data, monitors, notifiers)
}
