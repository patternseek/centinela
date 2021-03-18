use regex::Regex;
use futures::prelude::*;
use futures::future::{join_all, BoxFuture};
use std::collections::HashMap;

use lib::*;
use std::error::Error;

mod lib{
    use std::collections::HashMap;
    use chrono::{DateTime, Utc};
    use regex::Regex;
    use serde;
    use serde::{Deserialize, Serialize};
    use linemux::MuxedLines;
    use std::process::exit;
    use glob::glob as glob_parser;
    use std::error::Error;


    #[derive(Serialize, Deserialize)]
    pub(crate) struct ConfigFile {
        pub(crate) file_sets: HashMap<String,FileSet>,
    }

    #[derive(Serialize, Deserialize)]
    pub(crate) struct FileSet{
        pub(crate) file_globs: Vec<String>,
        pub(crate) monitors: Vec<Monitor>
    }

    impl FileSet{

        pub(crate) async fn follow( &self, file_set_name: &String, file_set: &FileSet) -> Result<(),Box<dyn Error>> {
            let mut line_follower = match MuxedLines::new() {
                Ok(lf) => lf,
                Err(e) => return Err(Box::new(e))
            };
            for glob in &file_set.file_globs {
                for entry in glob_parser(&glob).expect(format!("Failed to read glob pattern: {}", glob).as_ref()) {
                    match &entry {
                        Ok(path) => {
                            match line_follower.add_file(path).await {
                                Err(e) => {
                                    // Typically something like a file perm issue
                                    eprintln!("File error for {:?} {}", &path, e);
                                    exit(1);
                                }
                                _ => ()
                            };
                        },
                        Err(e) => {
                            // Typically something like a directory perm issue
                            eprintln!("File error for {} {}", glob, e);
                            exit(1);
                        },
                    };
                }
            }
            Ok(self.line_handler(line_follower, file_set_name).await)
        }

        pub async fn line_handler( &self, mut line_follower: MuxedLines, file_set_name : &String ) {
            let monitors = &self.monitors;
            while let Ok(Some(line)) = line_follower.next_line().await {
                for monitor in monitors{
                    if monitor.regex.is_match(line.line()) {
                        println!("fileset: {}, source: {}, line: {}", &file_set_name, line.source().display(), line.line());
                    } else {
                        println!("Nerp {}", &file_set_name, );
                    }
                }
            }
        }
    }

    #[derive(Serialize, Deserialize)]
    pub(crate) struct Monitor{
        #[serde(with = "serde_regex")]
        pub(crate) regex: Regex,
        pub(crate) log_recent_events: Option<usize>,
        pub(crate) keep_lines_before: Option<usize>,
        pub(crate) keep_lines_after: Option<usize>,
        pub(crate) log_counts: bool,
    }

    #[derive(Serialize, Deserialize)]
    struct FileSetData{
        pub(crate) monitor_data: Vec<MonitorData>
    }

    #[derive(Serialize, Deserialize)]
    struct MonitorData{
        pub(crate) counts: Option<EventCounts>,
        pub(crate) recent_events: Option<Vec<MonitorEvent>>,
    }

    #[derive(Serialize, Deserialize)]
    struct EventCounts{
        pub(crate) seconds: HashMap<DateTime<Utc>, usize>,
        pub(crate) minutes: HashMap<DateTime<Utc>, usize>,
        pub(crate) hours: HashMap<DateTime<Utc>, usize>,
        pub(crate) days: HashMap<DateTime<Utc>, usize>,
        pub(crate) months: HashMap<DateTime<Utc>, usize>,
        pub(crate) years: HashMap<DateTime<Utc>, usize>,
    }

    #[derive(Serialize, Deserialize)]
    struct MonitorEvent{
        pub(crate) lines: Vec<LogLine>,
    }

    #[derive(Serialize, Deserialize)]
    struct LogLine{
        pub(crate) date: DateTime<Utc>,
        pub(crate) line: String,
    }



}

#[tokio::main]
async fn main() -> std::io::Result<()> {

    let mut file_sets = HashMap::new();
    file_sets.insert( "Syslog".to_string(), FileSet{
        file_globs: vec!["/var/log/sysl*".to_string()],
        monitors: vec![
            Monitor{
                regex: Regex::new(r"Started").unwrap(),
                keep_lines_before: Some(3),
                keep_lines_after: Some(4),
                log_counts: true,
                log_recent_events: Some(5)
            },
            Monitor{
                regex: Regex::new(r"gnome").unwrap(),
                keep_lines_before: None,
                keep_lines_after: None,
                log_counts: false,
                log_recent_events: Some(2)
            },
        ]
    } );
    file_sets.insert("Apache access".to_string(), FileSet{
        file_globs: vec!["/var/log/apache2/access.log".to_string()],
        monitors: vec![
            Monitor{
                regex: Regex::new(r"404").unwrap(),
                log_counts: true,
                keep_lines_after: None,
                keep_lines_before: None,
                log_recent_events: None
            }
        ]
    } );


    let conf = ConfigFile{
        file_sets: file_sets
    };

    let mut file_handler_futures: Vec<BoxFuture<Result<(),Box<dyn Error>>>> = Vec::new();

    for (file_set_name, file_set) in &conf.file_sets{
        let fut = file_set.follow(file_set_name, &file_set);
        file_handler_futures.push(Box::pin(fut.boxed()));
    }

    let _res = join_all( file_handler_futures ).await;

    Ok(())
}



