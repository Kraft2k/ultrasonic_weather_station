use anyhow::{Context, Result};
use esp_idf_hal::{
    delay::FreeRtos,
    gpio::{AnyOutputPin, Output, PinDriver},
    peripherals::Peripherals,
    prelude::*,
    uart::{config::{DataBits, StopBits}, UartConfig, UartDriver},
};
use esp_idf_svc::{log::EspLogger, nvs::EspDefaultNvsPartition};
use esp_idf_sys as _;
use log::{error, info};

mod config;
mod model;
mod modbus;
mod provisioning;
mod telemetry;

use config::SAMPLE_PERIOD;
use model::Measurement;
use telemetry::{send_pending_measurements, MeasurementQueue};
#[cfg(not(feature = "mains-powered"))]
use telemetry::enter_deep_sleep;

const MODBUS_BAUD: u32 = 4_800;
const WEATHER_ADDRESS: u8 = 1;

fn main() -> Result<()> {
    esp_idf_sys::link_patches();
    EspLogger::initialize_default();
    let peripherals = Peripherals::take().context("ESP32 peripherals already taken")?;
    info!("RS-FSXCS-N01 Wi-Fi cycle starting; period={} s", SAMPLE_PERIOD.as_secs());

    let nvs = EspDefaultNvsPartition::take().context("take default NVS partition")?;
    let mut wifi_store = provisioning::WifiStore::open(nvs.clone())?;
    wifi_store.apply_embedded_profile()?;
    let wifi_credentials = match wifi_store.load()? {
        Some(value) => value,
        None => {
            info!("Wi-Fi is not configured; starting setup access point");
            return provisioning::run(peripherals.modem, nvs, &mut wifi_store);
        }
    };

    let mut direction = PinDriver::output(AnyOutputPin::from(peripherals.pins.gpio13))
        .context("configure GPIO13 RS-485 direction")?;
    direction.set_low().context("set RS-485 receive mode")?;
    let config = UartConfig::new().baudrate(MODBUS_BAUD.Hz())
        .data_bits(DataBits::DataBits8).stop_bits(StopBits::STOP1).parity_none();
    let uart = UartDriver::new(peripherals.uart2, peripherals.pins.gpio5, peripherals.pins.gpio6,
        Option::<esp_idf_hal::gpio::AnyIOPin>::None,
        Option::<esp_idf_hal::gpio::AnyIOPin>::None, &config)
        .context("configure UART2 for Modbus RTU")?;
    let mut modem = peripherals.modem;
    let mut queue = MeasurementQueue::open(nvs.clone())?;
    FreeRtos::delay_ms(250);

    info!("mains-powered mode={}", cfg!(feature = "mains-powered"));
    #[cfg(feature = "mains-powered")]
    {
        info!("waiting 30 seconds for station startup");
        std::thread::sleep(std::time::Duration::from_secs(30));
    }
    loop {
        #[cfg(feature = "mains-powered")]
        let cycle_started = std::time::Instant::now();
        #[cfg(feature = "mains-powered")]
        let mut measurement_failed = false;

        match measure_cycle(&uart, &mut direction) {
            Ok(measurement) => {
                info!("snapshot: {measurement:?}");
                if let Err(err) = queue.enqueue(measurement) {
                    error!("could not persist measurement: {err:#}");
                }
            }
            Err(err) => {
                error!("measurement failed: {err:#}");
                #[cfg(feature = "mains-powered")]
                { measurement_failed = true; }
                #[cfg(not(feature = "mains-powered"))]
                enter_deep_sleep(SAMPLE_PERIOD);
            }
        }
        info!("queued records={}", queue.len());
        if queue.len() > 0 {
            if let Err(err) = send_pending_measurements(&mut queue, &mut modem, nvs.clone(), &wifi_credentials, &mut wifi_store) {
                error!("telemetry failed; queued data retained: {err:#}");
            }
        }

        #[cfg(not(feature = "mains-powered"))]
        enter_deep_sleep(SAMPLE_PERIOD);

        #[cfg(feature = "mains-powered")]
        {
            if measurement_failed {
                info!("measurement failed; retrying in 30 seconds");
                std::thread::sleep(std::time::Duration::from_secs(30));
                continue;
            }
            // Count the interval from the start of the cycle using a monotonic clock.
            // If an upload overruns it, skip missed slots instead of polling in bursts.
            let elapsed = cycle_started.elapsed();
            let remaining = SAMPLE_PERIOD - std::time::Duration::from_nanos(
                (elapsed.as_nanos() % SAMPLE_PERIOD.as_nanos()) as u64,
            );
            info!("next measurement in {} seconds; ESP32 remains awake", remaining.as_secs());
            std::thread::sleep(remaining);
        }
    }
}

fn measure_cycle(
    uart: &UartDriver<'_>,
    direction: &mut PinDriver<'_, AnyOutputPin, Output>,
) -> Result<Measurement> {
    let raw = modbus::read_measurements(uart, direction)
        .with_context(|| format!("read weather station at Modbus address {WEATHER_ADDRESS}"))?;
    info!("Modbus address {WEATHER_ADDRESS}: {raw}");
    direction.set_low().context("leave RS-485 in receive mode")?;
    Ok(Measurement {
        wind_speed_ms: Some(raw.wind_speed_tenths_ms as f32 / 10.0),
        wind_force: Some(raw.wind_force as f32),
        wind_direction_sector: Some(raw.wind_direction_sector as f32),
        wind_direction_deg: Some(raw.wind_direction_degrees as f32),
        humidity_pct: Some(raw.humidity_tenths_percent as f32 / 10.0),
        temperature_c: Some(raw.temperature_tenths_c as f32 / 10.0),
        pressure_kpa: Some(raw.pressure_tenths_kpa as f32 / 10.0),
        illuminance_lux: Some(raw.illuminance_lux as f32),
        illuminance_hundreds_lux: Some(raw.illuminance_hundreds_lux as f32),
        rainfall_mm: Some(raw.rainfall_tenths_mm as f32 / 10.0),
    })
}
