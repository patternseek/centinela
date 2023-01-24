use crate::data::FileSetData;
use crate::fileset::FileSetId;
use actix_web::{get, web, App, HttpResponse, HttpServer, Responder};
use log::info;
use serde_json::json;
use std::collections::HashMap;
use std::sync::Arc;
use tokio::sync::RwLock as RwLock_Tokio;

use std::io::Error;

/// HTTP GET a list of all the filesets
#[get("/fileset")]
pub(crate) async fn get_filesets(
    filesets_data_rwlock: web::Data<Arc<RwLock_Tokio<HashMap<FileSetId, FileSetData>>>>,
) -> impl Responder {
    let fileset_data = filesets_data_rwlock.read().await;
    HttpResponse::Ok().json(&fileset_data.keys().cloned().collect::<Vec<FileSetId>>())
}

/// HTTP GET a list of all the monitors for a given fileset
#[get("/fileset/{fileset_id}/monitor")]
pub(crate) async fn get_monitors_for_fileset(
    filesets_data_rwlock: web::Data<Arc<RwLock_Tokio<HashMap<FileSetId, FileSetData>>>>,
    fileset_id: web::Path<String>,
) -> impl Responder {
    let fileset_data = filesets_data_rwlock.read().await;
    if let Some(fileset) = fileset_data.get::<String>(&fileset_id) {
        HttpResponse::Ok().json(
            &fileset
                .monitor_data
                .keys()
                .cloned()
                .collect::<Vec<FileSetId>>(),
        )
    } else {
        HttpResponse::NotFound().json(json!({ "error": "fileset not found" }))
    }
}

/// Get a specific monitor for a specific fileset
#[get("/fileset/{fileset_id}/monitor/{monitor_id}")]
pub(crate) async fn get_monitor(
    filesets_data_rwlock: web::Data<Arc<RwLock_Tokio<HashMap<FileSetId, FileSetData>>>>,
    path: web::Path<(String, String)>,
) -> impl Responder {
    let fileset_data = filesets_data_rwlock.read().await;
    if let Some(fileset) = fileset_data.get::<String>(&path.0) {
        if let Some(monitor_data) = fileset.monitor_data.get(&path.1) {
            HttpResponse::Ok().json(&monitor_data)
        } else {
            HttpResponse::NotFound().json(json!({ "error": "monitor not found" }))
        }
    } else {
        HttpResponse::NotFound().json(json!({ "error": "fileset not found" }))
    }
}

/// Dump the entire in-memory data set
#[get("/dump")]
pub(crate) async fn dump(
    filesets_data_rwlock: web::Data<Arc<RwLock_Tokio<HashMap<FileSetId, FileSetData>>>>,
) -> impl Responder {
    let fileset_data = filesets_data_rwlock.read().await;
    HttpResponse::Ok().body(serde_json::to_string(&*fileset_data).expect("Couldn't serialise data"))
}

