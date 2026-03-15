use crate::stream::camera::{Stream};
use crate::storage::capture::{store_stream};
use crate::util::counters::PerformanceCounter;
use std::sync::{Arc, Mutex, RwLock};
use std::thread;
use std::time::{Duration, Instant, SystemTime};


/// Given a stream, begins streaming the URL into the stream broadcast as FFMPEG packets
/// TODO: if the stream is over a slow network (tested with poor mobile hotspot over VPN from a different country)
///                                             [seconds of latency - kbps link]
/// the service can hog the lock for the stream while ffmpeg waits to open the stream. Maybe make the lock shorter lived?
pub fn capture_stream(stream: &RwLock<Stream>) -> Result<(), ffmpeg::Error> {
    println!("Initialising Stream Capture");

    // Acquire lock of stream to intialise
    let Ok(mut stream_lock) = stream.write() else {
        return Err(ffmpeg::Error::Unknown);
    };

    // Use TCP for streaming to not miss packets
    // Possibly make this optional?
    let mut input_dictionary = ffmpeg::Dictionary::new();    
    input_dictionary.set("rtsp_transport", "tcp");
    let url = stream_lock.url.clone();
    // Copy URL out and drop the lock as loading the FFMPEG stream can take some time and we don't want to halt the program
    // While it connects and sets up.
    drop(stream_lock);

    let mut ctx = ffmpeg::format::input_with_dictionary(&url, input_dictionary)?;

    // Regular UDP.
    // let mut ctx = ffmpeg::format::input(&stream_lock.url)?;

    let input = ctx
        .streams()
        .best(ffmpeg::media::Type::Video)
        .ok_or(ffmpeg::Error::StreamNotFound)?;
    let video_stream_index = input.index();

    // Acquire lock of stream again to intialise
    let Ok(mut stream_lock) = stream.write() else {
        return Err(ffmpeg::Error::Unknown);
    };

    // An unfortunate way of sharing the FFMPEG parameters without unsafe code
    let mut stream_parameters = stream_lock.stream_parameters.lock().unwrap();
    // Store stream parameters [Used in saving stream]
    stream_parameters.parameters = Some(input.parameters().clone());
    // Store stream timebase   [Used in WebRTC]
    stream_parameters.timebase = Some(input.time_base());
    drop(stream_parameters);

    // Create the broadcast for the stream
    // TODO: should q size be 128?
    let (tx, rx) = multiqueue2::broadcast_queue(1024);
    stream_lock.stream = Some(Mutex::new(rx.clone()));

    // Get a reference to add stream metrics
    let metrics = stream_lock.metrics.clone();

    // Release the lock
    drop(stream_lock);

    // Start packet ingest
    println!("Ingest started");
    // let mut count = 0;
    let iter = ctx.packets();
    for (s, packet) in iter {
        if s.index() == video_stream_index {
            let size = packet.size();

            // Broadcast packet to recievers (like live webstream, storage service, maybe AI ?)
            match tx.try_send(packet) {
                Ok(_) => {} // Do nothing if it works
                Err(v) => {
                    // TODO: Handle broadcast errors better.
                    eprintln!("Failed to broadcast packet {:?}", v);
                    match v {
                        std::sync::mpsc::TrySendError::Full(_) => {
                            // Buffer full... Perhaps no one is listening?
                            // Start dropping packets
                            thread::sleep(Duration::from_secs(1));
                        }
                        std::sync::mpsc::TrySendError::Disconnected(_) => {
                            // Should not logically happen. Exit and restart.
                            return Err(ffmpeg::Error::Unknown);
                        }
                    }
                }
            }

            // This broadcast library wont let a shared rx reference exist that isn't being consumed
            // Frustratingly it also requires all consumers consume otherwise it blocks the tx instead of drops packets
            // Probably need a different broadcaster in the future. This is a tacky fix
            let _ = rx.try_recv();

            // Update stream performance metrics
            let Ok(mut metric_lock) = metrics.write() else {
                continue;
            };
            metric_lock.bandwidth.push(size as f32);
            drop(metric_lock); // for clarity.
        }

    }

    println!("Terminating Stream");

    // Close broadcast by dropping references
    drop(tx);
    let Ok(mut stream_lock) = stream.write() else {
        return Err(ffmpeg::Error::Unknown);
    };
    stream_lock.stream = None;

    println!("Finished capture");
    Ok(())
}
