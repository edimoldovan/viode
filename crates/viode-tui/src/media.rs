//! Background thumbnail/waveform generation for the graphics TUI. A worker
//! thread runs ffmpeg so the UI never blocks; results are PNGs cached under
//! cache/tui/, keyed by (source, in, out) so trims regenerate and reopens
//! reuse.

use std::collections::{HashMap, HashSet};
use std::hash::{Hash, Hasher};
use std::path::{Path, PathBuf};
use std::process::Command;
use std::sync::mpsc::{channel, Receiver, Sender};

use viode_core::Time;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum Kind {
    Thumb,
    Wave,
}

struct Job {
    key: u64,
    kind: Kind,
    src: PathBuf,
    in_s: f64,
    out_s: f64,
    dest: PathBuf,
}

pub struct MediaCache {
    dir: PathBuf,
    ready: HashMap<u64, PathBuf>,
    requested: HashSet<u64>,
    tx: Sender<Job>,
    rx: Receiver<(u64, PathBuf)>,
}

fn key_of(kind: Kind, src: &Path, in_s: f64, out_s: f64) -> u64 {
    let mut h = std::collections::hash_map::DefaultHasher::new();
    kind.hash(&mut h);
    src.hash(&mut h);
    in_s.to_bits().hash(&mut h);
    out_s.to_bits().hash(&mut h);
    h.finish()
}

impl MediaCache {
    pub fn new(project_dir: &Path) -> MediaCache {
        let (tx, job_rx) = channel::<Job>();
        let (done_tx, rx) = channel();
        std::thread::spawn(move || {
            while let Ok(job) = job_rx.recv() {
                if generate(&job).is_ok() && job.dest.exists() {
                    let _ = done_tx.send((job.key, job.dest));
                }
            }
        });
        MediaCache {
            dir: project_dir.join("cache").join("tui"),
            ready: HashMap::new(),
            requested: HashSet::new(),
            tx,
            rx,
        }
    }

    pub fn dest_for(&self, kind: Kind, src: &Path, in_s: f64, out_s: f64) -> PathBuf {
        let key = key_of(kind, src, in_s, out_s);
        let prefix = match kind {
            Kind::Thumb => "thumb",
            Kind::Wave => "wave",
        };
        self.dir.join(format!("{prefix}_{key:016x}.png"))
    }

    /// Drain finished jobs. Returns true if anything new became ready.
    pub fn pump(&mut self) -> bool {
        let mut any = false;
        while let Ok((key, path)) = self.rx.try_recv() {
            self.ready.insert(key, path);
            any = true;
        }
        any
    }

    /// The PNG for this clip artifact if it exists — otherwise queue the
    /// generation (once) and return None for now.
    pub fn get(&mut self, kind: Kind, src: &Path, in_s: f64, out_s: f64) -> Option<PathBuf> {
        let key = key_of(kind, src, in_s, out_s);
        if let Some(p) = self.ready.get(&key) {
            return Some(p.clone());
        }
        let dest = self.dest_for(kind, src, in_s, out_s);
        if dest.exists() {
            self.ready.insert(key, dest.clone());
            return Some(dest);
        }
        if self.requested.insert(key) {
            let _ = std::fs::create_dir_all(&self.dir);
            let _ = self.tx.send(Job {
                key,
                kind,
                src: src.to_path_buf(),
                in_s,
                out_s,
                dest,
            });
        }
        None
    }
}

fn generate(job: &Job) -> Result<(), ()> {
    match job.kind {
        Kind::Thumb => {
            // Frame from the middle of the clip range — most representative.
            let at = (job.in_s + job.out_s) / 2.0;
            Command::new("ffmpeg")
                .args(["-y", "-loglevel", "error", "-ss", &at.to_string(), "-i"])
                .arg(&job.src)
                .args(["-frames:v", "1", "-vf", "scale=480:-2"])
                .arg(&job.dest)
                .status()
                .map(|s| s.success().then_some(()).ok_or(()))
                .map_err(|_| ())?
        }
        Kind::Wave => viode_core::waveform_png(
            &job.src,
            Time::from_secs_f64(job.in_s).map_err(|_| ())?,
            Time::from_secs_f64(job.out_s).map_err(|_| ())?,
            &job.dest,
            480,
            64,
        )
        .map_err(|_| ()),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn ready_files_are_found_without_the_worker() {
        let dir = tempfile::tempdir().unwrap();
        let mut cache = MediaCache::new(dir.path());
        let src = Path::new("media/a.mp4");

        // Nothing yet: queued, returns None.
        assert!(cache.get(Kind::Thumb, src, 0.0, 2.0).is_none());

        // A cached PNG from a previous session is picked up directly.
        let dest = cache.dest_for(Kind::Wave, src, 0.0, 2.0);
        std::fs::create_dir_all(dest.parent().unwrap()).unwrap();
        std::fs::write(&dest, b"png").unwrap();
        assert_eq!(cache.get(Kind::Wave, src, 0.0, 2.0), Some(dest));

        // Different in/out -> different key -> not confused with the above.
        assert!(cache.get(Kind::Wave, src, 0.5, 2.0).is_none());
    }

    #[test]
    fn keys_separate_kinds_and_ranges() {
        let src = Path::new("x.mp4");
        assert_ne!(
            key_of(Kind::Thumb, src, 0.0, 1.0),
            key_of(Kind::Wave, src, 0.0, 1.0)
        );
        assert_ne!(
            key_of(Kind::Thumb, src, 0.0, 1.0),
            key_of(Kind::Thumb, src, 0.0, 2.0)
        );
    }
}
