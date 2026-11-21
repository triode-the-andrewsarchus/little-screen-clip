use super::solar;
use chrono::{DateTime, Datelike, TimeZone, Timelike, Utc};
use egui::Color32;
use std::f64::consts::TAU;

const HOUR: f64 = 3_600.0;
const DAY: f64 = 86_400.0;
pub(super) const EARTH_YEAR: f64 = 365.2425 * DAY;
const J2000: f64 = 946_728_000.0;

pub(super) fn seconds(time: DateTime<Utc>) -> f64 {
    time.timestamp_millis() as f64 / 1000.0
}

pub(super) fn instant(seconds: f64) -> DateTime<Utc> {
    DateTime::from_timestamp_millis((seconds * 1_000.0).round() as i64).unwrap_or_else(|| {
        if seconds.is_sign_negative() {
            DateTime::<Utc>::MIN_UTC
        } else {
            DateTime::<Utc>::MAX_UTC
        }
    })
}

pub(super) struct Tick {
    pub time: f64,
    pub label: String,
}

pub(super) enum Calendar {
    Earth,
    World {
        day: Period,
        orbit: Period,
        tint: Color32,
    },
}

pub(super) struct Period {
    seconds: f64,
    body: &'static str,
    unit: &'static str,
}

impl Calendar {
    pub fn for_body(id: solar::Id) -> Self {
        let Some(body) = solar::body(id).filter(|body| body.id != solar::EARTH) else {
            return Self::Earth;
        };
        let mut lineage =
            std::iter::successors(Some(body), |body| body.parent.and_then(solar::body));
        let day = lineage.clone().find_map(|body| {
            solar::mean_day_hours(body).map(|hours| Period {
                seconds: hours * HOUR,
                body: body.name,
                unit: "day",
            })
        });
        let orbit = lineage.find_map(|body| {
            body.orbit_days().map(|days| Period {
                seconds: days * DAY,
                body: body.name,
                unit: if body.kind() == solar::Kind::Moon {
                    "orbit"
                } else {
                    "year"
                },
            })
        });
        Self::World {
            day: day.unwrap_or(Period {
                seconds: DAY,
                body: "Earth",
                unit: "day",
            }),
            orbit: orbit.unwrap_or(Period {
                seconds: EARTH_YEAR,
                body: "Earth",
                unit: "year",
            }),
            tint: body.color(),
        }
    }

    pub fn caption(&self, span: f64) -> String {
        match self {
            Self::Earth if span < 12.0 * DAY => "Daylight at 0°, 0°".into(),
            Self::Earth if span < 4.0 * EARTH_YEAR => "Northern seasons".into(),
            Self::Earth => "Earth calendar".into(),
            Self::World { day, orbit, .. } => {
                format!("{} {} · {} {}", day.body, day.unit, orbit.body, orbit.unit)
            }
        }
    }

    pub fn ticks(&self, range: [f64; 2], target: f64) -> Vec<Tick> {
        match self {
            Self::Earth => earth_ticks(range, target),
            Self::World { day, orbit, .. } => world_ticks(range, target, day, orbit),
        }
    }

    pub fn color(&self, time: f64, scale: f64) -> [Color32; 2] {
        let top = match self {
            Self::Earth => earth_color(time, scale),
            Self::World { day, orbit, tint } => {
                let orbital_phase = ((time - J2000) / orbit.seconds).rem_euclid(1.0);
                let day_phase = ((time - J2000) / day.seconds).rem_euclid(1.0);
                let orbit_color = mix(
                    Color32::from_rgb(29, 41, 61),
                    *tint,
                    0.21 + 0.10 * (orbital_phase * TAU).cos() as f32,
                );
                let day_strength = 1.0 - smoothstep(scale * 100.0 / day.seconds, 0.25, 0.9);
                mix(
                    orbit_color,
                    *tint,
                    (day_strength * (0.13 + 0.12 * (day_phase * TAU).cos())) as f32,
                )
            }
        };
        [top, mix(top, Color32::BLACK, 0.28)]
    }
}

fn earth_ticks(range: [f64; 2], target: f64) -> Vec<Tick> {
    if target < 18.0 * HOUR {
        return regular_ticks(range, 0.0, intraday_step(target), |time, _| {
            let date = instant(time);
            let format = if date.num_seconds_from_midnight() == 0 {
                "%b %-d"
            } else {
                "%H:%M"
            };
            date.format(format).to_string()
        });
    }
    if target < 19.0 * DAY {
        return regular_ticks(range, 0.0, DAY * nice_step(target / DAY), |time, _| {
            instant(time).format("%b %-d").to_string()
        });
    }
    let (stride, phase) = if target < 52.0 * DAY {
        (1, 0)
    } else if target < 140.0 * DAY {
        (3, 2)
    } else {
        (12 * nice_step(target / EARTH_YEAR).max(1.0) as i32, 0)
    };
    let date = instant(range[0]);
    let month = date.year() * 12 + date.month0() as i32;
    let start = month - (month - phase).rem_euclid(stride);
    (start..)
        .step_by(stride as usize)
        .map_while(|index| {
            let month = index.rem_euclid(12) as u32;
            let year = index.div_euclid(12);
            let date = Utc.with_ymd_and_hms(year, month + 1, 1, 0, 0, 0).single()?;
            let time = seconds(date);
            if time > range[1] {
                return None;
            }
            let label = match stride {
                1 => date.format("%b").to_string(),
                3 => ["Spring", "Summer", "Autumn", "Winter"][(month / 3) as usize].into(),
                _ => year.to_string(),
            };
            Some(Tick { time, label })
        })
        .filter(|tick| tick.time >= range[0])
        .collect()
}

fn world_ticks(range: [f64; 2], target: f64, day: &Period, orbit: &Period) -> Vec<Tick> {
    if target < day.seconds * 0.34 && target < orbit.seconds * 0.22 {
        let step = intraday_step(target);
        return regular_ticks(range, J2000, step, |time, _| {
            let elapsed = (time - J2000).rem_euclid(day.seconds);
            let minutes = (elapsed / 60.0) as i64;
            format!("{:02}:{:02}", minutes / 60, minutes % 60)
        });
    }
    let period = if target < (orbit.seconds * 0.22).max(day.seconds * 0.75) {
        day
    } else {
        orbit
    };
    let minimum = if period.unit == "day" { 1.0 } else { 0.25 };
    let units = nice_step(target / period.seconds).max(minimum);
    regular_ticks(range, J2000, units * period.seconds, |_, index| {
        let tag = match period.unit {
            "day" => "Day",
            "orbit" => "Orbit",
            _ => "Year",
        };
        format!("{tag} {}", index as f64 * units)
    })
}

fn intraday_step(target: f64) -> f64 {
    [
        1.0, 2.0, 5.0, 10.0, 15.0, 30.0, 60.0, 120.0, 180.0, 360.0, 720.0, 1440.0,
    ]
    .into_iter()
    .map(|minutes| minutes * 60.0)
    .find(|step| *step >= target)
    .unwrap_or_else(|| DAY * nice_step(target / DAY))
}

fn nice_step(target: f64) -> f64 {
    let power = 10.0_f64.powf(target.log10().floor());
    [1.0, 2.0, 5.0, 10.0]
        .into_iter()
        .map(|multiple| multiple * power)
        .find(|step| *step >= target)
        .unwrap_or(10.0 * power)
}

fn regular_ticks(
    range: [f64; 2],
    epoch: f64,
    step: f64,
    label: impl Fn(f64, i64) -> String,
) -> Vec<Tick> {
    let start = ((range[0] - epoch) / step).ceil() as i64;
    (start..)
        .map_while(|index| {
            let time = epoch + index as f64 * step;
            (time <= range[1]).then(|| Tick {
                time,
                label: label(time, index),
            })
        })
        .collect()
}

fn earth_color(time: f64, scale: f64) -> Color32 {
    let date = instant(time);
    let phase = (f64::from(date.ordinal0()) - 45.0) / 365.2425;
    let seasons = [
        Color32::from_rgb(44, 59, 95),
        Color32::from_rgb(34, 80, 74),
        Color32::from_rgb(106, 77, 43),
        Color32::from_rgb(98, 57, 57),
    ];
    let quarter = phase.rem_euclid(1.0) * 4.0;
    let index = quarter.floor() as usize;
    let season = mix(
        seasons[index],
        seasons[(index + 1) % 4],
        quarter.fract() as f32,
    );
    let season = mix(
        season,
        Color32::from_rgb(53, 61, 79),
        smoothstep(scale * 100.0 / DAY, 100.0, 380.0) as f32,
    );
    let daylight = light_at_origin(date);
    let night = Color32::from_rgb(23, 35, 69);
    let dawn = Color32::from_rgb(121, 66, 70);
    let day = Color32::from_rgb(133, 104, 52);
    let twilight = mix(night, dawn, smoothstep(daylight, -0.24, 0.02) as f32);
    let daily = mix(twilight, day, smoothstep(daylight, -0.04, 0.55) as f32);
    mix(
        daily,
        season,
        smoothstep(scale * 100.0 / DAY, 0.45, 1.35) as f32,
    )
}

fn light_at_origin(date: DateTime<Utc>) -> f64 {
    // NOAA's equation of time. At latitude zero, cos(declination) is
    // positive year-round, so the local solar hour angle locates twilight.
    // https://www.gml.noaa.gov/grad/solcalc/solareqns.PDF
    let fraction = f64::from(date.num_seconds_from_midnight()) / DAY;
    let year_days = if date.date_naive().leap_year() {
        366.0
    } else {
        365.0
    };
    let phase = (f64::from(date.ordinal0()) + fraction - 0.5) * TAU / year_days;
    let (sin, cos) = phase.sin_cos();
    let (double_sin, double_cos) = (2.0 * phase).sin_cos();
    let offset = 229.18
        * (0.000075 + 0.001868 * cos
            - 0.032077 * sin
            - 0.014615 * double_cos
            - 0.040849 * double_sin);
    ((fraction + offset / 1440.0 - 0.5) * TAU).cos()
}

fn smoothstep(value: f64, start: f64, end: f64) -> f64 {
    let t = ((value - start) / (end - start)).clamp(0.0, 1.0);
    t * t * (3.0 - 2.0 * t)
}

fn mix(left: Color32, right: Color32, weight: f32) -> Color32 {
    let channel = |a: u8, b: u8| {
        (f32::from(a) + f32::from(i16::from(b) - i16::from(a)) * weight).round() as u8
    };
    Color32::from_rgb(
        channel(left.r(), right.r()),
        channel(left.g(), right.g()),
        channel(left.b(), right.b()),
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn earth_uses_civil_minutes_months_northern_seasons_and_years() {
        let midnight = seconds(Utc.with_ymd_and_hms(2024, 1, 1, 0, 0, 0).unwrap());
        let minutes = earth_ticks([midnight, midnight + 600.0], 58.0);
        assert_eq!(minutes[1].label, "00:01");
        let months = earth_ticks([midnight, midnight + 180.0 * DAY], 28.0 * DAY);
        assert_eq!(months[1].label, "Feb");
        let seasons = earth_ticks([midnight, midnight + EARTH_YEAR], 75.0 * DAY);
        assert_eq!(seasons[2].label, "Autumn");
        let years = earth_ticks([midnight, midnight + 5.0 * EARTH_YEAR], 140.0 * DAY);
        assert_eq!(years[1].label, "2025");
        assert!(light_at_origin(instant(midnight)) < -0.9);
        assert!(light_at_origin(instant(midnight + DAY / 2.0)) > 0.9);
    }

    #[test]
    fn moon_uses_its_own_orbit_even_when_its_day_uses_the_parent() {
        let Calendar::World { day, orbit, .. } = Calendar::for_body(506) else {
            panic!("Himalia needs a body-relative calendar");
        };
        assert_eq!(day.body, "Jupiter");
        assert_eq!(orbit.body, "Himalia");
        assert_eq!(orbit.unit, "orbit");
        assert!((orbit.seconds / DAY - 249.909).abs() < 1e-9);
        let Calendar::World { day, orbit, .. } = Calendar::for_body(solar::MOON) else {
            panic!("the Moon needs a body-relative calendar");
        };
        assert!(day.seconds > orbit.seconds);
    }
}
