use crate::data::FileSetData;
use crate::fileset::FileSetId;
use actix_web::{get, web, App, HttpResponse, HttpServer, Responder};
use log::info;
use serde_json::json;
use std::collections::HashMap;
use std::sync::Arc;
use tokio::sync::RwLock as RwLock_Tokio;

/// HTTP GET a list of all the filesets
#[get("/fileset")]
async fn get_filesets(
    filesets_data_rwlock: web::Data<Arc<RwLock_Tokio<HashMap<FileSetId, FileSetData>>>>,
) -> impl Responder {
    let fileset_data = filesets_data_rwlock.read().await;
    HttpResponse::Ok().json(&fileset_data.keys().cloned().collect::<Vec<FileSetId>>())
}

/// HTTP GET a list of all the monitors for a given fileset
#[get("/fileset/{fileset_id}/monitor")]
async fn get_monitors_for_fileset(
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
async fn get_monitor(
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
async fn dump(
    filesets_data_rwlock: web::Data<Arc<RwLock_Tokio<HashMap<FileSetId, FileSetData>>>>,
) -> impl Responder {
    let fileset_data = filesets_data_rwlock.read().await;
    HttpResponse::Ok().body(serde_json::to_string(&*fileset_data).expect("Couldn't serialise data"))
}

/// Start the HTTP API
#[tokio::main]
async fn run_actix(
    filesets_data_rwlock: Arc<RwLock_Tokio<HashMap<FileSetId, FileSetData>>>,
) -> std::io::Result<()> {
    info!("Webserver starting");

    let wrapped_filesets_data_rwlock = web::Data::new(filesets_data_rwlock);

    let res = HttpServer::new(move || {
        App::new()
            .app_data(wrapped_filesets_data_rwlock.clone())
            .service(get_filesets)
            .service(get_monitors_for_fileset)
            .service(get_monitor)
            .service(dump)
    })
    .bind(("127.0.0.1", 8694))?
    .run()
    .await;
    info!("Webserver exited");
    res
}

/// Start the thread that contains the HTTP API
pub(crate) fn start_thread(
    filesets_data_rwlock: Arc<RwLock_Tokio<HashMap<FileSetId, FileSetData>>>,
) -> std::thread::JoinHandle<()> {
    std::thread::spawn(move || {
        let _ = run_actix(filesets_data_rwlock);
    })
}
