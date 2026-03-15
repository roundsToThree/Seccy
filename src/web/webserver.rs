// For serving UI interface to client + interfacing with camera server
// Note: I was more tolerant of unwrap usage here.

use crate::{storage::db::Application, stream::camera::Camera};
use rocket::{
    State, fs::{FileServer, relative}, get, http::{Method, Status}, routes, serde::{Deserialize, Serialize, json::Json}
};
use tokio::runtime::Runtime;

// Run within its own thread
// Launches webserver
pub fn start_webserver_service(application: Application) {
    // Initialise runtime
    let rt = Runtime::new().expect("Failed to create Tokio runtime");

    rt.block_on(async {
        // Launch webserver
        let _ = rocket::build()
            .manage(application)
            .mount("/api", routes![cameras])
            .mount("/", FileServer::from(relative!("static")))
            .launch()
            .await;
    });
}

#[derive(Serialize)]
struct CameraResponse {
    camera_id: u32,
    name: String,
    streams: Vec<StreamResponse>,
}

#[derive(Serialize)]
struct StreamResponse {
    stream_id: u32,
    stream_type: u8,
}

#[get("/cameras")]
fn cameras(state: &State<Application>) -> Json<Vec<CameraResponse>> {
    // Get cameras
    let cameras = state.cameras.read().unwrap();
    let cameras:Vec<CameraResponse> = cameras.values().map(|camera| {
        let streams = camera
            .streams
            .values()
            .map(|stream_lock| {
                // Open each stream lock, extract parameters or none
                let stream_lock = if let Ok(v) = stream_lock.read() {
                    v
                } else {
                    return None;
                };
                Some(StreamResponse {
                    stream_id: stream_lock.stream_id,
                    stream_type: stream_lock.stream_type,
                })
            })
            .filter_map(|s| s)
            .collect();

        CameraResponse {
            camera_id: camera.id,
            name: camera.name.clone(),
            streams: streams,
        }
    }).collect();

    Json::from(cameras)
}
