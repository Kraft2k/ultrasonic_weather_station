//! Modbus RTU exchange for the Renke RS-FSXCS-N01-3HP-EX weather station.
use anyhow::{anyhow, Context, Result};
use esp_idf_hal::{
    delay::{FreeRtos, TickType},
    gpio::{AnyOutputPin, Output, PinDriver},
    uart::UartDriver,
};

const ADDRESS: u8 = 1;
const FIRST_REGISTER: u16 = 500;
const REGISTER_COUNT: u16 = 14;
const RESPONSE_LEN: usize = 3 + REGISTER_COUNT as usize * 2 + 2;

#[derive(Clone, Copy, Debug)]
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

impl core::fmt::Display for Measurements {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        write!(f, "wind {:.1} m/s force {} direction {}° (sector {}), T {:.1} °C RH {:.1}%, pressure {:.1} kPa, light {} lux ({} × 100 lux), rain {:.1} mm",
            self.wind_speed_tenths_ms as f32 / 10.0, self.wind_force,
            self.wind_direction_degrees, self.wind_direction_sector,
            self.temperature_tenths_c as f32 / 10.0,
            self.humidity_tenths_percent as f32 / 10.0,
            self.pressure_tenths_kpa as f32 / 10.0, self.illuminance_lux,
            self.illuminance_hundreds_lux, self.rainfall_tenths_mm as f32 / 10.0)
    }
}

pub fn read_measurements(
    uart: &UartDriver<'_>,
    direction: &mut PinDriver<'_, AnyOutputPin, Output>,
) -> Result<Measurements> {
    let [r_hi, r_lo] = FIRST_REGISTER.to_be_bytes();
    let [n_hi, n_lo] = REGISTER_COUNT.to_be_bytes();
    let mut request = [ADDRESS, 3, r_hi, r_lo, n_hi, n_lo, 0, 0];
    let request_crc = crc16(&request[..6]).to_le_bytes();
    request[6..].copy_from_slice(&request_crc);
    let mut last_error = None;
    for attempt in 1..=3 {
        let mut stale = [0_u8; 32];
        while uart.read(&mut stale, TickType::new_millis(1).ticks()).unwrap_or(0) > 0 {}
        FreeRtos::delay_ms(8);
        direction.set_high().context("enable RS-485 transmitter")?;
        FreeRtos::delay_ms(1);
        let tx = write_all(uart, &request).and_then(|_| {
            uart.wait_tx_done(TickType::new_millis(100).ticks()).context("flush Modbus UART")
        });
        direction.set_low().context("enable RS-485 receiver")?;
        tx?;
        match read_response(uart) {
            Ok(frame) => return parse(&frame),
            Err(err) => { log::warn!("Modbus attempt {attempt}/3: {err:#}"); last_error = Some(err); }
        }
    }
    Err(last_error.unwrap_or_else(|| anyhow!("Modbus request failed")))
}

fn write_all(uart: &UartDriver<'_>, mut bytes: &[u8]) -> Result<()> {
    while !bytes.is_empty() {
        let n = uart.write(bytes).context("write Modbus request")?;
        if n == 0 { anyhow::bail!("UART made no progress") }
        bytes = &bytes[n..];
    }
    Ok(())
}

fn read_response(uart: &UartDriver<'_>) -> Result<Vec<u8>> {
    let mut frame = vec![0_u8; RESPONSE_LEN];
    let mut offset = 0;
    while offset < 3 { offset += read_some(uart, &mut frame[offset..3], 400)?; }
    let length = if frame[1] & 0x80 != 0 { 5 } else { RESPONSE_LEN };
    while offset < length { offset += read_some(uart, &mut frame[offset..length], 400)?; }
    frame.truncate(length);
    if frame.len() < 5 { anyhow::bail!("short Modbus response") }
    if frame[0] != ADDRESS { anyhow::bail!("wrong Modbus address {}", frame[0]) }
    if frame[1] == 0x83 { anyhow::bail!("Modbus exception {}", frame[2]) }
    if frame[1] != 3 { anyhow::bail!("unexpected Modbus function {}", frame[1]) }
    if frame.len() != RESPONSE_LEN || frame[2] != (2 * REGISTER_COUNT) as u8 {
        anyhow::bail!("unexpected Modbus response length/byte count")
    }
    let expected_crc = crc16(&frame[..frame.len() - 2]).to_le_bytes();
    if frame[frame.len() - 2..] != expected_crc[..] {
        anyhow::bail!("Modbus CRC mismatch")
    }
    Ok(frame)
}

fn read_some(uart: &UartDriver<'_>, buf: &mut [u8], timeout_ms: u32) -> Result<usize> {
    let mut remaining = timeout_ms;
    while remaining > 0 {
        let wait = remaining.min(10);
        match uart.read(buf, TickType::new_millis(u64::from(wait)).ticks()) {
            Ok(n) if n > 0 => return Ok(n),
            Ok(_) => remaining -= wait,
            Err(err) if err.code() == esp_idf_sys::ESP_ERR_TIMEOUT => remaining -= wait,
            Err(err) => return Err(err).context("read Modbus UART"),
        }
    }
    anyhow::bail!("Modbus response timeout")
}

fn parse(frame: &[u8]) -> Result<Measurements> {
    let reg = |i: usize| u16::from_be_bytes([frame[3 + 2*i], frame[4 + 2*i]]);
    Ok(Measurements {
        wind_speed_tenths_ms: reg(0), wind_force: reg(1), wind_direction_sector: reg(2),
        wind_direction_degrees: reg(3), humidity_tenths_percent: reg(4),
        temperature_tenths_c: reg(5) as i16, pressure_tenths_kpa: reg(9),
        illuminance_lux: (u32::from(reg(10)) << 16) | u32::from(reg(11)),
        illuminance_hundreds_lux: reg(12), rainfall_tenths_mm: reg(13),
    })
}

fn crc16(bytes: &[u8]) -> u16 {
    let mut crc = 0xffff_u16;
    for byte in bytes {
        crc ^= u16::from(*byte);
        for _ in 0..8 { crc = if crc & 1 == 0 { crc >> 1 } else { (crc >> 1) ^ 0xa001 }; }
    }
    crc
}
