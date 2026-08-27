//! Background thumbnail/waveform generation for the graphics TUI. A worker
//! thread runs ffmpeg so the UI never blocks; results are PNGs cached under
//! cache/tui/, keyed by (source, in, out) so trims regenerate and reopens
//! reuse.

use std::collections::HashMap;
use std::hash::{Hash, Hasher};
use std::path::{Path, PathBuf};
use std::sync::mpsc::{channel, Receiver, Sender};

use viode_core::Time;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum Kind {
    /// Filmstrip: several frames tiled horizontally.
    Strip,
    Wave,
}

/// Everything that determines an artifact's pixels — all of it is in the
/// cache key, so trims AND resizes regenerate exactly once.
#[derive(Debug, Clone, PartialEq)]
pub struct Spec {
    pub kind: Kind,
    pub src: PathBuf,
    pub in_s: f64,
    pub out_s: f64,
    pub px_w: u32,
    pub px_h: u32,
    /// Strip only: number of tiled frames.
    pub frames: u32,
}

struct Job {
    key: u64,
    spec: Spec,
    dest: PathBuf,
}

pub struct MediaCache {
    dir: PathBuf,
    ready: HashMap<u64, PathBuf>,
    requested: HashMap<u64, PathBuf>,
    tx: Sender<Job>,
    rx: Receiver<(u64, PathBuf)>,
}

fn key_of(spec: &Spec) -> u64 {
    let mut h = std::collections::hash_map::DefaultHasher::new();
    spec.kind.hash(&mut h);
    spec.src.hash(&mut h);
    spec.in_s.to_bits().hash(&mut h);
    spec.out_s.to_bits().hash(&mut h);
    spec.px_w.hash(&mut h);
    spec.px_h.hash(&mut h);
    spec.frames.hash(&mut h);
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
            requested: HashMap::new(),
            tx,
            rx,
        }
    }

    pub fn dest_for(&self, spec: &Spec) -> PathBuf {
        let key = key_of(spec);
        let prefix = match spec.kind {
            Kind::Strip => "strip",
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
    pub fn get(&mut self, spec: Spec) -> Option<PathBuf> {
        let key = key_of(&spec);
        if let Some(p) = self.ready.get(&key) {
            return Some(p.clone());
        }
        let dest = self.dest_for(&spec);
        if dest.exists() {
            self.ready.insert(key, dest.clone());
            return Some(dest);
        }
        if !self.requested.contains_key(&key) {
            self.requested.insert(key, dest.clone());
            let _ = std::fs::create_dir_all(&self.dir);
            let _ = self.tx.send(Job { key, spec, dest });
        }
        None
    }

    /// Destinations of queued-but-unfinished artifacts (also lets tests
    /// stand in for the worker).
    pub fn pending(&self) -> Vec<PathBuf> {
        self.requested
            .values()
            .filter(|d| !self.ready.values().any(|r| r == *d))
            .cloned()
            .collect()
    }
}

fn generate(job: &Job) -> Result<(), ()> {
    let s = &job.spec;
    let in_ = Time::from_secs_f64(s.in_s).map_err(|_| ())?;
    let out = Time::from_secs_f64(s.out_s).map_err(|_| ())?;
    match s.kind {
        Kind::Strip => {
            // n frames tiled 1 row high: a filmstrip at the exact pixel
            // size of the cells it will cover — no stretching.
            let n = s.frames.max(1);
            let dur = (s.out_s - s.in_s).max(0.001);
            let tile_w = (s.px_w / n).max(16);
            viode_core::contact_sheet_png(
                &s.src,
                in_,
                out,
                &job.dest,
                dur / n as f64,
                n,
                tile_w,
            )
            .map(|_| ())
            .map_err(|_| ())
        }
        Kind::Wave => {
            viode_core::waveform_png(&s.src, in_, out, &job.dest, s.px_w.max(16), s.px_h.max(16), "0x7d92a8")
                .map_err(|_| ())
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn spec(kind: Kind, in_s: f64, out_s: f64, px_w: u32) -> Spec {
        Spec {
            kind,
            src: "media/a.mp4".into(),
            in_s,
            out_s,
            px_w,
            px_h: 64,
            frames: 3,
        }
    }

    #[test]
    fn ready_files_are_found_without_the_worker() {
        let dir = tempfile::tempdir().unwrap();
        let mut cache = MediaCache::new(dir.path());

        // Nothing yet: queued, returns None.
        assert!(cache.get(spec(Kind::Strip, 0.0, 2.0, 480)).is_none());

        // A cached PNG from a previous session is picked up directly.
        let dest = cache.dest_for(&spec(Kind::Wave, 0.0, 2.0, 480));
        std::fs::create_dir_all(dest.parent().unwrap()).unwrap();
        std::fs::write(&dest, b"png").unwrap();
        assert_eq!(cache.get(spec(Kind::Wave, 0.0, 2.0, 480)), Some(dest));

        // Different in/out -> different key -> not confused with the above.
        assert!(cache.get(spec(Kind::Wave, 0.5, 2.0, 480)).is_none());
    }

    #[test]
    fn keys_separate_kinds_ranges_and_sizes() {
        assert_ne!(
            key_of(&spec(Kind::Strip, 0.0, 1.0, 480)),
            key_of(&spec(Kind::Wave, 0.0, 1.0, 480))
        );
        assert_ne!(
            key_of(&spec(Kind::Strip, 0.0, 1.0, 480)),
            key_of(&spec(Kind::Strip, 0.0, 2.0, 480))
        );
        // A terminal resize (new pixel budget) is a new artifact.
        assert_ne!(
            key_of(&spec(Kind::Strip, 0.0, 1.0, 480)),
            key_of(&spec(Kind::Strip, 0.0, 1.0, 960))
        );
    }
}
