mod api;
mod config;
mod data;
mod fileset;
mod monitor;
mod notifier;

use crate::config::{ConfigFile, NotifierConfig};
use crate::data::DataStoreMessage;
use crate::data::FileSetData;
use crate::fileset::{FileSet, FileSetId, LineHandlerMessage};
use crate::monitor::{Monitor, MonitorId};
use crate::notifier::{Notifier, NotifierId, NotifierMessage, WebhookBackEnd};
use chrono::{DateTime, Utc};
use futures::future::{join_all, BoxFuture};
use linemux::{Line, MuxedLines};
use log::info;
use std::collections::HashMap;
use std::process::exit;
use std::sync::Arc;
use structopt::*;
use tokio::signal::unix::{signal, SignalKind};
use tokio::sync::mpsc::{channel, Sender};
use tokio::sync::RwLock as RwLock_Tokio;
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
    console_subscriber::init();

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
    let period_for_files_last_seen = config.global.period_for_files_last_seen;

    // Prep structs and data
    let (mut filesets, filesets_data, _monitors, notifiers) = pop_structs_from_config(config);
    let files_last_seen_data: HashMap<FileSetId, HashMap<String, DateTime<Utc>>> = HashMap::new();

    // Start long-running threads and tasks
    let (notifiers_tx, notifier_join_handle) = notifier::start_thread(notifiers);
    let data_store_tx = data::start_task(
        filesets_data.clone(),
        files_last_seen_data,
        notifiers_tx.clone(),
    );

    // Start web API
    let api_join_handle = api::start_thread(filesets_data.clone());

    // Timer task to send a summary of which files have been seen and when
    start_file_summary_timer_task(
        notifiers_for_files_last_seen,
        period_for_files_last_seen,
        &data_store_tx,
    );

    // Follow the files matched by each FileSet
    let mut file_handler_futures: Vec<BoxFuture<()>> = Vec::new();
    let mut file_handler_txs: Vec<Sender<LineHandlerMessage>> = Vec::new();
    for (fileset_id, file_set) in &mut filesets {
        let line_follower: MuxedLines = match file_set.get_follower().await {
            Ok(lf) => lf,
            Err(e) => {
                eprintln!("Error: {}", e);
                exit(1);
            }
        };
        let (tx, rx) = channel(32);
        let fut = file_set.line_handler(fileset_id, line_follower, data_store_tx.clone(), rx);
        file_handler_futures.push(Box::pin(fut));
        file_handler_txs.push(tx);
    }

    tokio::select! {
        _ = signal(SignalKind::interrupt())?.recv() => println!("SIGINT"),
        _ = signal(SignalKind::terminate())?.recv() => println!("SIGTERM"),
    }

    let _res = join_all(file_handler_futures).await;

    // Shut down
    info!("Shutting down");

    data_store_tx
        .send(DataStoreMessage::Shutdown)
        .await
        .expect("Unable to send datastore task shutdown message");
    info!("Shut down datastore task");

    notifiers_tx
        .send(NotifierMessage::Shutdown)
        .expect("Unable to send notifier thread shutdown message");
    notifier_join_handle
        .join()
        .expect("Failed to join notifier thread");
    info!("Shut down notifier thread");

    api_join_handle.join().expect("Failed to join API thread");
    info!("Shut down API thread");

    Ok(())
}

fn start_file_summary_timer_task(
    notifiers_for_files_last_seen: Vec<NotifierId>,
    period_for_files_last_seen: usize,
    data_store_tx: &Sender<DataStoreMessage>,
) {
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
}

fn pop_structs_from_config(
    config: ConfigFile,
) -> (
    HashMap<FileSetId, FileSet>,
    Arc<RwLock_Tokio<HashMap<FileSetId, FileSetData>>>,
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
    let filesets_data_rwlock = Arc::new(RwLock_Tokio::new(filesets_data));

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
    (filesets, filesets_data_rwlock, monitors, notifiers)
}
