use embedded_hal::delay::DelayNs;
use esp_hal::{
    Blocking,
    gpio::Output,
    uart::{RxError, TxError, Uart},
};

use crate::weather::{self, FrameError, Measurements, RESPONSE_LEN};

const RESPONSE_TIMEOUT_MS: u32 = 400;
const RETRIES: usize = 3;

#[allow(dead_code)]
#[derive(Debug)]
pub enum Error {
    Tx(TxError),
    Rx(RxError),
    Timeout,
    Frame(FrameError),
}

pub struct Rs485Bus<'d> {
    uart: Uart<'d, Blocking>,
    direction: Output<'d>,
}

impl<'d> Rs485Bus<'d> {
    pub fn new(uart: Uart<'d, Blocking>, mut direction: Output<'d>) -> Self {
        // SparkFun BOB-10124: DE and /RE share RTS. Low = receive.
        direction.set_low();
        Self { uart, direction }
    }

    pub fn read_weather<D: DelayNs>(
        &mut self,
        delay: &mut D,
        address: u8,
    ) -> Result<Measurements, Error> {
        let mut result = Err(Error::Timeout);
        for _ in 0..RETRIES {
            // Manual V1.8 recommends at least 200 ms between polls.
            delay.delay_ms(250);
            result = self.read_once(delay, address);
            if result.is_ok() {
                break;
            }
        }
        result
    }

    fn read_once<D: DelayNs>(&mut self, delay: &mut D, address: u8) -> Result<Measurements, Error> {
        // Remove partial replies from a previous timed-out exchange.
        let mut stale = [0u8; 1];
        for _ in 0..64 {
            if !self.uart.read_ready() {
                break;
            }
            self.uart.read(&mut stale).map_err(Error::Rx)?;
        }

        let request = weather::request(address);
        self.direction.set_high();
        delay.delay_us(100);
        let tx = write_all(&mut self.uart, &request).and_then(|_| self.uart.flush());
        self.direction.set_low();
        tx.map_err(Error::Tx)?;

        let mut response = [0u8; RESPONSE_LEN];
        let mut remaining_ms = RESPONSE_TIMEOUT_MS;
        read_exact(&mut self.uart, delay, &mut response[..3], &mut remaining_ms)?;
        let length = if response[1] & 0x80 != 0 {
            5
        } else {
            RESPONSE_LEN
        };
        read_exact(
            &mut self.uart,
            delay,
            &mut response[3..length],
            &mut remaining_ms,
        )?;
        weather::parse(address, &response[..length]).map_err(Error::Frame)
    }
}

fn write_all(uart: &mut Uart<'_, Blocking>, mut bytes: &[u8]) -> Result<(), TxError> {
    while !bytes.is_empty() {
        let written = uart.write(bytes)?;
        bytes = &bytes[written..];
    }
    Ok(())
}

fn read_exact<D: DelayNs>(
    uart: &mut Uart<'_, Blocking>,
    delay: &mut D,
    bytes: &mut [u8],
    remaining_ms: &mut u32,
) -> Result<(), Error> {
    for slot in bytes {
        while !uart.read_ready() {
            if *remaining_ms == 0 {
                return Err(Error::Timeout);
            }
            delay.delay_ms(1);
            *remaining_ms -= 1;
        }
        uart.read(core::slice::from_mut(slot)).map_err(Error::Rx)?;
    }
    Ok(())
}
