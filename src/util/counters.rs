use std::time::{Duration, Instant};

pub trait PerformanceCounter {
    // Add to the integrator
    fn push(&mut self, val: f32);
    // Read the rate in seconds
    fn read(&mut self) -> f32;
}

pub struct RateCounter {
    // TODO: Profile push op. Perhaps this can be done with a queue of some kind that is evaluated on read

    time_sampled: Instant,   // Time of last accumulator reset
    accumulator: f32,             // Accumulated score
    last_rate: f32,               // Last rate until window time
    window_size: f32,           // How long to average rate for (seconds)
}


impl Default for RateCounter {
    fn default() -> Self {
        RateCounter {
            time_sampled: Instant::now(),
            accumulator: 0f32,
            last_rate: 0f32,
            window_size: 1f32,
        }
    }
}


impl RateCounter {
    // Window size in seconds
    pub fn set_window(&mut self, val: f32) {
        self.window_size = val;
    }
}

impl PerformanceCounter for RateCounter {
    fn push(&mut self, val: f32) {
        self.accumulator += val;
        self.read();
    }

    fn read(&mut self) -> f32 {
        // Has the window time elapsed?
        let time_now = Instant::now();
        // Time elapsed
        let delta = time_now.duration_since(self.time_sampled).as_secs_f32();
       
        if  delta > self.window_size {
            // Window elapsed
            self.last_rate = self.accumulator/delta;
            self.time_sampled = time_now;
            self.accumulator = 0f32;
        }

        self.last_rate
    }
}