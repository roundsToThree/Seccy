/*
Some design notes:

# database functions typically return None instead of Error
This is because i feel like error handling can be made simpler this way.
If upstream gets a "null" or an empty list, they dont need to do any logging etc.
This, however, means i probably need to go back through these functions and add logging to make it clear something has gone wrong.
*/

use std::{
    collections::HashMap,
    fs,
    net::{IpAddr, Ipv4Addr},
    sync::{Arc, Mutex, RwLock},
    thread::{Scope, ScopedJoinHandle},
    time::{Duration, SystemTime},
};

use chrono::Local;
use rusqlite::{Connection, Error, Params, ParamsFromIter, params, params_from_iter};

use crate::{storage::storage::delete_file, util::workers::ThreadManager};
use crate::{
    storage::storage::format_bytes,
    stream::camera::{Camera, Stream},
};

#[repr(i64)]
#[derive(Clone)]
pub enum RecordingStatusCode {
    RecordingStarted = 0,

    RecordingComplete = 10,
    RecordingInterrupted = 11, // Error occured
    RecordingFailed = 12,

    MarkedForRemoval = 20,

    ThumbnailOnly = 30, // Video has been deleted, thumbnail remains
}

#[derive(Debug)]
pub struct Recording {
    pub(crate) recording_id: i64,
    pub(crate) path: Option<String>,
    pub(crate) file_size: Option<u64>,
    // Recording Status Code TODO
}

#[derive(Clone)]
pub struct Application {
    pub(crate) db: Arc<Database>,
    pub(crate) config: Arc<RwLock<ApplicationConfig>>,
    pub(crate) cameras: Arc<RwLock<HashMap<u32, Camera>>>,
    pub(crate) workers: Arc<Mutex<ThreadManager>>,
}

pub struct Database {
    connection: Mutex<Connection>,
}

#[derive(Default)]
pub struct ApplicationConfig {
    // File location
    pub(crate) db_path: String,    // Where the database is located
    pub(crate) media_path: String, // Where videos and photos are stored.

    // Recording settings
    pub(crate) max_record_length: u8, // How long each video file should be in minutes
    pub(crate) RTSP_over_TCP: bool, // Whether the streams should be over TCP (More latency but less glitching)
    // TODO: OutOfOrder packet handling (min heap)

    // Recording storage
    pub(crate) video_output_extension: String, // The file extension for recorded files
    pub(crate) thumbnail_output_extension: String, // The file extension for thumbnails
    pub(crate) timestamp_format: String, // Based on the Chrono package, how time should be formatted

    // File Management
    pub(crate) minimum_buffer_space: u64, // How much buffer space (bytes) should be left to accomodate file writes in between cleanup intervals
    pub(crate) management_interval: Duration, // How frequently to look for old files to remove
    pub(crate) minimum_purge_size: u64, // The minimum number of bytes of old recordings to remove when disk usage reaches limit (split between each stream)
    pub(crate) limit_disk_usage: Option<u64>, // Whether to place a limit on how much storage is used. None to use all available disk space
    pub(crate) stream_storage_distribution: HashMap<u8, f64>, // What percentage of the disk should each stream type be allocated. 0 => 0.5 would give half the disk space for main streams
    // TODO: Distribution between stream stypess

    // Web Settings
    pub(crate) web_server_addr: String, // Where the web UI can be accessed
    pub(crate) web_server_port: u16,    // The port for the Web UI
    pub(crate) websocket_port: u16, // The port for the websocket channel (Signalling for UI and WebRTC)
}

impl Application {
    pub fn new() -> Result<Self, Error> {
        let mut config = ApplicationConfig::default();

        // Todo: Change from some .conf file.
        config.db_path = "./database.db".to_string();
        // config.db_path = "./database.db".to_string();
        config.video_output_extension = "mp4".to_string();
        config.thumbnail_output_extension = "jpg".to_string();
        config.timestamp_format = "%d-%m-%Y_%H:%M:%S_%Z".to_string();
        config.media_path = "/storage/media".to_string();


        config.max_record_length = 15; // (15minutes)

        config.minimum_buffer_space = 10000000000; // 10GB
        config.management_interval = Duration::from_mins(10);
        config.minimum_purge_size = 20000000000; // 20GB
        config.limit_disk_usage = Some(35000000000000); // 35TB ish
        config.stream_storage_distribution = HashMap::new();
        config.stream_storage_distribution.insert(0, 0.35);
        config.stream_storage_distribution.insert(1, 0.48);
        config.stream_storage_distribution.insert(1, 0.17);

        config.web_server_addr = "0.0.0.0".to_string();
        config.web_server_port = 8088;
        config.websocket_port = 8089;

        // Prepare DB connection
        let connection = Connection::open(config.db_path.as_str())?;

        // Initialise Cameras table
        // Stores camera name & id pairings
        connection.execute(
            "CREATE TABLE if not exists cameras (
            CamID INTEGER PRIMARY KEY,
            Name TEXT NOT NULL
        )",
            (),
        )?;

        // Initialise Streams table
        // Stores RTSP Stream information for each camera
        connection.execute(
            "CREATE TABLE if not exists streams (
            StreamID INTEGER PRIMARY KEY,
            CamID INTEGER NOT NULL,
            URL TEXT NOT NULL,
            Type INTEGER NOT NULL
        )",
            (),
        )?;

        // Initialise Recordings table
        // Stores where each recording w/ thumbnail is on disk
        connection.execute(
            "CREATE TABLE if not exists recordings (
            RecordingID INTEGER PRIMARY KEY,
            StreamID INTEGER NOT NULL,
            FilePath TEXT,
            StartTime INTEGER NOT NULL,
            Duration INTEGER,
            Status INTEGER NOT NULL,
            FileSize INTEGER
        )",
            (),
        )?;

        // TODO: Add indexes

        //TODO: Handle interrupted recordings (set status code from in progress to interrupted.)

        let db = Arc::new(Database::new(connection));

        println!("Database ready");

        // dbg!(db.locate_old_recrodings(1, 1000));
        // dbg!(db.set_recording_status(1, RecordingStatusCode::ThumbnailOnly));
        // dbg!(db.set_multiple_recording_status(vec![2,3,4,5], RecordingStatusCode::ThumbnailOnly));

        // Load cameras from the database
        let mut cameras = db.list_cameras();
        println!("Loaded {} cameras", cameras.len());
        // Turn into map
        let mut camera_map = HashMap::new();

        // Add the streams to each camera
        // This has a performance hit as it technically could be done in a single DB query
        // but this i feel is more versatile and its a one time performance hit.
        for mut camera in cameras {
            if db.load_camera_streams(&mut camera).is_err() {
                println!("An error occured loading streams");
            }
            println!(
                "Loaded {} streams for camera {}",
                camera.streams.len(),
                camera.name
            );
            camera_map.insert(camera.id, camera);
        }

        Ok(Application {
            db: db,
            config: Arc::new(RwLock::new(config)),
            cameras: Arc::new(RwLock::new(camera_map)),
            workers: Arc::new(Mutex::new(ThreadManager::new())),
        })
    }

    /// Generate a filename to save the recording as
    // Returns
    //  Thumbnail Path:     String
    //  Video Path:         String
    //  Recording ID        i64
    pub fn allocate_recording_filename(&self, stream_id: u32) -> Option<(String, String, i64)> {
        // File naming scheme
        // media_dir/video/{Camera_ID}/{Stream_ID}/{Camera_Name}_{DD-MM-YYYY_HH-MM-SS_TZ}.mp4
        // media_dir/thumbnails/{Camera_ID}/{Recording_ID}.jpg

        // Format constants
        let config = if let Ok(v) = self.config.read() {
            v
        } else {
            return None;
        };

        let time = Local::now();
        let formatted_time = time.format(&config.timestamp_format).to_string();
        let video_file_extension = config.video_output_extension.as_str();
        let thumbnail_file_extension = config.thumbnail_output_extension.as_str();

        // Find camera ID and Name given the stream ID
        let camera = if let Some(v) = self.db.get_camera_from_stream_id(stream_id) {
            v
        } else {
            return None;
        };

        // Generate video filepath
        let media_dir = config.media_path.as_str();
        let camera_id = camera.id;
        let camera_name = camera.name; // TODO: Ensure contains valid characters for a filename

        let video_folder = format!("{media_dir}/video/{camera_id}/{stream_id}");
        let video_path =
            format!("{video_folder}/{camera_name}_{formatted_time}.{video_file_extension}");

        // Create recordings table entry
        let recording_id = if let Some(v) = self.db.add_recording_entry(
            stream_id,
            &video_path,
            time.timestamp_millis(),
            RecordingStatusCode::RecordingStarted,
        ) {
            v
        } else {
            return None;
        };

        // Generate thumbnail path
        let thumbnail_folder = format!("{media_dir}/thumbnails/{camera_id}");
        let thumbnail_path =
            format!("{thumbnail_folder}/{recording_id}.{thumbnail_file_extension}");

        // Database entires created, ensure the folder structure exists
        if fs::create_dir_all(&video_folder).is_err()
            || fs::create_dir_all(&thumbnail_folder).is_err()
        {
            eprintln!("Failed to create folders");
            //TODO: undo the database transaction -> Delete recording entry
            return None;
        }

        Some((video_path, thumbnail_path, recording_id))
    }

    // Applies a status code and duration to a completed recording given its id
    pub fn mark_recording_complete(
        &self,
        recording_id: i64,
        duration: Option<u64>,
        size: Option<u64>,
        status_code: RecordingStatusCode,
    ) -> Result<(), ()> {
        // Cast to signed int for sqlite (i dont like this)
        let size = size.map(|f| f as i64);
        let duration = duration.map(|f| f as i64);

        return self
            .db
            .update_recording(recording_id, duration, size, status_code);
    }

    // A function that spawns all the required workers to start the application
    // Spawns:
    //  -> Stream Capture worker thread per stream
    //  -> Storage worker thread per stream
    //  -> File managmenet thread once
    //  -> WebUI thread once (may spawn child threads)
    pub fn start_services(&self) {
        println!("Starting Seccy Services...");
        let mut manager = if let Ok(v) = self.workers.lock() {
            v
        } else {
            return;
        };

        // Spawn stream & storage workers
        let cameras = if let Ok(v) = self.cameras.read() {
            v
        } else {
            return;
        };
        for (camera_id, camera) in cameras.iter() {
            println!("Spawning workers for camera '{}'", camera.name);
            for (stream_id, stream) in camera.streams.iter() {
                println!("\t-> Starting Stream Acquisition for stream {}", stream_id);
                manager.spawn_stream_worker(self.to_owned(), stream.clone());
                println!("\t-> Starting Storage Worker for stream {}", stream_id);
                manager.spawn_storage_worker(self.to_owned(), stream.clone());
            }
        }

        // Start file management worker
        manager.spawn_management_worker(self.to_owned());

        // Start websocket worker
        manager.spawn_web_workers(self.to_owned());
    }

    // Find oldest recordings of a certain type that add up to at least a specified cumulative file size
    // stream_type describes whether main stream (0) /sub stream (1)... is being cleaned up
    // If something went wrong, an error is returned.
    // Otherwise returns a list of files
    // This function is meant to be used in finding what files to delete.
    pub fn locate_old_recordings(
        &self,
        total_allocated_size: u64,
        size_to_delete: u64,
    ) -> Result<Vec<Recording>, ()> {
        // To do this:
        // -> Query DB to find usage based on stream type
        // -> Find which stream(s) is exceeding quota
        // -> Query DB to find which files from that stream(s) to delete

        // If no stream is exceeding quota or the amount exceeded is less than the amount to delete
        // Start deleting from all of them.
        println!(
            "Looking for old recordings.\n\tAllocated Space: {}\tTo Delete: {}",
            format_bytes(total_allocated_size),
            format_bytes(size_to_delete)
        );

        // Find the minimum size to retrieve
        let config_lock = if let Ok(v) = self.config.read() {
            v
        } else {
            return Err(());
        };

        // TODO: UTILISE THIS!!!!!!
        let minimum_purge_size = config_lock.minimum_purge_size as f64;

        // Cloned to not hold up the lock.
        let storage_distribution = config_lock.stream_storage_distribution.clone();
        // let stream_storage_fraction =
        //     if let Some(v) = config_lock.stream_storage_distribution.get(&stream_type) {
        //         v.clone()
        //     } else {
        //         // The user has not defined how much storage this type of stream should get
        //         // Maybe we can handle it in some default way? This should be defined in init though.
        //         return Err(());
        //     };
        drop(config_lock);

        // Find stream size distribution
        let usage_distribution = self.db.get_usage_by_stream_type();

        // Keep track of how much from each stream needs deletion
        let mut stream_overallocation = HashMap::new(); // (Stream Type, Overallocation)
        let mut discovered_size_to_delete = 0; // How much has actually been found to delete by querying database
        // This is needed as bugs/corruption could lead to files on disk that do not have an accurately reported size in the database.

        // Find which stream types need to be deleted
        for (stream_type, allocated_fraction) in storage_distribution.iter() {
            let usage_allowed = (allocated_fraction * (total_allocated_size as f64)).floor() as u64;
            let actual_usage = usage_distribution.get(&stream_type);
            // If no entry appears in database (strangely), no action needed.
            let actual_usage = actual_usage.unwrap_or(&0u64).clone();

            println!(
                "Stream {} has been allocated {} but uses {}",
                stream_type,
                format_bytes(usage_allowed),
                format_bytes(actual_usage)
            );

            if actual_usage > usage_allowed {
                // Over quota
                let over_size = actual_usage - usage_allowed;
                stream_overallocation.insert(stream_type, over_size);
                discovered_size_to_delete += over_size;
            }
        }
        // note: If a stream type is deleted but not deleted in database records, it wont be removed.

        // We now have a list of how much to delete from each stream
        // Ensure that the discovered overcapacity matches the actual overcapacity:
        // That is, how much the database says is over quota per stream type vs what the filesystem reports.

        // FS report                            // DB query based
        if (size_to_delete as f64 * 1.05f64) > discovered_size_to_delete as f64 {
            // If the error is significant enough (the filesystem may slightly over-report due to metadata etc.)
            // Significant Enough is 5% - arbitrary.
            println!(
                "Database storage estimate does not match filesystem\n\tDatabase: {}\tFile System: {}",
                format_bytes(discovered_size_to_delete),
                format_bytes(size_to_delete)
            );

            // Add the delta to each stream
            let delta = (size_to_delete - discovered_size_to_delete) as f64;
            for (stream_type, &fraction) in storage_distribution.iter() {
                // Find how much to allocate to this stream to delete
                let to_delete = (delta * fraction).ceil() as u64;

                // increment table
                *stream_overallocation.entry(stream_type).or_default() += to_delete;
            }
        }

        // Find all the files to delete
        let mut records = Vec::new();

        for (&stream_type, &overallocation) in stream_overallocation.iter() {
            println!(
                "Searching for {} of files from stream {}",
                format_bytes(overallocation),
                stream_type
            );

            let res = if let Ok(v) = self.db.locate_old_recrodings(*stream_type, overallocation) {
                v
            } else {
                eprintln!(
                    "Something went wrong searching for old recordings for stream type {}",
                    stream_type
                );
                continue;
            };

            records.extend(res);
        }

        Ok(records)
    }

    // Wrapper fn
    // Sets the status of multiple recordings
    pub fn set_recording_status(
        &self,
        recording_id: i64,
        status_code: RecordingStatusCode,
    ) -> Result<(), ()> {
        self.db.set_recording_status(recording_id, status_code)
    }

    // Wrapper fn
    // Sets the status of multiple recordings
    pub fn set_multiple_recording_status(
        &self,
        recording_ids: Vec<i64>,
        status_code: RecordingStatusCode,
    ) -> Result<(), ()> {
        self.db
            .set_multiple_recording_status(recording_ids, status_code)
    }

    // Deletes a provided recording, updating the database to reflect the changes
    // (Note, ID is enough technically but since the function that feeds this also has a path, it saves an extra query)
    pub fn delete_recording(&self, recording_id: i64, path: &str) -> Result<(), ()> {
        // Delete the file

        // Update the database
        if self.db.mark_recording_removed(recording_id).is_err() {
            // This is bad, may end up in an infinite loop deleting
            println!(
                "Failed to update recording as removed in database but recording has been deleted from filesystem."
            );
            return Err(());
        }

        println!("Deleting recording {}", path);
        if delete_file(path).is_err() {
            // Maybe update the database?
            println!("Failed to delete file");
        }

        Ok(())
    }

    // Given a camera ID and the stream type, returns a reference to the stream handle
    pub fn get_stream_by_cam_and_type(
        &self,
        camera_id: u32,
        stream_type: u8,
    ) -> Option<Arc<RwLock<Stream>>> {
        // Access list of cameras
        let cameras_lock = if let Ok(v) = self.cameras.read() {
            v
        } else {
            return None;
        };

        // Get the camera
        let camera = cameras_lock.get(&camera_id)?;

        // Find the stream type
        let stream = camera.streams.get(&stream_type).map(|e| e.clone());

        drop(cameras_lock);

        stream
    }
}

impl Database {
    // Initialiser
    pub fn new(connection: Connection) -> Self {
        Database {
            connection: Mutex::new(connection),
        }
    }

    // == Cameras table ==

    // Creates and registers a Camera into the database
    pub fn new_camera(&self, name: String) -> Option<Camera> {
        // Await a lock on the database
        let connection = if let Ok(v) = self.connection.lock() {
            v
        } else {
            return None;
        };

        // Attempt to add the camera
        let res: Result<usize, Error> =
            connection.execute("INSERT INTO cameras (Name) VALUES (?)", [name.as_str()]);

        if res.is_err() {
            return None;
        }

        // Insert succeeded, return camera object
        // Perhaps we should do something here to limit this to a reasonable value? Like u8?
        let inserted_id = connection.last_insert_rowid() as u32;

        Some(Camera::new(inserted_id, name))
    }

    // Provided StreamID, returns CameraID and Camera Name
    // This is filled as a skeleton camera object (stream entries aren't populated)
    pub fn get_camera_from_stream_id(&self, stream_id: u32) -> Option<Camera> {
        // Acquire database lock
        let connection = if let Ok(v) = self.connection.lock() {
            v
        } else {
            return None;
        };

        // Prepare query
        let mut query = if let Ok(v) = connection.prepare(
            "SELECT s.CamID, c.Name FROM streams s
            JOIN cameras c
            WHERE s.CamID = c.CamID
            AND s.StreamID = ?
            ",
        ) {
            v
        } else {
            return None;
        };

        let parameters = [stream_id];

        // We dont check but there really only should be one row given the uniqueness of stream ID.
        let res = query.query_row(parameters, |row| Ok(Camera::new(row.get(0)?, row.get(1)?)));

        let res = if let Ok(v) = res {
            v
        } else {
            return None;
        };

        Some(res)
    }

    // Returns all the cameras in the database without streams field populated
    pub fn list_cameras(&self) -> Vec<Camera> {
        let mut results = Vec::new();

        // Await a lock on the database
        let connection = if let Ok(v) = self.connection.lock() {
            v
        } else {
            return results;
        };

        // Load all the cameras
        let mut query: rusqlite::Statement<'_> =
            if let Ok(v) = connection.prepare("SELECT c.CamID, c.Name from cameras c") {
                v
            } else {
                return results;
            };

        // Map into structs
        let res = if let Ok(v) = query.query_map((), |row| {
            // 0 -> CamID
            // 1 -> Name
            let cam_id = row.get(0)?;
            let name = row.get(1)?;

            Ok(Camera::new(cam_id, name))
        }) {
            v
        } else {
            return results;
        };

        results = res.filter_map(|e| e.ok()).collect();

        results
    }

    // == Stream table ==
    // Populates the camera stream object using saved database records
    pub fn load_camera_streams(&self, camera: &mut Camera) -> Result<(), ()> {
        // Await a lock on the database
        let connection = if let Ok(v) = self.connection.lock() {
            v
        } else {
            return Err(());
        };

        // Load all the streams for the camera
        let mut query = if let Ok(v) = connection.prepare(
            "SELECT s.StreamID, s.URL, s.\"Type\" from streams s 
                WHERE s.CamID = ?",
        ) {
            v
        } else {
            return Err(());
        };

        // Convert response into structs
        let parameters = params![camera.id];
        let res = if let Ok(v) = query.query_map(parameters, |row| {
            // 0 -> StreamID
            // 1 -> Stream URL
            // 2 -> Stream Type
            let stream_id = row.get(0)?;
            let url = row.get(1)?;
            let stream_type: u8 = row.get(2)?;

            Ok((stream_type, Stream::new(stream_id, url, stream_type)))
        }) {
            v
        } else {
            return Err(());
        };

        // Add to the camera object
        for stream in res {
            let stream = if let Ok(v) = stream {
                v
            } else {
                continue;
            };
            camera
                .streams
                .insert(stream.0, Arc::new(RwLock::new(stream.1)));
        }

        Ok(())
    }

    pub fn new_stream(&self, cam_id: u32, url: String, stream_type: u8) -> Option<Stream> {
        // Await a lock on the database
        let connection = if let Ok(v) = self.connection.lock() {
            v
        } else {
            return None;
        };

        // Attempt to add the stream
        let res: Result<usize, Error> = connection.execute(
            "INSERT INTO streams (CamID, URL, \"Type\") VALUES (?,?,?)",
            params![cam_id, url, stream_type],
        );

        if res.is_err() {
            return None;
        }

        // Insert succeeded, return stream object
        let inserted_id = connection.last_insert_rowid() as u32;

        Some(Stream::new(inserted_id, url, stream_type))
    }
    // == Recordings ==

    // Attempts to add a new entry for a recording
    // Returns recording ID if successful
    // Duration is initialised to NULL
    // All times are in milliseconds from Jan 1 1970 (64b unix epoch)
    pub fn add_recording_entry(
        &self,
        stream_id: u32,
        file_path: &str,
        start_time: i64,
        status: RecordingStatusCode,
    ) -> Option<i64> {
        // Await a lock on the database
        let connection = if let Ok(v) = self.connection.lock() {
            v
        } else {
            return None;
        };

        // Attempt to add the recording
        let res: Result<usize, Error> = connection.execute(
            "INSERT INTO recordings (StreamID, FilePath, StartTime, Status) VALUES (?,?,?,?)",
            params![stream_id, file_path, start_time, status as i64],
        );

        if res.is_err() {
            return None;
        }

        // Insert succeeded, return recording id
        let inserted_id = connection.last_insert_rowid();

        Some(inserted_id)
    }

    // Sets the status of a single recording
    pub fn set_recording_status(
        &self,
        recording_id: i64,
        status_code: RecordingStatusCode,
    ) -> Result<(), ()> {
        self.update_recording(recording_id, None, None, status_code)
    }

    // Sets the status of multiple recordings
    pub fn set_multiple_recording_status(
        &self,
        recording_ids: Vec<i64>,
        status_code: RecordingStatusCode,
    ) -> Result<(), ()> {
        // Await lock of database
        let connection = if let Ok(v) = self.connection.lock() {
            v
        } else {
            return Err(());
        };

        let param_slots = std::iter::repeat("?")
            .take(recording_ids.len())
            .collect::<Vec<_>>()
            .join(",");

        let mut params = recording_ids;
        params.insert(0, status_code as i64);

        let query = format!(
            "UPDATE recordings
            SET Status = ?
            WHERE RecordingID in ({});",
            param_slots
        );

        let res = connection.execute(&query, params_from_iter(params.iter()));

        if res.is_err() {
            println!("{:?}", res);
            return Err(());
        }

        Ok(())
    }

    // Applies a status code, duration and size to a recording.
    pub fn update_recording(
        &self,
        recording_id: i64,
        duration: Option<i64>,
        size: Option<i64>,
        status_code: RecordingStatusCode,
    ) -> Result<(), ()> {
        // Await lock of database
        let connection = if let Ok(v) = self.connection.lock() {
            v
        } else {
            return Err(());
        };

        println!(
            "Setting Status: {}, Duration: {:?}, Size: {:?} for ID: {}",
            (status_code.clone()) as i64,
            duration,
            size,
            recording_id
        );
        let res = connection.execute(
            "UPDATE recordings 
                SET Status = ?, Duration = ?, FileSize = ? 
                WHERE RecordingID = ?;",
            params![status_code as i64, duration, size, recording_id],
        );

        if res.is_err() {
            println!("{:?}", res);
            return Err(());
        }

        Ok(())
    }

    // Marks a recording as removed from the database:
    // That is, no path and an updated status.
    // We can keep duration and size for statistics later perhaps?
    pub fn mark_recording_removed(&self, recording_id: i64) -> Result<(), ()> {
        // Await lock of database
        let connection = if let Ok(v) = self.connection.lock() {
            v
        } else {
            return Err(());
        };

        let res = connection.execute(
            "UPDATE recordings 
                SET Status = ?, FilePath = NULL 
                WHERE RecordingID = ?;",
            params![RecordingStatusCode::ThumbnailOnly as i64, recording_id],
        );

        if res.is_err() {
            println!("{:?}", res);
            return Err(());
        }

        Ok(())
    }

    // Find oldest recordings of a certain type that add up to at most a specified cumulative file size
    // stream_type describes whether main stream (0) /sub stream (1)... is being cleaned up
    // Size is the total size of recordings that should be located (e.g. size of 1GB may return 10 of the oldest 100MB recordings)
    // If something went wrong, an error is returned.
    // Otherwise returns a list of files
    pub fn locate_old_recrodings(&self, stream_type: u8, size: u64) -> Result<Vec<Recording>, ()> {
        // Await lock of database
        let connection = if let Ok(v) = self.connection.lock() {
            v
        } else {
            return Err(());
        };

        // Prepare query
        // This one finds all streams of a certain type, finds recordings under those streams
        // sorts by oldest first, then adds up their file sizes until the threshold is met.
        // TODO: Maybe consider making some status codes immune ?
        //  Like recording in progress or deletion in progress

        /*
        => This simpler query found results up to the specified file size

        let mut qry = if let Ok(v) = connection.prepare(
            "WITH cte AS (
                SELECT r.RecordingID, r.FilePath, r.FileSize, SUM([FileSize]) OVER (ORDER BY r.StartTime ASC) AS TotalSize FROM cameras c
                JOIN streams s ON s.CamID = c.CamID 
                JOIN recordings r ON r.StreamID = s.StreamID 
                WHERE s.\"Type\" = ?
                ORDER BY r.StartTime ASC
            ) SELECT * FROM cte
            WHERE TotalSize < ?"
        ) {v} else {return Err(());};
        */

        /*
         Newer, more complex query, finds up to and including the row that exceeds that size.
        (It does rely on RecordingID always increasing with time, perhaps using row number may be better?)

        Why? Its a little bit more logically sound. If you consider large recordings and small quotas, it may happen that
        old files never get deleted due to being slightly larger than what is set to be purged.
        */

        // Status code here is only [10,30) which is when a file has been written or marked for removal.
        // This relies on the management only being run once at any given time as otherwise there may be overlap in "MarkedForRemoval"
        let mut qry = if let Ok(v) = connection.prepare(
            "WITH cte AS (
                SELECT 
                    r.RecordingID, 
                    r.FilePath, 
                    r.FileSize, 
                    SUM(COALESCE(r.FileSize, 0)) OVER (ORDER BY r.StartTime ASC) AS TotalSize 
                FROM cameras c
                JOIN streams s ON s.CamID = c.CamID 
                JOIN recordings r ON r.StreamID = s.StreamID 
                WHERE s.\"Type\" = ?
                AND r.Status BETWEEN 10 AND 29
                ORDER BY r.StartTime ASC
            ) 
            SELECT
                *
            FROM
                cte
            WHERE
                (cte.TotalSize - COALESCE(cte.FileSize, 0)) < ?;
            ",
        ) {
            v
        } else {
            return Err(());
        };

        let params = params![stream_type as i64, size as i64];

        let iter = if let Ok(v) = qry.query_map(params, |row| {
            let recording_id: i64 = row.get(0)?;
            let file_path: Option<String> = row.get(1)?;
            let size: Option<i64> = row.get(2)?;

            // Typecast to unsigned for clarity. Size should be positive or None.
            let size = size.map(|f| f as u64);

            Ok(Recording {
                recording_id: recording_id,
                path: file_path,
                file_size: size,
            })
        }) {
            v
        } else {
            return Err(());
        };

        Ok(iter.filter_map(|f| f.ok()).collect())
    }

    // Returns a map with each stream type and how much disk space it occupies
    //NOTE: only considers status codes between 10 and 29 inclusive
    pub fn get_usage_by_stream_type(&self) -> HashMap<u8, u64> {
        let mut map = HashMap::new();

        // Await lock of database
        let connection = if let Ok(v) = self.connection.lock() {
            v
        } else {
            return map;
        };
        let mut qry = if let Ok(v) = connection.prepare(
            "SELECT s.\"Type\", SUM(FileSize) as \"Size\"  FROM recordings r
            JOIN streams s 
            ON s.StreamID = r.StreamID 
            WHERE r.Status BETWEEN 10 AND 29
            GROUP BY s.\"Type\" ",
        ) {
            v
        } else {
            return map;
        };

        let iter = if let Ok(v) = qry.query_map((), |row| {
            let stream_type: i64 = row.get(0)?;
            let size: Option<i64> = row.get(1)?;

            // Typecast to unsigned for clarity. Size should be positive or None.
            let size = (size.unwrap_or(0i64)) as u64;

            Ok((stream_type as u8, size))
        }) {
            v
        } else {
            return map;
        };

        // Return map
        // Probably better way to do this but im trying to get something done
        for v in iter {
            match v {
                Ok((k, v)) => {
                    map.insert(k, v);
                }
                Err(_) => {}
            }
        }

        map
    }
}

/*

sq

for cam in iter {
    let c = cam.unwrap();
    println!("Camera:\t ID: {}\tName: {}", c.id, c.name);
}
 */
