use ffmpeg::codec::Parameters;
use ffmpeg::format::Context;
use ffmpeg::format::context::Input;
use ffmpeg::sys::AVCodecParameters;
use ffmpeg::{codec, encoder};
use ffmpeg_next::Packet;
use std::error::Error;
use std::sync::{Arc, Mutex};
use std::time::{Duration, SystemTime};
use std::sync::RwLock;
use tokio::sync::broadcast;
use tokio::sync::broadcast::error::SendError;
use tokio::sync::broadcast::{Receiver, Sender};
use tokio::time::sleep;

struct Camera {
    main_stream: Stream,
}

struct Stream {
    // RTSP Stream URL
    url: String,
    // Initially Nothing until the stream service starts capturing the stream.
    // Then the service broadcasts packets here.
    stream: Option<Sender<Packet>>,
    stream_parameters: Arc<Mutex<Parameters>>,

    // Initialised to default.
    metrics: Arc<RwLock<StreamMetrics>>,
}

struct StreamMetrics {
    bandwidth: f32,
}

pub async fn test() {
    let mut stream = Stream {
        url: "rtsp://10.8.0.2:554/test.mp4".to_string(),
        stream: None,
        stream_parameters: Arc::new(Mutex::new(Parameters::default())),
        metrics: Arc::new(RwLock::new(StreamMetrics { bandwidth: 0.0 })),
    };

    let ls = Arc::new(RwLock::new(stream));

    let l3 = ls.clone();
    tokio::spawn(async move {
        let l4 = Arc::clone(&l3);
        let l2 = l4.as_ref();
        // capture_stream(l2).expect("TODO: panic message");
        capture_stream(l2).await.expect("TODO: panic message");
    });

    let l4 = ls.clone();
    tokio::spawn(async move {
        loop {
            println!("Waiting for lock 1");
            let lock = l4.read().unwrap();
            println!("Waiting for lock 2");
            let lock2 = lock.metrics.read().unwrap();
            let bandwidth_average = lock2.bandwidth;
            println!("{} bytes per second", bandwidth_average);
            drop(lock2);
            drop(lock);
            // sleep(Duration::from_secs(1)).await;
        }
    });

    sleep(Duration::from_secs(10)).await;
    store_stream(&ls).await.expect("TODO: panic message");
}



async fn store_stream(stream: &RwLock<Stream>) -> Result<(), ffmpeg::Error> {
    // Subscribe to the stream to get live packets
    let stream = stream.read().unwrap();
    let mut rx = match &stream.stream {
        None => {
            return Err(ffmpeg::Error::StreamNotFound);
        }
        Some(v) => v.subscribe(),
    };
    let params = stream.stream_parameters.lock().unwrap().clone();
    let metrics = stream.metrics.clone();
    // let params = (params).clone();
    drop(stream);

    // Initialise output
    let mut output_context = ffmpeg::format::output(&"./test.mp4")?;
    let video_encoder = encoder::find(codec::Id::H264).expect("Encoder not found");
    let mut output_video_stream = output_context.add_stream(video_encoder)?;
    output_video_stream.set_parameters(params);

    output_context.write_header()?;

    let mut bandwidth_average = 0; // Counts 1s average
    let mut bandwidth_time = SystemTime::now(); // Time bandwidth averaged to

    println!("Stream writer intialised");

    loop {
        match rx.recv().await {
            Ok(packet) => {
                // println!("Received: packet {}", packet.size());

                bandwidth_average += packet.size();
                packet.write_interleaved(&mut output_context)?;

                let now = SystemTime::now();
                if now.duration_since(bandwidth_time).expect("TODO: handle?")
                    > Duration::from_secs(1)
                {
                    let _ = update_stream_bandwidth(metrics.clone(), bandwidth_average as f32);

                    bandwidth_average = 0;
                    bandwidth_time = now;
                }
            }
            Err(broadcast::error::RecvError::Lagged(v)) => {
                eprintln!("Receiver lagged, catching up... {}", v);
                // The receiver's internal cursor is automatically updated to the oldest message
                continue;
            }
            Err(broadcast::error::RecvError::Closed) => {
                println!("Channel closed, no more senders.");
                break; // Exit the loop
            }
        }
    }

    // Close file
    output_context.write_trailer()?;

    Ok(())
}

async fn update_stream_bandwidth(metrics: Arc<RwLock<StreamMetrics>>, bandwidth: f32) {
    println!("Updating bw");
    // Locking per second may be bad?
    let mut metric_lock = metrics.write().await;
    metric_lock.bandwidth = bandwidth;
    drop(metric_lock);
    println!("done bw");
}

async fn open_stream(stream_: &RwLock<Stream>) -> Result<Input, ffmpeg::Error> {
    let mut stream = stream_.write().await;
    println!("Init Capture Stream");
    let mut ictx = ffmpeg::format::input(&stream.url)?;

    // let p = input.parameters().medium();
    let input = ictx
        .streams()
        .best(ffmpeg::media::Type::Video)
        .ok_or(ffmpeg::Error::StreamNotFound)?;
    stream.stream_parameters = Arc::new(Mutex::new(input.parameters().clone()));
    // let p = ffmpeg::codec::context::Context::from_parameters(input.parameters())?;

    drop(stream);

    Ok(ictx)
}

async fn create_broadcast(stream_: &RwLock<Stream>) -> Sender<Packet> {
    let mut stream = stream_.write().await;

    // Create broadcast
    if stream.stream.is_some() {
        // Dismantle existing stream
    }
    let (tx, mut _rx) = broadcast::channel::<Packet>(16);
    stream.stream = Some(tx.clone());
    drop(stream);

    tx
}

async fn capture_stream(stream_: &RwLock<Stream>) -> Result<(), ffmpeg::Error> {
    let tx = create_broadcast(stream_).await;

    let mut ictx = open_stream(stream_).await?;
    let input = ictx
        .streams()
        .best(ffmpeg::media::Type::Video)
        .ok_or(ffmpeg::Error::StreamNotFound)?;
    let video_stream_index = input.index();
    println!("Found video stream at index: {}", video_stream_index);



    let mut count = 0;
    for (s, packet) in ictx.packets() {
        if s.index() == video_stream_index {
            match tx.send(packet) {
                Ok(_) => {}
                Err(err) => {
                    // println!("Failed to broadcast stream");
                    // println!("{}", err);
                }
            }
        }

        count += 1;
        if count > 5000 {
            break;
        }
    }

    eprintln!("Terminating Stream");
    tx.closed();
    close_broadcast(stream_);

    eprintln!("...");
    Ok(())
}

async fn close_broadcast(stream: &RwLock<Stream>) {
    let mut s = stream.write().await;
    s.stream = None;
}
