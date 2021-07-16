mod config;
mod core;
mod data;
mod notifiers;

use crate::config::{ConfigFile, NotifierConfig};
use crate::core::{FileSet, FileSetId, Monitor, MonitorId};
use crate::data::FileSetData;
use crate::notifiers::{Notifier, NotifierId, WebhookBackEnd};
use futures::future::{join_all, BoxFuture};
use linemux::MuxedLines;
use std::collections::HashMap;
use std::process::exit;
use structopt::*;

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
    let args = Args::from_args();
    let config = match config::load(args.config_file) {
        Ok(config) => config,
        Err(err) => {
            eprintln!("Error: {}", err);
            exit(1);
        }
    };

    let (mut filesets, filesets_data, _monitors, notifiers) = pop_structs_from_config(config);

    let notifiers_tx = notifiers::start_thread(notifiers);

    let data_store_tx = data::start_task(filesets_data, notifiers_tx.clone());

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
