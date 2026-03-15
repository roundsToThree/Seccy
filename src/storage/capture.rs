use crate::storage::db::{Application, RecordingStatusCode};
use crate::storage::storage::get_file_size;
use crate::stream::camera::Stream;
use ffmpeg::codec::Parameters;
use ffmpeg::{Error, Packet};
use multiqueue2::BroadcastReceiver;
use std::sync::RwLock;
use std::time::{Duration, Instant};
use std::{fs, thread};

pub fn store_stream(
    application: &Application,
    stream: &RwLock<Stream>,
) -> Result<(), ffmpeg::Error> {
    println!("Initialising Stream Storage");

    // Acquire lock of stream to intialise
    let Ok(mut stream_lock) = stream.write() else {
        return Err(ffmpeg::Error::Unknown);
    };

    // Subscribe to the stream
    let broadcast = if let Some(v) = &stream_lock.stream {
        v.clone()
    } else {
        return Err(ffmpeg::Error::StreamNotFound);
    };

    let rx = if let Ok(v) = broadcast.lock() {
        v.add_stream()
    } else {
        return Err(ffmpeg::Error::StreamNotFound);
    };

    // Get stream metadata
    let stream_params = stream_lock.stream_parameters.lock().unwrap();
    // We only care for stream parameters
    let params = if let Some(v) = stream_params.parameters.clone() {
        v
    } else {
        return Err(ffmpeg::Error::Unknown);
    };
    drop(stream_params);

    // Get stream id for database requests
    let stream_id = stream_lock.stream_id;

    // Release the lock
    drop(stream_lock);

    // Access applicaation config to get recording duration
    let config = if let Ok(v) = application.config.read() {
        v
    } else {
        return Err(Error::Unknown);
    };
    let record_duration = Duration::from_mins(config.max_record_length as u64);
    drop(config);

    // Record on repeat
    loop {
        // Request a file name for the video
        let (video_file_path, thumbnail_file_path, recording_id) =
            if let Some(v) = application.allocate_recording_filename(stream_id) {
                v
            } else {
                return Err(Error::Unknown);
            };

        // Record segment
        let res = store_stream_segment(
            video_file_path.as_str(),
            &rx,
            record_duration,
            params.clone(),
        );

        let file_size = get_file_size(video_file_path.as_str());

        // Mark file as closed in database
        let mut status_code = RecordingStatusCode::RecordingComplete;
        let mut recorded_duration = None;
        match res {
            Ok(duration) => {
                recorded_duration = Some(duration.as_millis() as u64);
                if duration < record_duration {
                    status_code = RecordingStatusCode::RecordingInterrupted;
                    println!("Stream ended early, only recorded for {:?}", duration);
                }
            }
            Err(v) => {
                status_code = RecordingStatusCode::RecordingFailed;
                println!("Error: {:?}", v);
            }
        }

        let res2 = application.mark_recording_complete(
            recording_id,
            recorded_duration,
            file_size,
            status_code,
        );

        if res.is_err() || res2.is_err() {
            return Err(Error::Unknown);
        }
    }

    Ok(())
}

fn store_stream_segment(
    path: &str,
    stream: &BroadcastReceiver<Packet>,
    duration: Duration,
    params: Parameters,
) -> Result<Duration, ffmpeg::Error> {
    println!("Recording file as {}", path);

    // Prepare the FFMPEG stream
    let mut ctx = ffmpeg::format::output(path)?;

    // TODO: Handle variable encoder type depending on the stream
    let video_encoder = ffmpeg::encoder::find(ffmpeg::codec::Id::H265).expect("Encoder not found");
    let mut output_video_stream = ctx.add_stream(video_encoder)?;
    output_video_stream.set_parameters(params);

    ctx.write_header()?;

    println!("Stream writer intialised");

    let start_time = Instant::now();
    let deadline = start_time + duration;

    // Rescale the output
    let mut start_pts: Option<i64> = None;
    let mut start_dts: Option<i64> = None;
    let mut last_dts: Option<i64> = None; 

    while deadline >= Instant::now() {
        let packet = stream.try_recv();

        // Handle packet
        match packet {
            // Packet recieved, add to file
            Ok(mut packet) => {
                // If we haven't found the first keyframe yet, wait for it
                if start_pts.is_none() {
                    // Check if keyframe
                    if packet.is_key() {
                        // Reference timescale to this keyframe
                        start_pts = packet.pts();
                        start_dts = packet.dts();
                    } else {
                        // Ignore packet
                        // TODO: Instead of dropping, find a way to move it to the next segment.
                        // Maybe when ending a segment, wait till keyframe and then move that for the start of the next seg.
                        continue;
                    }
                }

                // Modify packet timescale so the output starts at 00:00
                if let (Some(start_p), Some(start_d)) = (start_pts, start_dts) {
                    packet.set_pts(packet.pts().map(|pts| pts - start_p));
                    packet.set_dts(packet.dts().map(|dts| dts - start_d));
                }

                // If DTS is still None after offset, derive it from PTS
    if packet.dts().is_none() {
        packet.set_dts(packet.pts());
    }

    // Now enforce monotonically increasing DTS — DTS is guaranteed Some() here
    let current_dts = match packet.dts() {
        Some(dts) => dts,
        None => {
            println!("Skipping packet with no DTS and no PTS");
            continue;
        }
    };

    if let Some(prev_dts) = last_dts {
        if current_dts <= prev_dts {
            // Clamp rather than skip — preserves keyframes
            let clamped_dts = prev_dts + 1;
            packet.set_dts(Some(clamped_dts));
            // PTS must never be less than DTS
            if packet.pts().map_or(true, |pts| pts < clamped_dts) {
                packet.set_pts(Some(clamped_dts));
            }
            println!(
                "{} Clamped out-of-order dts {} -> {} (prev: {})", path,
                current_dts, clamped_dts, prev_dts
            );
        }
    }

    // Update last_dts AFTER any clamping
    last_dts = packet.dts();

                packet.set_stream(0);
                // Write to file

                packet.write_interleaved(&mut ctx)?;
            }

            // Stream could be closed or maybe a packet isn't ready yet
            Err(err) => {
                match err {
                    std::sync::mpsc::TryRecvError::Empty => {
                        // Wait a bit (Maybe should tune the delay)
                        thread::sleep(Duration::from_millis(100));
                    }
                    std::sync::mpsc::TryRecvError::Disconnected => {
                        // Stream closed!
                        return Err(ffmpeg::Error::StreamNotFound);
                    }
                }
            }
        }
    }

    // To confirm upstream that the stream recorded to completion.
    let recorded_duration = Instant::now() - start_time;

    // End the file.
    ctx.write_trailer()?;
    println!("File closed");

    Ok(recorded_duration)
}
