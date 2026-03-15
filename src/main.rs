// mod camera;
mod storage;
mod stream;
mod util;
mod web;

extern crate ffmpeg_next as ffmpeg;

use std::{thread, time::Duration};

use crate::{
    storage::{
        capture::store_stream,
        db::Application,
        storage::{get_file_size, get_folder_size},
    },
    stream::stream::capture_stream,
    util::counters::PerformanceCounter,
};

fn main() {
    ffmpeg::init().expect("Failed to initialise FFMPEG");
    let app = Application::new().unwrap();
    // let cameras = app.cameras.read().unwrap();
    // let c1: &stream::camera::Camera = cameras.get(&1).unwrap();
    // let s1 = (&c1.streams).get(&0).unwrap().clone();
    // let s2 = (&c1.streams).get(&1).unwrap().clone();

    app.start_services();
    loop {
        
    }
    // thread::scope(|s: &thread::Scope<'_, '_>| {
    //     s.spawn(|| {
    //         let Ok(mut stream_lock) = s1.read() else {
    //             return;
    //         };
    //         let m1 = stream_lock.metrics.clone();
    //         drop(stream_lock);

    //         let Ok(mut stream_lock) = s2.read() else {
    //             return;
    //         };
    //         let m2 = stream_lock.metrics.clone();
    //         drop(stream_lock);

    //         loop {
    //             let Ok(mut metric_lock) = m1.write() else {
    //                 continue;
    //             };
    //             let r1 = metric_lock.bandwidth.read();
    //             drop(metric_lock);

    //             let Ok(mut metric_lock) = m2.write() else {
    //                 continue;
    //             };
    //             let r2 = metric_lock.bandwidth.read();
    //             drop(metric_lock);

    //             println!("Stream Rate: {} kbps", (r1 + r2) * 8f32 / 1000f32);
    //             println!("\t S1: {} kbps", (r1) * 8f32 / 1000f32);
    //             println!("\t S2: {} kbps", (r2) * 8f32 / 1000f32);
    //             thread::sleep(Duration::from_secs(5));
    //         }
    //     });
    // });

    // test(app);
}
