mod config;
mod core;
mod data;
mod notifiers;

use crate::core::FileSet;
use futures::future::{join_all, BoxFuture};
use futures::prelude::*;
use std::collections::HashMap;
use std::error::Error;
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

    let mut filesets: HashMap<String, FileSet> = Default::default();
    for (fileset_name, fileset_conf) in config.file_sets {
        filesets.insert(fileset_name.clone(), FileSet::new_from_config(fileset_conf));
    }

    let mut file_handler_futures: Vec<BoxFuture<Result<(), Box<dyn Error>>>> = Vec::new();

    for (file_set_name, file_set) in &mut filesets {
        let fut = file_set.follow(file_set_name);
        file_handler_futures.push(Box::pin(fut.boxed()));
    }

    let res = join_all(file_handler_futures).await;

    res.iter().for_each(|res| match res {
        Ok(_res) => (),
        Err(err) => {
            eprintln!("Error: {}", err);
        }
    });

    Ok(())
}
