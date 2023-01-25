mod api;
mod config;
mod data;
mod fileset;
mod monitor;
mod notifier;

use crate::config::{ConfigFile, NotifierConfig};
use crate::data::FileSetData;
use crate::data::{DataStoreMessage, EventCounts, MonitorData};
use crate::fileset::{FileSet, FileSetId, LineHandlerMessage};
use crate::monitor::{Monitor, MonitorId};
use crate::notifier::{Notifier, NotifierId, NotifierMessage, WebhookBackEnd};
use chrono::{DateTime, Utc};
use futures::future::{join_all, BoxFuture};
use linemux::MuxedLines;
use std::collections::HashMap;
use std::process::exit;
use std::sync::Arc;
use structopt::*;
use tokio::signal::unix::{signal, SignalKind};
use tokio::sync::mpsc::{channel, Sender};
use tokio::sync::RwLock as RwLock_Tokio;
use tokio::task::JoinHandle;
use tokio::time::{sleep, Duration};
use actix_web::{web, App, HttpServer};


use std::io::Error;

/// CLI argument config
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
    #[structopt(help = "Data storage file path (JSON). Will be created if not present.")]
    data_file: String,
}

/// Main entry point
#[tokio::main]
async fn main() -> Result<(), Error> {
    // For tokio debugging. Enable this and run with: RUSTFLAGS="--cfg tokio_unstable" cargo run ./config.yaml ./data.json
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

    // Load event counts data from file, if present.
    let counts: HashMap<FileSetId, HashMap<MonitorId, EventCounts>> =
        match data::load_data_from_file(&args.data_file) {
            Ok(data) => {
                println!("Loaded data file from {}", &args.data_file);
                data
            }
            Err(e) => {
                eprintln!("Failed to load data from {}: {}", &args.data_file, e);
                Default::default()
            }
        };

    // Grab a couple of values before giving away the config object
    let notifiers_for_files_last_seen = config.global.notifiers_for_files_last_seen.clone();
    let period_for_files_last_seen = config.global.period_for_files_last_seen;

    // Prep structs and data
    let (mut filesets, filesets_data, _monitors, notifiers) =
        pop_structs_from_config(config, counts);
    let files_last_seen_data: HashMap<FileSetId, HashMap<String, DateTime<Utc>>> = HashMap::new();

    // Start long-running tasks
    let (notifiers_tx, notifier_join_handle) = notifier::start_task(notifiers).await;
    let (data_store_tx, data_store_join_handle) = data::start_task(
        filesets_data.clone(),
        files_last_seen_data,
        notifiers_tx.clone(),
        args.data_file.clone(),
    ).await;

    // Start web API
    let wrapped_filesets_data_rwlock = web::Data::new(filesets_data.clone());
    let actix_future = HttpServer::new(move || {
        App::new()
            .app_data(wrapped_filesets_data_rwlock.clone())
            .service(api::get_filesets)
            .service(api::get_monitors_for_fileset)
            .service(api::get_monitor)
            .service(api::dump)
    })
    .bind(("127.0.0.1", 8694)).expect("Failed to bind to API port: 8694" )
    .run();

    let api_join_handle = tokio::spawn(async move {
        println!("Webserver starting");
        actix_future.await.expect("API server failed");
    });

    // Timer task to send a summary of which files have been seen and when
    let file_summary_timer_task_join_handle = start_file_summary_timer_task(
        notifiers_for_files_last_seen,
        period_for_files_last_seen,
        &data_store_tx,
    );

    // Start a timer task to periodically persist the counts data
    let start_persist_data_timer_task_join_handle = start_persist_data_timer_task(&data_store_tx);

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

    // Handle signals
    let mut inter = signal(SignalKind::interrupt()).expect("couldn't listen for interrupt signal");
    let mut term = signal(SignalKind::terminate()).expect("couldn't listen for terminate signal");
    let mut file_handlers_join_future = join_all(file_handler_futures);
    tokio::select! {
        _ = inter.recv() => println!("SIGINT"),
        _ = term.recv() => println!("SIGTERM"),
        _ = &mut file_handlers_join_future => println!("JOINED ALL")
    };

    // Shut down
    println!("Shutting down");

    file_summary_timer_task_join_handle.abort();
    println!("Killed file summary timer task");

    data_store_tx
        .send(DataStoreMessage::Persist)
        .await
        .expect("Failed to send DataStoreMessage::Persist message during shutwodn");
    println!("Saving data");
    start_persist_data_timer_task_join_handle.abort();
    println!("Killed data persistance timer task");

    data_store_tx
        .send(DataStoreMessage::Shutdown)
        .await
        .expect("Unable to send datastore task shutdown message");
    data_store_join_handle.await.expect("Unable to join data store task");

    notifiers_tx
        .send(NotifierMessage::Shutdown)
        .await
        .expect("Unable to send notifier task shutdown message");
    notifier_join_handle
        .await
        .expect("Failed to join notifier task");

    api_join_handle.abort();
    println!("Killed API task");

    println!("Signalling shutdown to file handlers tasks");
    for tx in &mut file_handler_txs {
        tx.send(LineHandlerMessage::Shutdown)
            .await
            .expect("couldn't send file handler shutdown message");
    }
    file_handlers_join_future.await;
    println!("Shut down file handlers tasks");

    println!("Exiting");
    Ok(())
}

/// Starts a timer task which periodically sends notifications
/// indicating which files Centinela is monitoring.
fn start_file_summary_timer_task(
    notifiers_for_files_last_seen: Vec<NotifierId>,
    period_for_files_last_seen: usize,
    data_store_tx: &Sender<DataStoreMessage>,
) -> JoinHandle<()> {
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
                .expect("Datastore task seems to be dead when sending DataStoreMessage::NotifyFilesSeen");
            sleep(Duration::from_secs(period_for_files_last_seen as u64)).await;
        }
    })
}

/// Starts a timer which periodically persists counts data to disk
fn start_persist_data_timer_task(data_store_tx: &Sender<DataStoreMessage>) -> JoinHandle<()> {
    let data_store_tx_for_timer = data_store_tx.clone();
    tokio::spawn(async move {
        // Wait before first send
        sleep(Duration::from_secs(10)).await;
        loop {
            data_store_tx_for_timer
                .send(DataStoreMessage::Persist)
                .await
                .expect("Datastore task seems to be dead when sending DataStoreMessage::Persist");
            sleep(Duration::from_secs(30)).await;
        }
    })
}

/// Populates the main in memory data structures based on the config
/// file and any persisted data in the counts data file
fn pop_structs_from_config(
    config: ConfigFile,
    counts: HashMap<FileSetId, HashMap<MonitorId, EventCounts>>,
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
            let mut md = MonitorData::default();
            if let Some(fileset_counts) = counts.get(&fileset_id) {
                if let Some(monitor_counts) = fileset_counts.get(monitor_id) {
                    md.counts = monitor_counts.clone();
                }
            }
            fsd.monitor_data.insert(monitor_id.clone(), md);
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
