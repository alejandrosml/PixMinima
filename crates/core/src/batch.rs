//! Schedule mixed-size jobs using a snapshot of available physical memory.
//! Reserve half of the available bytes, up to 12 GiB, with a 512 MiB fallback
//! when the system has no estimate. A Linux cgroup can lower the snapshot.
//! Admissions use 16 MiB units and at most eight worker threads. File headers
//! guide the estimates; memory use can change as the encoders and other apps run.
//! A job larger than the batch budget runs on its own.
use super::{Effort, MAX_INPUT_BYTES};
use std::collections::{BTreeMap, VecDeque};
use std::fs::File;
use std::io::Read;
use std::path::Path;
use std::sync::{Condvar, Mutex};

const MIB: u64 = 1 << 20;
const MAX_WORKERS: usize = 8;
const QUANTUM: u64 = 16 * MIB;
const GIB: u64 = 1024 * MIB;

fn budget_units(available: Option<u64>) -> u64 {
    // Leave at least half of the reported available RAM to the system and other
    // applications. The single-job rule still makes progress on smaller hosts.
    let bytes = available.map_or(512 * MIB, |n| (n / 2).min(12 * GIB));
    (bytes / QUANTUM).max(1)
}

fn available_memory() -> Option<u64> {
    use sysinfo::{MemoryRefreshKind, RefreshKind, System};
    let s = System::new_with_specifics(
        RefreshKind::nothing().with_memory(MemoryRefreshKind::nothing().with_ram()),
    )
    .ok()?;
    if s.total_memory() == 0 {
        return None;
    }
    let available = s.available_memory().min(s.total_memory());
    Some(
        s.cgroup_limits()
            .map_or(available, |c| available.min(c.free_memory)),
    )
}

fn jpeg_pixels(header: &[u8]) -> Option<u64> {
    if !header.starts_with(&[0xff, 0xd8]) {
        return None;
    }
    let mut pos = 2;
    while header.get(pos) == Some(&0xff) {
        while header.get(pos) == Some(&0xff) {
            pos += 1;
        }
        let marker = *header.get(pos)?;
        pos += 1;
        let segment = u16::from_be_bytes(header.get(pos..pos + 2)?.try_into().ok()?) as usize;
        if segment < 2 {
            return None;
        }
        if matches!(marker,0xc0..=0xc3|0xc5..=0xc7|0xc9..=0xcb|0xcd..=0xcf) {
            let h = u16::from_be_bytes(header.get(pos + 3..pos + 5)?.try_into().ok()?);
            let w = u16::from_be_bytes(header.get(pos + 5..pos + 7)?.try_into().ok()?);
            return Some(u64::from(w) * u64::from(h));
        }
        if marker == 0xda || marker == 0xd9 {
            break;
        }
        pos += segment;
    }
    None
}

// Recognize static PNG only once IDAT is reached. A long metadata chunk can
// hide acTL beyond the bounded header read, so that case receives the animation
// reservation as well. PNG requires acTL before the first image data chunk.
fn png_frames(header: &[u8]) -> Option<u64> {
    let mut i = 8usize;
    while let Some(chunk) = header.get(i..).and_then(|suffix| suffix.first_chunk::<8>()) {
        let len = u32::from_be_bytes(chunk[..4].try_into().unwrap()) as usize;
        match &chunk[4..] {
            b"IDAT" => return Some(1),
            b"acTL" if len == 8 => {
                return Some(
                    u64::from(u32::from_be_bytes(
                        header.get(i + 8..i + 12)?.try_into().ok()?,
                    ))
                    .max(2),
                );
            }
            _ => i = i.checked_add(12)?.checked_add(len)?,
        }
    }
    None
}

fn admission(header: &[u8], file_size: u64, effort: Effort, budget: u64) -> u64 {
    if file_size > MAX_INPUT_BYTES {
        return budget;
    }
    let be = |b: &[u8]| u64::from(u32::from_be_bytes(b.try_into().unwrap()));
    let base = 48 * MIB + file_size * 5; // original, candidate and verification copies
    let opaque = budget.div_ceil(2);
    let (work, minimum) = if header.starts_with(b"\x89PNG\r\n\x1a\n") && header.len() >= 29 {
        let width = be(&header[16..20]);
        let height = be(&header[20..24]);
        let depth = u64::from(header[24]);
        let channels = match header[25] {
            0 | 3 => 1,
            2 => 3,
            4 => 2,
            6 => 4,
            _ => return budget,
        };
        let pixels = width.saturating_mul(height);
        let raw = (width * channels * depth)
            .div_ceil(8)
            .saturating_add(1)
            .saturating_mul(height);
        if raw > 128 * MIB {
            return budget;
        }
        // Includes the native raster, filter trials and a canonical RGBA
        // comparison. Maximum retains more alternative filter layouts.
        let factor = match effort {
            Effort::Standard => 12,
            Effort::Thorough => 16,
            Effort::Maximum => 24,
        };
        let frames = png_frames(header);
        let animated = frames != Some(1);
        let frame_storage = if animated {
            raw.saturating_mul(frames.unwrap_or(u64::MAX))
                .min(256 * MIB)
        } else {
            0
        };
        let minimum = if raw > 64 * MIB || (animated && frame_storage >= 32 * MIB) {
            opaque
        } else {
            1
        };
        (
            raw.saturating_mul(factor)
                .saturating_add(pixels.saturating_mul(8))
                .saturating_add(frame_storage.saturating_mul(2)),
            minimum,
        )
    } else if let Some(pixels) = jpeg_pixels(header) {
        (
            pixels.saturating_mul(if effort == Effort::Maximum { 48 } else { 32 }),
            1,
        )
    } else if header.starts_with(b"qoif") && header.len() >= 12 {
        (
            be(&header[4..8])
                .saturating_mul(be(&header[8..12]))
                .saturating_mul(20),
            1,
        )
    } else if header.starts_with(b"8BPS") && header.len() >= 26 {
        let h = be(&header[14..18]);
        let w = be(&header[18..22]);
        let depth = u64::from(u16::from_be_bytes([header[22], header[23]]));
        let ch = u64::from(u16::from_be_bytes([header[12], header[13]]));
        let raw = (w * depth).div_ceil(8).saturating_mul(h).saturating_mul(ch);
        (
            raw.saturating_mul(10),
            if raw >= 64 * MIB { opaque } else { 1 },
        )
    } else if header.starts_with(b"GIF8") && header.len() >= 10 {
        let w = u64::from(u16::from_le_bytes([header[6], header[7]]));
        let h = u64::from(u16::from_le_bytes([header[8], header[9]]));
        (w * h * 24, 1)
    } else {
        // TIFF strips, EXR and layered containers can expand far beyond their
        // compressed header size. Limit concurrent opaque jobs even on a large Mac.
        (512 * MIB, opaque)
    };
    base.saturating_add(work)
        .div_ceil(QUANTUM)
        .clamp(minimum, budget)
}

fn cost(path: &Path, effort: Effort, budget: u64) -> u64 {
    let Ok(mut file) = File::open(path) else {
        return budget;
    };
    let Ok(meta) = file.metadata() else {
        return budget;
    };
    let mut buf = [0; 4096];
    let Ok(n) = file.read(&mut buf) else {
        return budget;
    };
    admission(&buf[..n], meta.len(), effort, budget)
}

struct Pending {
    bins: BTreeMap<u64, VecDeque<usize>>,
    available: u64,
}

pub struct BatchQueue {
    jobs: usize,
    pending: Mutex<Pending>,
    released: Condvar,
}

impl BatchQueue {
    /// Compatibility constructor for callers that do not supply an effort.
    pub fn new<'a>(paths: impl IntoIterator<Item = &'a Path>) -> Self {
        Self::with_effort(paths, Effort::Maximum)
    }

    /// Reserve estimated workspaces from available RAM at batch start. These
    /// estimates control concurrency; codecs enforce their own per-file bounds.
    pub fn with_effort<'a>(paths: impl IntoIterator<Item = &'a Path>, effort: Effort) -> Self {
        let available = budget_units(available_memory());
        let mut bins = BTreeMap::<u64, VecDeque<usize>>::new();
        let mut jobs = 0;
        for (i, path) in paths.into_iter().enumerate() {
            bins.entry(cost(path, effort, available))
                .or_default()
                .push_back(i);
            jobs += 1;
        }
        Self {
            jobs,
            pending: Mutex::new(Pending { bins, available }),
            released: Condvar::new(),
        }
    }

    pub fn worker_count(&self) -> usize {
        std::thread::available_parallelism()
            .map_or(1, usize::from)
            .min(MAX_WORKERS)
            .min(self.jobs)
    }

    /// Acquire the heaviest job that fits in the free workspace estimate.
    pub fn next(&self) -> Option<BatchJob<'_>> {
        let mut p = self.pending.lock().unwrap_or_else(|e| e.into_inner());
        loop {
            if p.bins.is_empty() {
                return None;
            }
            let available = p.available;
            if let Some((&units, bucket)) = p.bins.range_mut(..=available).next_back() {
                let index = bucket.pop_front().expect("queued bucket is never empty");
                if bucket.is_empty() {
                    p.bins.remove(&units);
                }
                p.available -= units;
                return Some(BatchJob {
                    queue: self,
                    index,
                    units,
                });
            }
            p = self.released.wait(p).unwrap_or_else(|e| e.into_inner());
        }
    }
}

pub struct BatchJob<'a> {
    queue: &'a BatchQueue,
    index: usize,
    units: u64,
}
impl BatchJob<'_> {
    pub fn index(&self) -> usize {
        self.index
    }
}
impl Drop for BatchJob<'_> {
    fn drop(&mut self) {
        let mut p = self.queue.pending.lock().unwrap_or_else(|e| e.into_inner());
        p.available += self.units;
        self.queue.released.notify_all();
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    fn png(width: u32, height: u32, kind: &[u8; 4]) -> Vec<u8> {
        let mut header = b"\x89PNG\r\n\x1a\n\x00\x00\x00\x0dIHDR".to_vec();
        header.extend(width.to_be_bytes());
        header.extend(height.to_be_bytes());
        header.extend([8, 6, 0, 0, 0, 0, 0, 0, 0]);
        header.extend(8u32.to_be_bytes());
        header.extend(kind);
        header.extend(32u32.to_be_bytes());
        header
    }

    #[test]
    fn high_resolution_jobs_scale_with_ram_and_effort() {
        let large = budget_units(Some(39 * GIB));
        let small = budget_units(Some(2 * GIB));
        let image = png(3840, 2160, b"IDAT");
        let fast = admission(&image, 24 * MIB, Effort::Standard, large);
        let maximum = admission(&image, 24 * MIB, Effort::Maximum, large);
        assert!(fast < maximum);
        assert!(large / maximum >= MAX_WORKERS as u64);
        assert_eq!(
            small / admission(&image, 24 * MIB, Effort::Maximum, small),
            1
        );
        assert_eq!(budget_units(None), 512 * MIB / QUANTUM);
        assert_eq!(budget_units(Some(0)), 1);
    }

    #[test]
    fn unknown_animation_and_layer_sizes_receive_conservative_reservations() {
        let budget = budget_units(Some(8 * GIB));
        for kind in [b"acTL", b"iCCP"] {
            let data = png(3840, 2160, kind);
            assert!(admission(&data, 90, Effort::Standard, budget) >= budget.div_ceil(2));
        }
        let mut psd = vec![0; 26];
        psd[..4].copy_from_slice(b"8BPS");
        psd[12..14].copy_from_slice(&4u16.to_be_bytes());
        psd[14..18].copy_from_slice(&8192u32.to_be_bytes());
        psd[18..22].copy_from_slice(&8192u32.to_be_bytes());
        psd[22..24].copy_from_slice(&16u16.to_be_bytes());
        assert_eq!(admission(&psd, 100, Effort::Standard, budget), budget);
        assert!(admission(b"II*\0", 50, Effort::Standard, budget) >= budget.div_ceil(2));
        assert_eq!(
            admission(
                &png(u32::MAX, u32::MAX, b"IDAT"),
                100,
                Effort::Maximum,
                budget
            ),
            budget
        );
    }

    #[test]
    fn admitted_jobs_fit_and_each_finishes_once() {
        use std::sync::atomic::{AtomicU64, Ordering};
        let bins = BTreeMap::from([
            (1, (0..8).collect()),
            (2, (8..11).collect()),
            (4, (11..14).collect()),
            (8, (14..16).collect()),
        ]);
        const TOTAL_UNITS: u64 = 8;
        let queue = BatchQueue {
            jobs: 16,
            pending: Mutex::new(Pending {
                bins,
                available: TOTAL_UNITS,
            }),
            released: Condvar::new(),
        };
        let running = AtomicU64::new(0);
        let done = Mutex::new(Vec::new());
        std::thread::scope(|s| {
            for _ in 0..8 {
                s.spawn(|| {
                    while let Some(j) = queue.next() {
                        assert!(
                            running.fetch_add(j.units, Ordering::SeqCst) + j.units <= TOTAL_UNITS
                        );
                        done.lock().unwrap().push(j.index());
                        std::thread::yield_now();
                        running.fetch_sub(j.units, Ordering::SeqCst);
                    }
                });
            }
        });
        let mut done = done.into_inner().unwrap();
        done.sort_unstable();
        assert_eq!(done, (0..16).collect::<Vec<_>>());
    }
}
