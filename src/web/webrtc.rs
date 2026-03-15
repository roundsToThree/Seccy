/*

WS listener -> request to open stream made
            -> WebRTC returns key   [open_web_stream returns key and spawns thread]
            -> Stream begins


*/

use std::{
    error::Error,
    sync::{Arc, RwLock},
    time::Duration,
};

use base64::{Engine, prelude::BASE64_STANDARD};
use ffmpeg::{Packet, Rational};
use webrtc::{
    api::{
        API, APIBuilder,
        interceptor_registry::register_default_interceptors,
        media_engine::{MIME_TYPE_H264, MediaEngine},
        // media_engine::{MIME_TYPE_H265, MediaEngine},

    },
    ice_transport::ice_server::RTCIceServer,
    interceptor::registry::Registry,
    media::Sample,
    peer_connection::{
        RTCPeerConnection, configuration::RTCConfiguration,
        peer_connection_state::RTCPeerConnectionState,
        sdp::session_description::RTCSessionDescription,
    },
    rtp_transceiver::rtp_codec::RTCRtpCodecCapability,
    track::track_local::{TrackLocal, track_local_static_sample::TrackLocalStaticSample},
};

use crate::{
    storage::db::Application,
    stream::camera::{Stream, StreamHandle},
};

pub struct WebRTC {
    config: RTCConfiguration,
    api: API,
}

impl WebRTC {
    pub fn new() -> Option<Self> {
        // Clarity: This was made with the help of gemini cause ive never used WebRTC before.
        // Might be unoptimised or improper

        let mut engine = MediaEngine::default();
        if engine.register_default_codecs().is_err() {
            return None;
        }

        let mut registry = Registry::new();
        registry = if let Ok(v) = register_default_interceptors(registry, &mut engine) {
            v
        } else {
            return None;
        };

        let api = APIBuilder::new()
            .with_media_engine(engine)
            .with_interceptor_registry(registry)
            .build();

        let config = RTCConfiguration {
            ice_servers: vec![RTCIceServer {
                urls: vec![
                // "stun:stun.l.google.com:19302".to_owned()
                ],
                ..Default::default()
            }],
            ..Default::default()
        };

        Some(WebRTC { config, api })
    }

    pub async fn peer_connection(
        &self,
        video_track: Arc<TrackLocalStaticSample>,
    ) -> Result<RTCPeerConnection, Box<dyn Error>> {
        let peer_connection = self.api.new_peer_connection(self.config.clone()).await?;
        // let peer_connection = Arc::new(peer_connection);

        // Add the track to the Peer Connection
        peer_connection
            .add_track(Arc::clone(&video_track) as Arc<dyn TrackLocal + Send + Sync>)
            .await?;

        Ok(peer_connection)
    }
}

// Given a WebRTC client offer and a handle to a stream, will:
// - spawn a worker to stream packets to the client
//      (Until connection cloesed or worker dies)
// - Return the offer answer thing back.
// - If anything goes wrong, nothing spawned lives and "err" is returned.
pub async fn open_web_stream(
    webrtc: &WebRTC,
    application: &Application,
    stream: StreamHandle,
    offer: &str,
) -> Result<String, Box<dyn Error>> {
    // Acquire lock of stream
    // ? notation weird here, idk why. Had to break into if let.
    let Ok(mut stream_lock) = stream.write() else {
        return Err(Box::new(ffmpeg::Error::Unknown));
    };

    // Subscribe to the stream
    let broadcast: &std::sync::Mutex<multiqueue2::BroadcastReceiver<ffmpeg::Packet>> =
        if let Some(v) = &stream_lock.stream {
            v.clone()
        } else {
            return Err(Box::new(ffmpeg::Error::StreamNotFound));
        };

    let rx = if let Ok(v) = broadcast.lock() {
        v.add_stream()
    } else {
        return Err(Box::new(ffmpeg::Error::StreamNotFound));
    };
    println!("Locked stream");

    // Some parameters to name the rtc track
    let stream_id = stream_lock.stream_id;
    let stream_type = stream_lock.stream_type;

    // Get stream parameters
    let time_base = if let Ok(v) = stream_lock.stream_parameters.lock() {
        v.timebase
    } else {
        None
    };

    drop(stream_lock);

    // Create the Video Track
    let video_track = Arc::new(TrackLocalStaticSample::new(
        RTCRtpCodecCapability {
            mime_type: MIME_TYPE_H264.to_owned(),
            // mime_type: MIME_TYPE_H264.to_owned(),
            ..Default::default()
        },
        stream_id.to_string(),                // Stream ID
        format!("{stream_id}-{stream_type}"), // Track ID
    ));

    // Create peer connection
    let peer_connection = webrtc.peer_connection(video_track.clone()).await?;

    // Generate a response to the offer
    let offer = String::from_utf8(BASE64_STANDARD.decode(offer)?)?;
    let desc: RTCSessionDescription = serde_json::from_str(&offer)?;
    peer_connection.set_remote_description(desc).await?;

    let answer = peer_connection.create_answer(None).await?;
    let mut gather_complete = peer_connection.gathering_complete_promise().await;
    peer_connection.set_local_description(answer).await?;
    let _ = gather_complete.recv().await;

    // I dont like this way of returning "err" but i just want some code runnign for now.
    let mut answer = "err".to_string();
    if let Some(local_desc) = peer_connection.local_description().await {
        let json_str = serde_json::to_string(&local_desc)?;
        let b64 = BASE64_STANDARD.encode(json_str);
        answer = b64;
    }

    println!("Answer ready");

    // Prepare to send frames
    tokio::spawn(async move {
        let track = video_track.clone();
        // let timebase = time_base.clone();
        println!("Spawn");

        // Stay alive as long as the stream is active or
        // as long as the client is listening
        for packet in rx {
            let con_state = peer_connection.connection_state();
            match con_state {
                RTCPeerConnectionState::Closed
                | RTCPeerConnectionState::Disconnected
                | RTCPeerConnectionState::Failed => {
                    println!("RTC Stream ended");
                    return;
                }
                RTCPeerConnectionState::New
                | RTCPeerConnectionState::Unspecified
                | RTCPeerConnectionState::Connecting => {
                    // Discard packets while the connection is still not ready
                    continue;
                }
                _ => {}
            }
            send_stream_packet(packet, &time_base, &track).await;
        }
    });

    Ok(answer)
}

async fn send_stream_packet(
    packet: Packet,
    time_base: &Option<Rational>,
    track: &TrackLocalStaticSample,
) {
    let duration_ms = if packet.duration() > 0 && time_base.is_some() {
        // Maybe better way to write this.
        (packet.duration() as f64 * time_base.unwrap().numerator() as f64
            / time_base.unwrap().denominator() as f64
            * 1000.0) as u128
    } else {
        33
    };

    let duration: Duration = Duration::from_millis(duration_ms as u64);

    let data = packet.data().unwrap();

    let sample = Sample {
        data: data.to_vec().into(), // Bytes
        duration,
        ..Default::default()
    };

    track.write_sample(&sample).await;
}
