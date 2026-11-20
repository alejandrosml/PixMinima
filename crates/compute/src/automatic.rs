//! Per-process crossover calibration on actual RGBA8 images. Comparisons include
//! upload, dispatch, readback and synchronization. A busy GPU leaves work to the CPU.
use super::{cpu, metal};
use std::sync::Mutex;
use std::time::Instant;

const MIN_BYTES: usize = 2 << 20;
const BUCKETS: usize = 7 * 3;

#[derive(Clone, Copy, Default)]
struct Profile {
    visits: u32,
    samples: u8,
    wins: u8,
    testing: bool,
    enabled: bool,
}
static HISTORY: Mutex<[Profile; BUCKETS]> = Mutex::new(
    [Profile {
        visits: 0,
        samples: 0,
        wins: 0,
        testing: false,
        enabled: false,
    }; BUCKETS],
);

fn bucket(bytes: usize, w: usize, h: usize) -> Option<usize> {
    if bytes < MIN_BYTES || w < 64 || h < 64 {
        return None;
    }
    let size = (bytes / MIN_BYTES).ilog2().min(6) as usize;
    let shape = if w > h.saturating_mul(3) {
        0
    } else if h > w.saturating_mul(3) {
        2
    } else {
        1
    };
    Some(3 * size + shape)
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum Route {
    Cpu,
    Metal,
    Measure,
}

impl Profile {
    fn route(&mut self) -> Route {
        self.visits = self.visits.saturating_add(1);
        if self.testing || self.visits < 3 {
            return Route::Cpu;
        }
        if self.samples < 3 || self.visits.is_multiple_of(96) {
            self.testing = true;
            if self.samples == 3 {
                self.samples = 0;
                self.wins = 0;
            }
            return Route::Measure;
        }
        if self.enabled {
            Route::Metal
        } else {
            Route::Cpu
        }
    }
    fn record(&mut self, faster: bool) {
        self.testing = false;
        self.samples += 1;
        self.wins += u8::from(faster);
        if self.samples == 3 {
            self.enabled = self.wins >= 2;
        }
    }
}

pub(super) fn choose(data: &[u8], w: usize, h: usize) -> Vec<u8> {
    let host = || cpu::rows(data, w);
    let Some(class) = bucket(data.len(), w, h) else {
        return host();
    };
    let route = HISTORY
        .lock()
        .map(|mut state| state[class].route())
        .unwrap_or(Route::Cpu);
    match route {
        Route::Cpu => host(),
        Route::Metal => metal::try_choose(data, w, h).unwrap_or_else(host),
        Route::Measure => {
            // Initialize the pipeline once. A single isolated photo never pays for it.
            let gpu = metal::ready()
                .then(|| {
                    let start = Instant::now();
                    metal::try_choose(data, w, h).map(|rows| (rows, start.elapsed()))
                })
                .flatten();
            let start = Instant::now();
            let cpu = host();
            let cpu_time = start.elapsed();
            let win = gpu.as_ref().is_some_and(|(rows, time)| {
                *rows == cpu && time.as_nanos() * 5 < cpu_time.as_nanos() * 4
            });
            if let Ok(mut state) = HISTORY.lock() {
                state[class].record(win);
            }
            cpu
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn repeated_large_inputs_require_two_measured_wins() {
        assert_eq!(bucket(4096, 32, 32), None);
        assert_eq!(bucket(1 << 20, 8192, 32), None);
        let mut profile = Profile::default();
        for outcome in [Route::Cpu, Route::Cpu, Route::Measure, Route::Cpu] {
            assert_eq!(profile.route(), outcome);
        }
        profile.record(true);
        assert_eq!(profile.route(), Route::Measure);
        profile.record(false);
        assert_eq!(profile.route(), Route::Measure);
        profile.record(true);
        assert_eq!(profile.route(), Route::Metal);
        profile.visits = 95;
        assert_eq!(profile.route(), Route::Measure);
        profile.record(false);
        assert_eq!(profile.route(), Route::Measure);
        profile.record(true);
        assert_eq!(profile.route(), Route::Measure);
        profile.record(false);
        assert_eq!(profile.route(), Route::Cpu);
    }
}
