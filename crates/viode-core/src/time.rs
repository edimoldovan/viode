//! Timeline time: nanosecond precision, human-friendly TOML/CLI forms.
//! Accepts `12.5` (seconds), `"12.5"`, `"MM:SS.mmm"`, `"HH:MM:SS.mmm"`.
//! Serializes as `"HH:MM:SS.mmm"`.

use std::fmt;
use std::ops::{Add, Sub};

use gstreamer as gst;
use serde::{de, Deserialize, Deserializer, Serialize, Serializer};

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Default)]
pub struct Time(pub u64); // nanoseconds

#[derive(Debug, thiserror::Error)]
#[error("invalid time {0:?} (expected seconds or [HH:]MM:SS[.mmm])")]
pub struct TimeParseError(pub String);

impl Time {
    pub const ZERO: Time = Time(0);

    pub fn from_secs_f64(secs: f64) -> Result<Self, TimeParseError> {
        if !secs.is_finite() || secs < 0.0 {
            return Err(TimeParseError(secs.to_string()));
        }
        Ok(Time((secs * 1e9).round() as u64))
    }

    pub fn as_secs_f64(self) -> f64 {
        self.0 as f64 / 1e9
    }

    pub fn to_clocktime(self) -> gst::ClockTime {
        gst::ClockTime::from_nseconds(self.0)
    }

    pub fn from_clocktime(t: gst::ClockTime) -> Self {
        Time(t.nseconds())
    }

    pub fn parse(s: &str) -> Result<Self, TimeParseError> {
        let s = s.trim();
        let err = || TimeParseError(s.to_string());
        let parts: Vec<&str> = s.split(':').collect();
        if parts.len() > 3 || parts.is_empty() {
            return Err(err());
        }
        let secs: f64 = parts.last().unwrap().parse().map_err(|_| err())?;
        let mut total = secs;
        if parts.len() >= 2 {
            let mins: u64 = parts[parts.len() - 2].parse().map_err(|_| err())?;
            total += mins as f64 * 60.0;
        }
        if parts.len() == 3 {
            let hours: u64 = parts[0].parse().map_err(|_| err())?;
            total += hours as f64 * 3600.0;
        }
        Time::from_secs_f64(total).map_err(|_| err())
    }
}

impl fmt::Display for Time {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        let total_ms = self.0 / 1_000_000;
        let ms = total_ms % 1000;
        let total_s = total_ms / 1000;
        let (h, m, s) = (total_s / 3600, (total_s % 3600) / 60, total_s % 60);
        write!(f, "{h:02}:{m:02}:{s:02}.{ms:03}")
    }
}

impl Add for Time {
    type Output = Time;
    fn add(self, rhs: Time) -> Time {
        Time(self.0 + rhs.0)
    }
}

impl Sub for Time {
    type Output = Time;
    fn sub(self, rhs: Time) -> Time {
        Time(self.0.saturating_sub(rhs.0))
    }
}

impl Serialize for Time {
    fn serialize<S: Serializer>(&self, s: S) -> Result<S::Ok, S::Error> {
        s.serialize_str(&self.to_string())
    }
}

impl<'de> Deserialize<'de> for Time {
    fn deserialize<D: Deserializer<'de>>(d: D) -> Result<Self, D::Error> {
        struct V;
        impl de::Visitor<'_> for V {
            type Value = Time;
            fn expecting(&self, f: &mut fmt::Formatter) -> fmt::Result {
                f.write_str("seconds (number) or a \"[HH:]MM:SS.mmm\" string")
            }
            fn visit_f64<E: de::Error>(self, v: f64) -> Result<Time, E> {
                Time::from_secs_f64(v).map_err(E::custom)
            }
            fn visit_i64<E: de::Error>(self, v: i64) -> Result<Time, E> {
                self.visit_f64(v as f64)
            }
            fn visit_u64<E: de::Error>(self, v: u64) -> Result<Time, E> {
                self.visit_f64(v as f64)
            }
            fn visit_str<E: de::Error>(self, v: &str) -> Result<Time, E> {
                Time::parse(v).map_err(E::custom)
            }
        }
        d.deserialize_any(V)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parse_forms() {
        assert_eq!(Time::parse("2").unwrap(), Time(2_000_000_000));
        assert_eq!(Time::parse("1.5").unwrap(), Time(1_500_000_000));
        assert_eq!(Time::parse("01:30").unwrap(), Time(90_000_000_000));
        assert_eq!(
            Time::parse("01:00:01.250").unwrap(),
            Time(3_601_250_000_000)
        );
        assert!(Time::parse("nope").is_err());
        assert!(Time::parse("-1").is_err());
    }

    #[test]
    fn display_roundtrip() {
        let t = Time::parse("01:02:03.456").unwrap();
        assert_eq!(t.to_string(), "01:02:03.456");
        assert_eq!(Time::parse(&t.to_string()).unwrap(), t);
    }
}
