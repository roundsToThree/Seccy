use std::{fs, path::Path};

use crate::storage::db::{Application, RecordingStatusCode};
use num_traits::cast::AsPrimitive;

// Returns the size (in bytes) of a file given path
// None returned if file does not exist/other error.
pub fn get_file_size(path: &str) -> Option<u64> {
    // Open file metadata
    let metadata = if let Ok(v) = fs::metadata(path) {
        v
    } else {
        return None;
    };

    Some(metadata.len())
}

// Returns the size (in bytes) of a folder given path
// None returned if folder does not exist/other error.
pub fn get_folder_size(path: &str) -> Option<u64> {
    match dir_size::get_size_in_bytes(Path::new(path)) {
        Ok(v) => Some(v),
        Err(_) => None,
    }
}

// Returns the available space (in bytes) of a folder given path
// None returned if folder does not exist/other error.
pub fn get_available_space(path: &str) -> Option<u64> {
    match fs2::available_space(Path::new(path)) {
        Ok(v) => Some(v),
        Err(_) => None,
    }
}

// Deletes a provided file
pub fn delete_file(path: &str) -> Result<(), ()> {
    fs::remove_file(path).map_err(|e| {
        println!("Failed to delete file {}, {:?}", path, e);
        return ();
    })
}

// Formats bytes as a string (like KB/MB)
// todo: consider maybe using MiB or KiB?
pub fn format_bytes<T>(size: T) -> String
where
    T: num_traits::PrimInt,
{
    // let negative = false;
    let size = if let Some(v) = size.to_i64() {
        v
    } else {
        return "?".to_string();
    };

    let suffix = ["", "k", "M", "G", "T", "P", "E"];

    let scale = (size.abs() as f64).log10(); // How many zeros
    let index = (scale.floor() as u32) / 3; // What index in the suffix

    let mut value = (10 as f64).powf(scale - (3 * index) as f64);

    if size < 0 {
        value *= -1f64;
    }

    let suffix = if let Some(v) = suffix.get(index as usize) {
        v
    } else {
        "?"
    };
    format!("{:.2}{}B", value, suffix)
}

// File storage logic might need doube checking
pub fn run_management_interval(application: &Application) {
    let config_lock = if let Ok(v) = application.config.read() {
        v
    } else {
        return;
    };
    // Check file usage in the media dir
    let disk_usage = if let Some(v) = get_folder_size(&config_lock.media_path) {
        v
    } else {
        // If we cant get disk_usage, theres not much we can do as we don't know if management needs to run
        eprintln!(
            "Failed to read disk usage, mangement inhibited.\nNo old recordings will be deleted."
        );
        return;
    };

    // Check how much free space remains (Both against system & set limit)
    let real_space_remaining = get_available_space(&config_lock.media_path);
    let real_space_remaining = if let Some(v) = real_space_remaining {
        v
    } else {
        // No concept of storage limit can be established
        eprintln!(
            "Failed to read space remaining, mangement inhibited.\nNo old recordings will be deleted."
        );
        return;
    };

    let storage_limit = config_lock.limit_disk_usage;
    let minimum_buffer_space = config_lock.minimum_buffer_space;

    // How much space is actually free (including storage limit)
    // This can be negative! As you can record beyond the limit (even if momentarily)
    let mut effective_free_space = real_space_remaining as i64;

    // How much space has been allocated for storage (either limit or free space + used)
    let mut total_allocated_space = 0;

    // First just double check for misconfiguration or accidental overrun
    // (Storage limit is too small)
    if let Some(limit) = storage_limit {
        if limit < disk_usage {
            eprintln!(
                "Recorded content exceeds disk allocation of {}. Recorded {}.\nConsider increasing minimum purge size, buffer size or frequency of management interval",
                format_bytes(limit),
                format_bytes(disk_usage)
            );
        }
        if limit > (real_space_remaining + disk_usage) {
            eprintln!(
                "Storage limit larger than available allocated disk space, defaulting to using available free space as a limit.\n\tLimit {}\n\tTotal allocated {} \t[{} Available on disk + {} Used in recordings]",
                format_bytes(limit),
                format_bytes(real_space_remaining + disk_usage),
                format_bytes(real_space_remaining),
                format_bytes(disk_usage)
            );
        } else {
            // Limit defined & effective
            // Note, these casts technically can go wrong, but i dont think anyone's gonna have yottabtytes or what not.
            effective_free_space = (limit as i64) - (disk_usage as i64);
            total_allocated_space = limit;
        }
    } else {
        // No limit has been defined, take the remaining space as the limit
        total_allocated_space = real_space_remaining + disk_usage;
        if total_allocated_space > minimum_buffer_space {
            total_allocated_space -= minimum_buffer_space;
        }else {
            println!("Minimum buffer space is misconfigured - larger than available space");
        }
    }

    // Check if the service needs to run
    let min_buffer_size = config_lock.minimum_buffer_space;
    let min_purge_size = config_lock.minimum_purge_size;

    if effective_free_space > min_buffer_size as i64 {
        // Cleanup not required
        return;
    }
    drop(config_lock);

    println!(
        "Cleaning up old recordings:\n{} free\t{} required to be free",
        format_bytes(effective_free_space),
        format_bytes(min_buffer_size)
    );

    // Calculate how much to delete
    let delete_size = min_buffer_size as i64 - effective_free_space;
    // If delete size is less than 0 for whatever reason, contrain it
    let delete_size = min_purge_size.max(delete_size.max(0) as u64);
    println!("Deleting as least {}", format_bytes(delete_size));

    // Find files needing deletion
    let recordings =
        if let Ok(v) = application.locate_old_recordings(total_allocated_space, delete_size) {
            v
        } else {
            println!("Failed to find recordings to delete, no files have been removed.");
            return;
        };
    let size: u64 = recordings.iter().map(|e| e.file_size.unwrap_or(0)).sum();
    println!(
        "Found {} entries totalling {} (Reported by database)",
        recordings.len(),
        format_bytes(size)
    );

    // Mark as to be deleted in database
    let recording_ids: Vec<i64> = recordings.iter().map(|e| e.recording_id).collect();
    if application
        .set_multiple_recording_status(recording_ids, RecordingStatusCode::MarkedForRemoval)
        .is_err()
    {
        eprintln!("Failed to mark recordings for removal in database");
        // This is bad but maybe theres not enough space? by continuing, we may free up space.
    }

    // Delete files
    for recording in recordings {
        let path = if let Some(v) = recording.path {
            v
        } else {
            println!(
                "Skipping ID {} as no path provided.",
                recording.recording_id
            );
            continue;
        };
        if application
            .delete_recording(recording.recording_id, &path)
            .is_err()
        {
            eprintln!(
                "Failed to delete recording {} at {}",
                recording.recording_id, path
            );
        }
    }
}
