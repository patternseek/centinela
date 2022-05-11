use crate::data::FileSetData;
use crate::fileset::FileSetId;
use actix_web::{web, App, HttpResponse, HttpServer, Responder};
use log::info;
use std::collections::HashMap;
use std::sync::Arc;
use tokio::sync::RwLock as RwLock_Tokio;
use tokio::task::JoinHandle;

async fn dump(
    filesets_data_rwlock: web::Data<Arc<RwLock_Tokio<HashMap<FileSetId, FileSetData>>>>,
) -> impl Responder {
    let fileset_data = filesets_data_rwlock.read().await;
    HttpResponse::Ok().body(serde_json::to_string(&*fileset_data).expect("Couldn't serialise data"))
}

#[tokio::main]
async fn run_actix(
    filesets_data_rwlock: Arc<RwLock_Tokio<HashMap<FileSetId, FileSetData>>>,
) -> std::io::Result<()> {
    // console_subscriber::init();
    info!("Webserver starting");

    let wrapped_filesets_data_rwlock = web::Data::new(filesets_data_rwlock);

    let res = HttpServer::new(move || {
        App::new()
            .app_data(wrapped_filesets_data_rwlock.clone())
            .route("/dump", web::get().to(dump))
    })
    .bind(("127.0.0.1", 8080))?
    .run()
    .await;
    info!("Webserver exited");
    res
}

pub(crate) fn start_thread(
    filesets_data_rwlock: Arc<RwLock_Tokio<HashMap<FileSetId, FileSetData>>>,
) -> std::thread::JoinHandle<()> {
    std::thread::spawn(move || {
        let _ = run_actix(filesets_data_rwlock);
    })
}
