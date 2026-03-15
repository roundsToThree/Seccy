# Seccy
A lightweight Rust security camera NVR

Many cameras can capture multple streams in different resolutions, Seccy records them all and periodically deletes the oldest footage by resolution. Service can be configured in `db.rs` to allow for different stream recordng times, for example:
```rs
config.limit_disk_usage = Some(35000000000000); // 35TB ish
config.stream_storage_distribution = HashMap::new();
config.stream_storage_distribution.insert(0, 0.35); //Stream type: 0 (main stream), use 35% of the total space allocated
config.stream_storage_distribution.insert(1, 0.48); //Stream type: 0 (sub stream), use 48% of the total space allocated
config.stream_storage_distribution.insert(1, 0.17); //Stream type: 0 (sub-sub stream), use 17% of the total space allocated
```
In this particular example, a camera recording 4K, 720P & 320P for each stream type can record for months at full resolution, years at medium resolution and essentially indefinitely at low resolution.

While the program is experimental, I have tested it on multiple platforms for an extended period and it appears to work reliably.

# Setup
``src/sorage/db.rs`` to configure recording duration, file space available, split of recording space etc..\
``cargo run`` to run.\
configure cameras after first run using a database tool like dbeaver by opening the database file\
``Rocket.toml`` to configure web server

# WIP
Issues: 
- H264/H265 usage is hardcoded in the storage service, set on compile time in capture.rs
- WebRTC online playback does not currently support H265 Streams
- No GUI to add cameras, edit the sql database directly to add cameras by providing RSTP streams to the streams table & cameras to the cameras table
- Instead of OOO buffering, OOO packets are just dropped

# Criticisms/Issues
This is my first project in rust, hence i'm still learning the correct design patterns and formats for clean code. I've also mostly made it to fit the constrants of my application so it is not as easily usable. If people are interested in this project, i'd happily apply fixes or features. If you are looking for a better rust NVR, look at the Frigate project (iirc thats a rust based nvr).\
- Mixed usage of Result and Option
- Not using built in structs as much (Duration v u64, Path v String)
- Mixing unsigned and signed. Limiting variables to 8 bits.
- Pretty much everything public? This good? or should getters and setters be used?
- Maybe Macros should be eused for getting locks
- Maybe StreamHandle should implement common methods
- Not yet authenticated.

# AI Disclosure
AI was used in the making of tihs, but not everywhere. It was only used in Ffmpeg DTS rescaling & skeleton structure and WebRTC as I have not used either before and the documentation was limited, especially in ffmpeg case. Everything else I tried to write from scratch as this is mostly a learning exercise.
