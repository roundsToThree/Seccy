use std::{
    collections::HashMap,
    fs,
    sync::{Arc, Mutex, RwLock},
    thread::{self, JoinHandle, Scope},
    time::{Duration, SystemTime},
};

use crate::{storage::{db::Application, storage}, web::{webserver::start_webserver_service, websocket::start_websocket_service}};
use crate::{
    storage::capture::store_stream,
    stream::{
        camera::{Camera, Stream, StreamHandle},
        stream::capture_stream,
    },
};

pub struct ThreadManager {
    // Workers which capture a camera's livestream and broadcast it internally
    // Key is a Stream ID
    pub(crate) stream_workers: HashMap<u32, JoinHandle<()>>,

    // Workers which capture a recorded stream and store it as a file.
    // Key is a Stream ID
    pub(crate) storage_workers: HashMap<u32, JoinHandle<()>>,
    // pub(crate) stream_workers: HashMap<u32, ScopedJoinHandle<'a, ()>>,
}

impl ThreadManager {
    pub fn new() -> Self {
        ThreadManager {
            stream_workers: HashMap::new(),
            storage_workers: HashMap::new(),
        }
    }

    // Spawns a worker to capture the raw stream from the camera and broadcast it to other
    pub fn spawn_stream_worker(&mut self, application: Application, stream: StreamHandle) {
        // This error could be invisible but im going to hope its detectable upstream using error checking methods
        // like low data rate, missing files, no stream etc.
        let stream_id = if let Ok(v) = stream.read() {
            v.stream_id
        } else {
            return;
        };
        let existing_handle = self.stream_workers.remove(&stream_id);
        match existing_handle {
            Some(handle) => {
                // Stream worker already exists, maybe the thread actually closed but this record was not updated?
                // Close the thread and restart it.
                // TODO: This doesn't close the thread. Theres a chance the application hangs here
                // Maybe we should handle this better?
                let _ = handle.join();
            }
            None => {}
        }

        let handle = thread::spawn(move || {
            loop {
                // TODO: Maybe add some channel to externally terminate these threads
                // TODO: Application currently unused, might be needed in the future inside capture_stream
                let r = capture_stream(&stream);
                match r {
                    Ok(_) => {
                        eprintln!("Capture Service Ended with No Errors");
                    }
                    Err(v) => {
                        eprintln!("Capture Service Ended with Error {}", v);
                        println!("Restarting in 5 seconds");
                        thread::sleep(Duration::from_secs(5));
                    }
                }
            }
        });

        // Add handle to manager
        self.stream_workers.insert(stream_id, handle);
    }

    // Spawns a worker to record streams to disk
    // Application is copied as its not behind Arc<Mutex. All internal fields are. Maybe should be split into a handle into the future?
    pub fn spawn_storage_worker(&mut self, application: Application, stream: StreamHandle) {
        // Note: copied code from other spawn function, may be useful to refactor to reduce duplicated code?
        // This error could be invisible but im going to hope its detectable upstream using error checking methods
        // like low data rate, missing files, no stream etc.
        let stream_id = if let Ok(v) = stream.read() {
            v.stream_id
        } else {
            return;
        };
        let existing_handle = self.storage_workers.remove(&stream_id);
        match existing_handle {
            Some(handle) => {
                // Stream worker already exists, maybe the thread actually closed but this record was not updated?
                // Close the thread and restart it.
                // TODO: This doesn't close the thread. Theres a chance the application hangs here
                // Maybe we should handle this better?
                let _ = handle.join();
            }
            None => {}
        }

        let handle = thread::spawn(move || {
            loop {
                // TODO: Maybe add some channel to externally terminate these threads

                let r = store_stream(&application, &stream);
                match r {
                    Ok(_) => {
                        eprintln!("Storage Service Ended with No Errors");
                    }
                    Err(v) => {
                        eprintln!("Storage Service Ended with Error {}", v);
                        println!("Restarting in 5 seconds");
                        thread::sleep(Duration::from_secs(5));
                    }
                }
            }
        });

        // Add handle to manager
        self.storage_workers.insert(stream_id, handle);
    }

    pub fn spawn_management_worker(&mut self, application: Application) {
        // Determine management interval
        let config_lock = if let Ok(v) = application.config.read() {
            v
        } else {
            eprintln!(
                "Failed to start file management service, no old recordings will be removed."
            );
            return;
        };
        let interval = config_lock.management_interval;

        drop(config_lock);

        // Run management loop
        thread::spawn(move || {
            loop {
                println!("Checking for old recordings");
                storage::run_management_interval(&application);
                println!("Management complete");
                thread::sleep(interval);
            }
        });
    }


    pub fn spawn_web_workers(&mut self, application: Application) {

        // Spawn websocket worker
        let app = application.clone();
        thread::spawn(move || {
            loop {
                println!("Starting websocket worker");
                start_websocket_service(app.clone());
                eprintln!("Websocket worker unexpectedly stopped, restarting in 5 seconds");
                thread::sleep(Duration::from_secs(5));
            }
        });

        // Spawn web seerver
        thread::spawn(move || {
            loop {
                println!("Starting webserver worker");
                start_webserver_service(application.clone());
                eprintln!("Webserver worker unexpectedly stopped, restarting in 5 seconds");
                thread::sleep(Duration::from_secs(5));
            }
        });

    }
}
