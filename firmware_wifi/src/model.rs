use crate::config::MEASUREMENT_FIELD_COUNT;

#[derive(Debug, Clone, Copy)]
pub struct Measurement {
    pub wind_speed_ms: Option<f32>,
    pub wind_force: Option<f32>,
    pub wind_direction_sector: Option<f32>,
    pub wind_direction_deg: Option<f32>,
    pub humidity_pct: Option<f32>,
    pub temperature_c: Option<f32>,
    pub pressure_kpa: Option<f32>,
    pub illuminance_lux: Option<f32>,
    pub illuminance_hundreds_lux: Option<f32>,
    pub rainfall_mm: Option<f32>,
}

impl Measurement {
    pub fn values(self) -> [Option<f32>; MEASUREMENT_FIELD_COUNT] {
        [
            self.wind_speed_ms,
            self.wind_force,
            self.wind_direction_sector,
            self.wind_direction_deg,
            self.humidity_pct,
            self.temperature_c,
            self.pressure_kpa,
            self.illuminance_lux,
            self.illuminance_hundreds_lux,
            self.rainfall_mm,
        ]
    }

    pub fn from_values(v: [Option<f32>; MEASUREMENT_FIELD_COUNT]) -> Self {
        Self {
            wind_speed_ms: v[0], wind_force: v[1], wind_direction_sector: v[2],
            wind_direction_deg: v[3], humidity_pct: v[4], temperature_c: v[5],
            pressure_kpa: v[6], illuminance_lux: v[7], illuminance_hundreds_lux: v[8],
            rainfall_mm: v[9],
        }
    }

    pub fn valid_mask(self) -> u16 {
        self.values().iter().enumerate().fold(0_u16, |mask, (i, value)| {
            mask | if value.is_some() { 1_u16 << i } else { 0 }
        })
    }
}

#[cfg(test)]
mod tests {
    use super::Measurement;

    #[test]
    fn all_ten_metrics_have_distinct_bits() {
        let empty = Measurement::from_values([None; 10]);
        assert_eq!(empty.valid_mask(), 0);
        for index in 0..10 {
            let mut values = [None; 10];
            values[index] = Some(0.0);
            assert_eq!(Measurement::from_values(values).valid_mask(), 1_u16 << index);
        }
        assert_eq!(Measurement::from_values([Some(0.0); 10]).valid_mask(), 0x03ff);
    }
}
