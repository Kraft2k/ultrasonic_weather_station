//! Read-only Modbus RTU protocol for Renke RS-FSXCS-N01-*-EX, manual V1.8.

use core::fmt;

pub const FIRST_REGISTER: u16 = 500;
pub const REGISTER_COUNT: u16 = 14; // 500..=513; includes optional elements.
pub const RESPONSE_LEN: usize = 3 + 2 * REGISTER_COUNT as usize + 2;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum FrameError {
    Length,
    Crc,
    Address,
    Function,
    ByteCount,
    Exception(u8),
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Measurements {
    pub wind_speed_tenths_ms: u16,
    pub wind_force: u16,
    pub wind_direction_sector: u16,
    pub wind_direction_degrees: u16,
    pub humidity_tenths_percent: u16,
    pub temperature_tenths_c: i16,
    pub pressure_tenths_kpa: u16,
    pub illuminance_lux: u32,
    pub illuminance_hundreds_lux: u16,
    pub rainfall_tenths_mm: u16,
}

impl fmt::Display for Measurements {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        let temperature = i32::from(self.temperature_tenths_c);
        write!(
            f,
            "wind {}.{:01} m/s, force {}, direction {} deg (sector {}), \
             T {}{}.{:01} C, RH {}.{:01}%, pressure {}.{:01} kPa, \
             light {} lux ({} x 100 lux), rain {}.{:01} mm",
            self.wind_speed_tenths_ms / 10,
            self.wind_speed_tenths_ms % 10,
            self.wind_force,
            self.wind_direction_degrees,
            self.wind_direction_sector,
            if temperature < 0 { "-" } else { "" },
            temperature.abs() / 10,
            temperature.abs() % 10,
            self.humidity_tenths_percent / 10,
            self.humidity_tenths_percent % 10,
            self.pressure_tenths_kpa / 10,
            self.pressure_tenths_kpa % 10,
            self.illuminance_lux,
            self.illuminance_hundreds_lux,
            self.rainfall_tenths_mm / 10,
            self.rainfall_tenths_mm % 10,
        )
    }
}

pub fn crc16(bytes: &[u8]) -> u16 {
    let mut crc = 0xffff_u16;
    for &byte in bytes {
        crc ^= u16::from(byte);
        for _ in 0..8 {
            crc = if crc & 1 == 0 {
                crc >> 1
            } else {
                (crc >> 1) ^ 0xa001
            };
        }
    }
    crc
}

pub fn request(address: u8) -> [u8; 8] {
    let [start_hi, start_lo] = FIRST_REGISTER.to_be_bytes();
    let [count_hi, count_lo] = REGISTER_COUNT.to_be_bytes();
    let mut frame = [address, 0x03, start_hi, start_lo, count_hi, count_lo, 0, 0];
    let crc = crc16(&frame[..6]).to_le_bytes();
    frame[6..].copy_from_slice(&crc);
    frame
}

pub fn parse(address: u8, frame: &[u8]) -> Result<Measurements, FrameError> {
    if frame.len() != RESPONSE_LEN && frame.len() != 5 {
        return Err(FrameError::Length);
    }
    if crc16(frame) != 0 {
        return Err(FrameError::Crc);
    }
    if frame[0] != address {
        return Err(FrameError::Address);
    }
    if frame[1] == 0x83 && frame.len() == 5 {
        return Err(FrameError::Exception(frame[2]));
    }
    if frame[1] != 0x03 {
        return Err(FrameError::Function);
    }
    if frame.len() != RESPONSE_LEN || frame[2] != (2 * REGISTER_COUNT) as u8 {
        return Err(FrameError::ByteCount);
    }

    let register = |offset: usize| {
        let start = 3 + 2 * offset;
        u16::from_be_bytes([frame[start], frame[start + 1]])
    };
    Ok(Measurements {
        wind_speed_tenths_ms: register(0),
        wind_force: register(1),
        wind_direction_sector: register(2),
        wind_direction_degrees: register(3),
        humidity_tenths_percent: register(4),
        temperature_tenths_c: register(5) as i16,
        pressure_tenths_kpa: register(9),
        illuminance_lux: (u32::from(register(10)) << 16) | u32::from(register(11)),
        illuminance_hundreds_lux: register(12),
        rainfall_tenths_mm: register(13),
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn response(registers: [u16; REGISTER_COUNT as usize]) -> [u8; RESPONSE_LEN] {
        let mut frame = [0u8; RESPONSE_LEN];
        frame[..3].copy_from_slice(&[1, 3, (REGISTER_COUNT * 2) as u8]);
        for (index, value) in registers.into_iter().enumerate() {
            frame[3 + 2 * index..5 + 2 * index].copy_from_slice(&value.to_be_bytes());
        }
        let crc = crc16(&frame[..RESPONSE_LEN - 2]).to_le_bytes();
        frame[RESPONSE_LEN - 2..].copy_from_slice(&crc);
        frame
    }

    #[test]
    fn request_uses_documented_zero_based_register() {
        // V1.8 manual, section 5.4.1: read wind speed at register 500.
        assert_eq!(crc16(&[1, 3, 0x01, 0xf4, 0, 1]), 0x04c4);
        assert_eq!(crc16(&[1, 3, 2, 0, 125, 0x78, 0x65]), 0);
        assert_eq!(request(1), [1, 3, 0x01, 0xf4, 0, 14, 0x84, 0x00]);
    }

    #[test]
    fn parses_all_selected_fields_and_signed_temperature() {
        let mut registers = [0u16; REGISTER_COUNT as usize];
        registers[0] = 125;
        registers[1] = 6;
        registers[2] = 2;
        registers[3] = 90;
        registers[4] = 658;
        registers[5] = (-101i16) as u16;
        registers[9] = 1013;
        registers[10] = 3;
        registers[11] = 3392; // 200000 lux
        registers[12] = 2000;
        registers[13] = 27;
        assert_eq!(
            parse(1, &response(registers)),
            Ok(Measurements {
                wind_speed_tenths_ms: 125,
                wind_force: 6,
                wind_direction_sector: 2,
                wind_direction_degrees: 90,
                humidity_tenths_percent: 658,
                temperature_tenths_c: -101,
                pressure_tenths_kpa: 1013,
                illuminance_lux: 200000,
                illuminance_hundreds_lux: 2000,
                rainfall_tenths_mm: 27,
            })
        );
    }

    #[test]
    fn rejects_bad_crc_wrong_address_and_modbus_exception() {
        let mut frame = response([0; REGISTER_COUNT as usize]);
        frame[4] ^= 1;
        assert_eq!(parse(1, &frame), Err(FrameError::Crc));
        frame[4] ^= 1;
        assert_eq!(parse(2, &frame), Err(FrameError::Address));
        let mut exception = [1, 0x83, 2, 0, 0];
        let crc = crc16(&exception[..3]).to_le_bytes();
        exception[3..].copy_from_slice(&crc);
        assert_eq!(parse(1, &exception), Err(FrameError::Exception(2)));
    }
}
