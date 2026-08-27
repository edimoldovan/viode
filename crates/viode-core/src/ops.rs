//! Edit operations — pure functions over the Project model. No I/O here;
//! side effects (probing, rendering, file copies) live at the edges.

use crate::model::{Clip, Project};
use crate::time::Time;

#[derive(Debug, thiserror::Error)]
pub enum OpError {
    #[error("clip index {0} out of range (timeline has {1} clips)")]
    BadIndex(usize, usize),
    #[error("invalid trim: in {0} must be before out {1}")]
    BadRange(Time, Time),
    #[error("split point {0} outside clip (length {1})")]
    BadSplit(Time, Time),
}

fn check_index(project: &Project, index: usize) -> Result<(), OpError> {
    if index >= project.clips.len() {
        return Err(OpError::BadIndex(index, project.clips.len()));
    }
    Ok(())
}

pub fn add(project: &mut Project, clip: Clip) -> Result<(), OpError> {
    if clip.in_ >= clip.out {
        return Err(OpError::BadRange(clip.in_, clip.out));
    }
    project.clips.push(clip);
    Ok(())
}

pub fn remove(project: &mut Project, index: usize) -> Result<Clip, OpError> {
    check_index(project, index)?;
    Ok(project.clips.remove(index))
}

pub fn move_clip(project: &mut Project, from: usize, to: usize) -> Result<(), OpError> {
    check_index(project, from)?;
    check_index(project, to)?;
    let clip = project.clips.remove(from);
    project.clips.insert(to, clip);
    Ok(())
}

/// Adjust a clip's source in/out points. `None` leaves a bound unchanged.
pub fn trim(
    project: &mut Project,
    index: usize,
    in_: Option<Time>,
    out: Option<Time>,
) -> Result<(), OpError> {
    check_index(project, index)?;
    let clip = &mut project.clips[index];
    let new_in = in_.unwrap_or(clip.in_);
    let new_out = out.unwrap_or(clip.out);
    if new_in >= new_out {
        return Err(OpError::BadRange(new_in, new_out));
    }
    clip.in_ = new_in;
    clip.out = new_out;
    Ok(())
}

/// Split a clip at `at` (offset from the clip's own start) into two clips.
pub fn split(project: &mut Project, index: usize, at: Time) -> Result<(), OpError> {
    check_index(project, index)?;
    let clip = &project.clips[index];
    if at == Time::ZERO || at >= clip.len() {
        return Err(OpError::BadSplit(at, clip.len()));
    }
    let mid = clip.in_ + at;
    let mut right = clip.clone();
    right.in_ = mid;
    project.clips[index].out = mid;
    project.clips.insert(index + 1, right);
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::model::Project;

    fn clip(in_s: f64, out_s: f64) -> Clip {
        Clip {
            src: "media/x.mp4".into(),
            in_: Time::from_secs_f64(in_s).unwrap(),
            out: Time::from_secs_f64(out_s).unwrap(),
            label: None,
        }
    }

    fn project() -> Project {
        let mut p = Project::new("t", 30.0, [1920, 1080]);
        p.clips.push(clip(0.0, 3.0));
        p.clips.push(clip(1.0, 4.0));
        p
    }

    #[test]
    fn split_preserves_total() {
        let mut p = project();
        let before = p.total_duration();
        split(&mut p, 0, Time::from_secs_f64(1.5).unwrap()).unwrap();
        assert_eq!(p.clips.len(), 3);
        assert_eq!(p.total_duration(), before);
        assert_eq!(p.clips[0].out, p.clips[1].in_);
    }

    #[test]
    fn split_rejects_bounds() {
        let mut p = project();
        assert!(split(&mut p, 0, Time::ZERO).is_err());
        assert!(split(&mut p, 0, Time::from_secs_f64(3.0).unwrap()).is_err());
    }

    #[test]
    fn trim_validates() {
        let mut p = project();
        assert!(trim(&mut p, 0, None, Some(Time::from_secs_f64(0.0).unwrap())).is_err());
        trim(&mut p, 0, Some(Time::from_secs_f64(0.5).unwrap()), None).unwrap();
        assert_eq!(p.clips[0].in_, Time::from_secs_f64(0.5).unwrap());
    }

    #[test]
    fn move_and_remove() {
        let mut p = project();
        move_clip(&mut p, 1, 0).unwrap();
        assert_eq!(p.clips[0].in_, Time::from_secs_f64(1.0).unwrap());
        remove(&mut p, 1).unwrap();
        assert_eq!(p.clips.len(), 1);
        assert!(remove(&mut p, 5).is_err());
    }

    #[test]
    fn positions_are_gapless() {
        let p = project();
        let pos = p.positions();
        assert_eq!(pos[0], Time::ZERO);
        assert_eq!(pos[1], Time::from_secs_f64(3.0).unwrap());
    }
}
