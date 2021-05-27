use regex::Regex;
use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::error::Error;
use std::fs::File;
use std::io::Read;
use url::Url;

pub fn load(config_path: String) -> Result<ConfigFile, Box<dyn Error>> {
    let mut file = File::open(config_path)?;
    let mut contents = String::new();
    file.read_to_string(&mut contents)?;
    let res = serde_yaml::from_str(&contents)?;
    Ok(res)
}

#[derive(Serialize, Deserialize)]
pub struct ConfigFile {
    pub file_sets: HashMap<String, FileSetConfig>,
}

#[derive(Serialize, Deserialize)]
pub struct FileSetConfig {
    pub file_globs: Vec<String>,
    pub monitors: HashMap<String, MonitorConfig>,
}

#[derive(Serialize, Deserialize, Clone)]
pub struct MonitorConfig {
    #[serde(with = "serde_regex")]
    pub regex: Regex,
    pub log_recent_events: Option<usize>,
    pub keep_lines_before: Option<usize>,
    pub keep_lines_after: Option<usize>,
    pub log_counts: bool,
    pub notifiers: Option<Vec<NotifierConfig>>,
}

#[derive(Serialize, Deserialize, Clone)]
#[serde(rename_all = "snake_case")]
pub enum NotifierConfig {
    Webhook(WebhookNotifierConfig),
}

#[derive(Serialize, Deserialize, Clone)]
pub struct WebhookNotifierConfig {
    pub(crate) url: Url,
    pub(crate) template: String,
    pub(crate) minimum_interval: Option<usize>,
    pub(crate) wait_seconds_for_additional_lines: Option<usize>,
}
