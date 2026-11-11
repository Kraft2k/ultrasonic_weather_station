#![no_std]
#![no_main]

mod bus;
mod weather;

use embedded_hal::delay::DelayNs;
use esp_backtrace as _;
use esp_hal::{
    delay::Delay,
    gpio::{Level, Output, OutputConfig},
    main,
    uart::{Config as UartConfig, Uart},
};
use esp_println::println;

esp_bootloader_esp_idf::esp_app_desc!();

const MODBUS_BAUD: u32 = 4_800;
const WEATHER_ADDRESS: u8 = 1;
const POLL_INTERVAL_MS: u32 = 5_000;

#[main]
fn main() -> ! {
    let peripherals = esp_hal::init(esp_hal::Config::default());
    let mut delay = Delay::new();
    let rs485_rts = Output::new(peripherals.GPIO13, Level::Low, OutputConfig::default());
    let uart = Uart::new(
        peripherals.UART2,
        UartConfig::default().with_baudrate(MODBUS_BAUD),
    )
    .expect("invalid weather UART configuration")
    .with_rx(peripherals.GPIO6)
    .with_tx(peripherals.GPIO5);
    let mut bus = bus::Rs485Bus::new(uart, rs485_rts);

    println!(
        "RS-FSXCS-N01-3HP-EX: Modbus RTU address {}, {} baud",
        WEATHER_ADDRESS, MODBUS_BAUD
    );
    loop {
        match bus.read_weather(&mut delay, WEATHER_ADDRESS) {
            Ok(measurements) => println!("{}", measurements),
            Err(error) => println!("weather read error: {:?}", error),
        }
        delay.delay_ms(POLL_INTERVAL_MS);
    }
}
