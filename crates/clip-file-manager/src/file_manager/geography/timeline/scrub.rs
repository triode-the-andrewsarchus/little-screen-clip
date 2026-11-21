use crate::file_manager::geography::interpolate_instant;
use chrono::{DateTime, TimeDelta, Utc};

const LOCAL_STEP: TimeDelta = TimeDelta::minutes(12);
const FAST_SCRUB_CONTRAST: f32 = 0.28;
const FADE_OUT_SECONDS: f64 = 0.12;
const FADE_IN_SECONDS: f64 = 0.32;
const QUIET_SECONDS: f64 = 0.20;

#[derive(Clone, Copy)]
pub(in crate::file_manager::geography) struct Preview {
    pub time: DateTime<Utc>,
    pub directional_light: f32,
}

impl Preview {
    pub fn exact(time: DateTime<Utc>) -> Self {
        Self {
            time,
            directional_light: 1.0,
        }
    }
}

pub(super) struct Scrub {
    displayed: DateTime<Utc>,
    target: DateTime<Utc>,
    light: f64,
    updated: f64,
    quiet_until: f64,
}

impl Scrub {
    pub fn new(time: DateTime<Utc>, now: f64) -> Self {
        Self {
            displayed: time,
            target: time,
            light: 1.0,
            updated: now,
            quiet_until: now,
        }
    }

    pub fn retarget(&mut self, target: DateTime<Utc>, now: f64) {
        self.target = target;
        if self.large_step() || now < self.quiet_until {
            self.quiet_until = now + QUIET_SECONDS;
        }
    }

    fn large_step(&self) -> bool {
        (self.target - self.displayed).abs() > LOCAL_STEP
    }

    /// Traverse short intervals normally. For distant dates, soften the contrast
    /// before following the playhead so the changing phase stays visible.
    pub fn frame(&mut self, now: f64) -> Preview {
        let dt = (now - self.updated).clamp(0.0, 0.05);
        self.updated = now;
        if self.large_step() || now < self.quiet_until {
            self.light = (self.light - dt / FADE_OUT_SECONDS).max(0.0);
            if self.light == 0.0 {
                self.displayed = self.target;
            }
        } else {
            self.displayed = if (self.target - self.displayed).abs() <= TimeDelta::seconds(1) {
                self.target
            } else {
                interpolate_instant(self.displayed, self.target, 1.0 - (-dt / 0.065).exp())
            };
            self.light = (self.light + dt / FADE_IN_SECONDS).min(1.0);
        }
        let eased = (self.light * self.light * (3.0 - 2.0 * self.light)) as f32;
        let directional_light = FAST_SCRUB_CONTRAST + (1.0 - FAST_SCRUB_CONTRAST) * eased;
        Preview {
            time: self.displayed,
            directional_light,
        }
    }

    pub fn settled(&self) -> bool {
        self.light == 1.0 && self.displayed == self.target && self.updated >= self.quiet_until
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn rapid_scrub_follows_changes_of_day_at_reduced_contrast_then_restores_it() {
        let start = DateTime::UNIX_EPOCH;
        let mut scrub = Scrub::new(start, 0.0);
        let mut target = start;
        for frame in 1..=22 {
            let now = f64::from(frame) / 60.0;
            target = start + TimeDelta::hours(i64::from(frame) * 30);
            scrub.retarget(target, now);
            let preview = scrub.frame(now);
            if preview.directional_light > FAST_SCRUB_CONTRAST {
                assert_eq!(preview.time, start);
            } else {
                assert_eq!(preview.time, target);
            }
        }
        let mut previous = FAST_SCRUB_CONTRAST;
        for frame in 23..=70 {
            let preview = scrub.frame(f64::from(frame) / 60.0);
            assert_eq!(preview.time, target);
            assert!(preview.directional_light >= previous);
            previous = preview.directional_light;
        }
        assert!(scrub.settled());
        assert_eq!(previous, 1.0);
    }

    #[test]
    fn small_adjustments_ease_time_without_dimming_the_surface() {
        let start = DateTime::UNIX_EPOCH;
        let target = start + TimeDelta::minutes(2);
        let mut scrub = Scrub::new(start, 1.0);
        scrub.retarget(target, 1.0);
        let first = scrub.frame(1.016);
        assert!(first.time > start && first.time < target);
        assert_eq!(first.directional_light, 1.0);
        for frame in 1..=35 {
            scrub.frame(1.0 + f64::from(frame) / 60.0);
        }
        assert!(scrub.settled());
    }
}
