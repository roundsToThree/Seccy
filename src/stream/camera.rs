use std::{collections::HashMap, sync::{Arc, Mutex, RwLock}};

use ffmpeg_next::Packet;
use ffmpeg::{Rational, codec::Parameters, format::context::Output};
use multiqueue2::BroadcastReceiver;

use crate::util::counters::RateCounter;



pub struct Camera {
    pub(crate) id: u32,
    pub(crate) name: String,

    // key is the stream ranking
    // 0 -> Main stream
    // 1 -> Sub stream... etc.
    pub(crate) streams: HashMap<u8, StreamHandle>
}

impl Camera {
    pub fn new(id: u32, name: String) -> Self{
        Camera {
            id: id,
            name: name,
            // main_stream: None,
            streams: HashMap::new()
        }
    }
}

// Struct for RTSP stream -> Input and output.
pub struct Stream {
    // Identifier allocated to stream for use in database management
    pub(crate) stream_id: u32,
    // Stream type (main stream, substream etc.)
    pub(crate) stream_type: u8,

    // RTSP Stream URL
    pub(crate) url: String,
    // Initially Nothing until the stream service starts capturing the stream.
    // Then the service broadcasts packets here.
    pub(crate) stream: Option<Mutex<BroadcastReceiver<Packet>>>,
    // pub(crate) stream_parameters: Arc<Mutex<Parameters>>,
    pub(crate) stream_parameters: Arc<Mutex<StreamParameters>>,

    // Initialised to default.
    pub(crate) metrics: Arc<RwLock<StreamMetrics>>,

    // Output stream configuration
    // pub(crate) output_properties: OutputProperties,
}

#[derive(Default)]
pub struct StreamParameters {
    pub(crate) parameters: Option<Parameters>,
    pub(crate) timebase: Option<Rational>,
}

pub type StreamHandle = Arc<RwLock<Stream>>;

impl Stream {
    pub fn new(stream_id: u32, url: String, stream_type: u8) -> Self {
        Stream {
            stream_id: stream_id,
            stream_type: stream_type,
            url: url,
            stream: None,
            stream_parameters: Arc::new(Mutex::new(StreamParameters::default())),
            metrics:  Arc::new(RwLock::new(StreamMetrics { bandwidth: RateCounter::default() })),
        }
    }
}


pub struct OutputProperties {
    // How often to write files to disk (in minutes)
    // 0 not permitted, 1m -> ~4 hours range
    pub(crate) max_record_length: u8,

    // File writer properties
    // Output directory
    // Output file numbering scheme
    // 

    // Or alternatively -> Request made to database which returns a file path to write to.
}

pub struct StreamMetrics {
    pub(crate) bandwidth: RateCounter,
}
