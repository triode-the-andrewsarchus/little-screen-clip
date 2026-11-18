//! Display units for geographic distances and elevations; source data remains SI.
use serde::{Deserialize, Serialize};

#[derive(Clone, Copy, Debug, Default, Deserialize, Serialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum Units {
    #[default]
    Metric,
    Imperial,
}

impl Units {
    fn distance_unit(self, meters: f64) -> (f64, &'static str) {
        match self {
            Self::Metric if meters.abs() >= 1000.0 => (1000.0, "km"),
            Self::Metric => (1.0, "m"),
            Self::Imperial if meters.abs() >= 1609.344 => (1609.344, "mi"),
            Self::Imperial if meters.abs() >= 30.48 => (0.9144, "yd"),
            Self::Imperial => (0.3048, "ft"),
        }
    }

    pub fn distance(self, meters: f64) -> String {
        let (factor, suffix) = self.distance_unit(meters);
        format!("{:.2} {suffix}", meters / factor)
    }

    pub fn elevation(self, meters: f64) -> String {
        let (factor, suffix) = match self {
            Self::Metric => (1.0, "m"),
            Self::Imperial => (0.3048, "ft"),
        };
        format!("{:.1} {suffix}", meters / factor)
    }

    pub fn elevation_range(self, low: f64, high: f64) -> String {
        format!("{} – {}", self.elevation(low), self.elevation(high))
    }

    /// Returns the exact physical length and label of a 1/2/5 scale bar.
    pub fn scale_bar(self, target_m: f64) -> (f64, String) {
        let (factor, suffix) = self.distance_unit(target_m);
        let target = (target_m / factor).max(1e-9);
        let decade = 10_f64.powf(target.log10().floor());
        let step = [5.0, 2.0, 1.0]
            .into_iter()
            .find(|v| v * decade <= target)
            .unwrap_or(1.0);
        let length = step * decade;
        (length * factor, format!("{length} {suffix}"))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn config_preserves_old_metric_default_and_round_trips_imperial() {
        let metric: crate::config::AppConfig = toml::from_str("[geography]").unwrap();
        assert_eq!(metric.geography.units, Units::Metric);
        let imperial: crate::config::AppConfig = toml::from_str(
            r#"[geography]
units = "imperial""#,
        )
        .unwrap();
        assert_eq!(imperial.geography.units, Units::Imperial);
        let saved = toml::to_string(&imperial).unwrap();
        let loaded: crate::config::AppConfig = toml::from_str(&saved).unwrap();
        assert_eq!(loaded.geography.units, Units::Imperial);
        assert!(
            toml::from_str::<crate::config::AppConfig>(
                r#"[geography]
units = "unknown""#
            )
            .is_err()
        );
    }

    #[test]
    fn imperial_distances_and_elevations_preserve_physical_lengths() {
        assert_eq!(Units::Imperial.distance(1609.344), "1.00 mi");
        assert_eq!(Units::Imperial.distance(91.44), "100.00 yd");
        assert_eq!(Units::Imperial.distance(0.3048), "1.00 ft");
        assert_eq!(Units::Imperial.elevation(-30.48), "-100.0 ft");
        assert_eq!(Units::Metric.distance(1609.344), "1.61 km");
        for units in [Units::Metric, Units::Imperial] {
            for target in [0.1, 50.0, 500.0, 5000.0, 5e6] {
                let (meters, _) = units.scale_bar(target);
                assert!(meters <= target * 1.000001 && meters >= target * 0.39);
            }
        }
        assert_eq!(Units::Imperial.scale_bar(2000.0), (1609.344, "1 mi".into()));
    }
}
