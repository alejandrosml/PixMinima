//! A bounded, asynchronous hint for the saved Metal preference. The encoder
//! makes the final compatibility decision for each PNG or APNG stream.
use eframe::egui;
use std::io::Read;
use std::path::{Path, PathBuf};
use std::sync::mpsc::{Receiver, TryRecvError, channel};
use std::time::Duration;

#[derive(Default)]
pub struct Selection {
    inspected: Vec<PathBuf>,
    pending: Option<Receiver<bool>>,
    incompatible: bool,
}

impl Selection {
    pub fn incompatible(&mut self, paths: &[PathBuf], ctx: &egui::Context) -> bool {
        // Large selections and folders are processed by the batch, without an
        // extra walk for a preference panel. Never disable an untested case.
        if paths.is_empty() || paths.len() > 64 {
            return false;
        }
        if self.inspected != paths {
            self.inspected = paths.to_vec();
            self.incompatible = false;
            let candidates = self.inspected.clone();
            let (tx, rx) = channel();
            self.pending = std::thread::Builder::new()
                .name("pixminima-png-selection".into())
                .spawn(move || {
                    let _ = tx.send(candidates.iter().all(|p| known_incompatible(p)));
                })
                .ok()
                .map(|_| rx);
        }
        if let Some(rx) = &self.pending {
            match rx.try_recv() {
                Ok(incompatible) => {
                    self.incompatible = incompatible;
                    self.pending = None;
                }
                Err(TryRecvError::Disconnected) => self.pending = None,
                Err(TryRecvError::Empty) => ctx.request_repaint_after(Duration::from_millis(90)),
            }
        }
        self.incompatible
    }
}

fn known_incompatible(path: &Path) -> bool {
    let Ok(meta) = std::fs::symlink_metadata(path) else {
        return false;
    };
    if !meta.is_file() {
        return false;
    }
    let Ok(mut file) = std::fs::File::open(path) else {
        return false;
    };
    let mut data = [0u8; 29];
    if file.read_exact(&mut data).is_err() {
        return false;
    }
    if data[..8] != *b"\x89PNG\r\n\x1a\n" {
        return true;
    }
    // Only non-interlaced 8-bit RGBA streams enter the Metal row selector.
    data[12..16] == *b"IHDR" && (data[24] != 8 || data[25] != 6 || data[28] != 0)
}
#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn png_compatibility_uses_header_not_extension_and_keeps_directories_available() {
        let area = tempfile::tempdir().unwrap();
        assert!(!known_incompatible(area.path()));
        let path = area.path().join("actually-a-png.jpg");
        let mut header = include_bytes!("../tests/fixtures/flat.png")[..29].to_vec();
        std::fs::write(&path, &header).unwrap();
        assert!(known_incompatible(&path));
        header[25] = 6;
        std::fs::write(&path, &header).unwrap();
        assert!(!known_incompatible(&path));
        header[28] = 1;
        std::fs::write(&path, &header).unwrap();
        assert!(known_incompatible(&path));
    }
}
