use crate::{
    storage::db::Application,
    web::webrtc::{WebRTC, open_web_stream},
};
use std::{
    net::{IpAddr, Ipv4Addr, TcpListener},
    sync::Arc,
    thread,
};
use tokio::runtime::Runtime;
use tungstenite::{Message, accept};

/*
    WS Overview

    > Open WebRTC stream    [Bidirectional]
        -> Live
        -> Old video
            [Also allow scrubbing]
    > Events    [Broadcast only]
        -> Recording start/stop/failed
        -> Errors
        -> Motion etc...


    Websocket structure
    Colon delimited?

    Inbound

    RTC:OPEN:0.0:ajKEeV1uj.....                                   SDP Offer from client for camera 0 stream 0
                                RTC:OPEN:0.0:kKe&vGs....          Server response
    RTC:CLOSE:0.0                                                 Close WebRTC channel
*/

// Starts listening for websocket connections for the webUI
pub fn start_websocket_service(application: Application) {
    let config_lock = if let Ok(v) = application.config.read() {
        v
    } else {
        return;
    };

    let addr = config_lock.web_server_addr.as_str();
    let port = config_lock.websocket_port;

    let server = if let Ok(v) = TcpListener::bind((addr, port)) {
        v
    } else {
        return;
    };
    drop(config_lock);

    // Configure WebRTC
    let webrtc = if let Some(v) = WebRTC::new() {
        Arc::new(v)
    } else {
        return;
    };

    println!("Websocket listening for connections ...");
    for stream in server.incoming() {
        let app = application.clone();
        let webrtc_clone = webrtc.clone();
        thread::spawn(move || {
            // Start runtime for tokio
            let rt = Runtime::new().unwrap();
            rt.block_on(async {
                println!("New WS connection");

                // TODO: too verbose?
                let stream = if let Ok(v) = stream {
                    v
                } else {
                    eprintln!("Failed to open connection");
                    return;
                };

                let mut websocket = if let Ok(v) = accept(stream) {
                    v
                } else {
                    eprintln!("Failed to open websocket");
                    return;
                };

                loop {
                    let msg = if let Ok(v) = websocket.read() {
                        v
                    } else {
                        break;
                    };

                    if msg.is_binary() || msg.is_text() {
                        // Interpret request
                        let msg = if let Ok(v) = msg.into_text() {
                            v
                        } else {
                            continue;
                        };
                        let response =
                            handle_websocket_request(&webrtc_clone, &app, msg.as_str()).await;
                        // Respond if applicable
                        if let Some(res) = response {
                            let _ = websocket.send(Message::text(res));
                        }
                    }
                }
                println!("WS Connection closed");
            });
        });
    }
}

async fn handle_websocket_request(
    webrtc: &WebRTC,
    application: &Application,
    request: &str,
) -> Option<String> {
    // Split on :
    let segments: &Vec<_> = &request.split(':').collect();
    dbg!(segments);

    match segments[0] {
        "RTC" => {
            println!("RTC Type request");

            // See if its an open or close
            if segments.len() >= 2 {
                if segments[1] == "OPEN" {
                    // Needs 3 arguments
                    println!("{}", segments.len());
                    if segments.len() != 4 {
                        return None;
                    }

                    // Arg 1 is cam-stream
                    let (camera_id, stream_type) =
                        if let Some(v) = get_cam_identifier_from_string(segments[2]) {
                            v
                        } else {
                            return None;
                        };
                    println!("CamID: {camera_id}\tStream Type: {stream_type}");
                    let stream = if let Some(v) =
                        application.get_stream_by_cam_and_type(camera_id, stream_type)
                    {
                        v
                    } else {
                        return None;
                    };
                    println!("Stream acq.");

                    // Arg 2 is offer
                    let offer = segments[3];

                    let sdp_answer = if let Ok(v) = open_web_stream(webrtc, application, stream, offer).await {v} else {return None;};
                    

                    return Some(format!(
                        "RTC:OPEN:{}.{}:{}",
                        camera_id, stream_type, sdp_answer
                    ));
                } else if segments[1] == "CLOSE" {
                }
            }
        }
        _ => {
            // Unknown
        }
    }
    None
}

// Returns the camera id and stream type from the "A.B" format
fn get_cam_identifier_from_string(identifier: &str) -> Option<(u32, u8)> {
    let idents: Vec<u32> = identifier
        .split('.')
        .map(|e| e.parse::<u32>())
        .filter_map(|e| e.ok())
        .collect();
    dbg!(&idents);
    if idents.len() != 2 {
        return None;
    }
    Some((idents[0], idents[1] as u8))
}
