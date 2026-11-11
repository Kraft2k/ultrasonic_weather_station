use anyhow::{anyhow, bail, Context, Result};
use esp_idf_hal::delay::FreeRtos;
use esp_idf_svc::eventloop::EspSystemEventLoop;
use esp_idf_svc::handle::RawHandle;
use esp_idf_svc::mqtt::client::{EspMqttClient, EventPayload, MqttClientConfiguration, QoS};
use esp_idf_svc::nvs::{EspDefaultNvs, EspDefaultNvsPartition, EspNvs};
use esp_idf_svc::sntp::{EspSntp, SntpConf, SyncStatus};
use esp_idf_svc::tls::X509;
use esp_idf_svc::wifi::{AuthMethod, BlockingWifi, ClientConfiguration, Configuration, EspWifi};
use log::{info, warn};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use crate::config::*;
use crate::model::Measurement;
use crate::provisioning;

// Let's Encrypt's current YE2 chain is not present in older ESP-IDF bundles.
// Keep certificate verification enabled with the public Root YE trust anchor.
const ROOT_YE_CERT: X509<'static> =
    X509::pem_until_nul(concat!(include_str!("../certs/root_ye.pem"), "\0").as_bytes());

#[derive(Debug, Clone, Copy)]
struct QueuedMeasurement {
    measurement: Measurement,
    /// Исходное время измерения, Unix time UTC в секундах.
    timestamp: i64,
    /// Биты метрик, для которых ещё не получен MQTT PUBACK.
    pending_mask: u16,
}

pub struct MeasurementQueue {
    nvs: EspDefaultNvs,
    records: Vec<QueuedMeasurement>,
}

impl MeasurementQueue {
    pub fn len(&self) -> usize {
        self.records.len()
    }

    const NAMESPACE: &'static str = "weather_queue";
    const KEY: &'static str = "pending";
    const MAGIC: [u8; 4] = *b"WTH2";
    const LEGACY_MAGIC: [u8; 4] = *b"WTH1";

    pub fn open(partition: EspDefaultNvsPartition) -> Result<Self> {
        let nvs = EspNvs::new(partition, Self::NAMESPACE, true).context("open NVS queue")?;
        let mut queue = Self {
            nvs,
            records: Vec::new(),
        };
        if let Err(err) = queue.load() {
            warn!("stored measurement queue is invalid; starting empty: {err:#}");
            queue.records.clear();
            queue.save()?;
        }
        Ok(queue)
    }

    fn load(&mut self) -> Result<()> {
        let Some(length) = self
            .nvs
            .blob_len(Self::KEY)
            .context("read NVS queue length")?
        else {
            return Ok(());
        };
        let maximum = 5 + MEASUREMENT_QUEUE_CAPACITY * (12 + 4 * MEASUREMENT_FIELD_COUNT);
        if length > maximum {
            bail!("NVS queue is too large: {length} bytes");
        }
        let mut bytes = vec![0_u8; length];
        let bytes = self
            .nvs
            .get_blob(Self::KEY, &mut bytes)
            .context("read NVS queue")?
            .ok_or_else(|| anyhow!("NVS queue disappeared while reading"))?;
        if bytes.len() < 5 || (bytes[..4] != Self::MAGIC && bytes[..4] != Self::LEGACY_MAGIC) {
            bail!("NVS queue header mismatch");
        }
        let legacy = bytes[..4] == Self::LEGACY_MAGIC;
        let count = usize::from(bytes[4]);
        let field_count = MEASUREMENT_FIELD_COUNT;
        let record_size = (if legacy { 10 } else { 12 }) + 4 * field_count;
        if count > MEASUREMENT_QUEUE_CAPACITY || bytes.len() != 5 + count * record_size {
            bail!("NVS queue size mismatch");
        }

        self.records.clear();
        let mut offset = 5;
        for _ in 0..count {
            let (pending_mask, valid_mask) = if legacy {
                // WTH1 used one byte for ten metrics. The firmware always recorded
                // both trailing sensor values, but could never publish them.
                let pending = u16::from(bytes[offset]) | (0b11 << 8);
                let valid = u16::from(bytes[offset + 1]) | (0b11 << 8);
                offset += 2;
                (pending, valid)
            } else {
                let pending = u16::from_le_bytes(bytes[offset..offset + 2].try_into().unwrap());
                let valid = u16::from_le_bytes(bytes[offset + 2..offset + 4].try_into().unwrap());
                offset += 4;
                (pending, valid)
            };
            let timestamp = i64::from_le_bytes(bytes[offset..offset + 8].try_into().unwrap());
            offset += 8;
            let mut stored_values = [None; MEASUREMENT_FIELD_COUNT];
            for (index, value) in stored_values.iter_mut().enumerate().take(field_count) {
                let raw = f32::from_le_bytes(bytes[offset..offset + 4].try_into().unwrap());
                offset += 4;
                if valid_mask & (1_u16 << index) != 0 {
                    *value = Some(raw);
                }
            }
            let values = stored_values;
            let (measurement, pending_mask) =
                (Measurement::from_values(values), pending_mask & valid_mask);
            self.records.push(QueuedMeasurement {
                measurement,
                timestamp,
                pending_mask,
            });
        }
        if legacy {
            info!("upgrading stored WTH1 queue; restoring pending light and rainfall metrics");
            self.save()?;
        }
        Ok(())
    }

    fn save(&mut self) -> Result<()> {
        let mut bytes =
            Vec::with_capacity(5 + self.records.len() * (12 + 4 * MEASUREMENT_FIELD_COUNT));
        bytes.extend_from_slice(&Self::MAGIC);
        bytes.push(self.records.len() as u8);
        for record in &self.records {
            let values = record.measurement.values();
            bytes.extend_from_slice(&record.pending_mask.to_le_bytes());
            bytes.extend_from_slice(&record.measurement.valid_mask().to_le_bytes());
            bytes.extend_from_slice(&record.timestamp.to_le_bytes());
            for value in values {
                bytes.extend_from_slice(&value.unwrap_or(0.0).to_le_bytes());
            }
        }
        self.nvs
            .set_blob(Self::KEY, &bytes)
            .context("commit measurement queue to NVS")
    }

    pub fn enqueue(&mut self, measurement: Measurement) -> Result<()> {
        let pending_mask = measurement.valid_mask();
        if pending_mask == 0 {
            bail!("measurement contains no valid sensor values");
        }
        if self.records.len() == MEASUREMENT_QUEUE_CAPACITY {
            warn!("measurement queue full; dropping oldest record");
            self.records.remove(0);
        }
        self.records.push(QueuedMeasurement {
            measurement,
            timestamp: current_unix_timestamp().unwrap_or(0),
            pending_mask,
        });
        self.save()
    }

    fn assign_missing_timestamps(&mut self, now: i64) -> Result<()> {
        let missing = self
            .records
            .iter()
            .filter(|record| record.timestamp == 0)
            .count();
        if missing == 0 {
            return Ok(());
        }
        // Такое возможно только для старого WMQ1 или первого запуска без уже
        // синхронизированных часов. Точное время при полном обесточивании без RTC
        // восстановить нельзя; сохраняем ближайшую доступную оценку и явно логируем её.
        let record_count = self.records.len();
        for (index, record) in self.records.iter_mut().enumerate() {
            if record.timestamp == 0 {
                let age = (record_count - 1 - index) as i64 * SAMPLE_PERIOD.as_secs() as i64;
                record.timestamp = now.saturating_sub(age);
            }
        }
        warn!("assigned estimated timestamps to {missing} legacy/unsynchronized record(s)");
        self.save()
    }
}

/// Connect for one upload cycle; release network resources before returning.
pub fn send_pending_measurements(
    queue: &mut MeasurementQueue,
    modem: &mut esp_idf_hal::modem::Modem,
    nvs_partition: EspDefaultNvsPartition,
    wifi_credentials: &provisioning::WifiCredentials,
    wifi_store: &mut provisioning::WifiStore,
) -> Result<()> {
    let mqtt_username = option_env!("MQTT_USERNAME").unwrap_or("");
    let mqtt_password = option_env!("MQTT_PASSWORD").unwrap_or("");
    if MQTT_TOPIC_PREFIX.is_empty()
        || MQTT_CLIENT_ID.is_empty()
        || mqtt_username.is_empty()
        || mqtt_password.is_empty()
    {
        bail!("device-specific MQTT credentials are missing; set MQTT_USERNAME, MQTT_PASSWORD, MQTT_TOPIC_PREFIX and MQTT_CLIENT_ID in firmware_wifi/device.env");
    }

    let sys_loop = EspSystemEventLoop::take().context("take system event loop")?;
    let mut esp_wifi =
        EspWifi::new(modem, sys_loop.clone(), Some(nvs_partition)).context("initialize Wi-Fi")?;
    let mut wifi =
        BlockingWifi::wrap(&mut esp_wifi, sys_loop).context("initialize blocking Wi-Fi")?;
    let preferred = if wifi_credentials.alternate.is_some() {
        wifi_store.preferred_network()?
    } else {
        0
    };
    let mut connected_network = None;
    let mut last_error = None;
    for index in [preferred, 1 - preferred] {
        let Some((ssid, password)) = wifi_credentials.network(index) else { continue };
        info!("trying Wi-Fi profile {}", index + 1);
        let attempt = (|| -> Result<()> {
            wifi.set_configuration(&Configuration::Client(ClientConfiguration {
                ssid: ssid.try_into().map_err(|_| anyhow!("Wi-Fi SSID is too long"))?,
                password: password.try_into().map_err(|_| anyhow!("Wi-Fi password is too long"))?,
                auth_method: if password.is_empty() { AuthMethod::None } else { AuthMethod::WPA2Personal },
                ..Default::default()
            }))?;
            wifi.start()?;
            wifi.connect()?;
            wifi.wait_netif_up()?;
            Ok(())
        })();
        match attempt {
            Ok(()) => { connected_network = Some(index); break; }
            Err(err) => {
                warn!("Wi-Fi profile {} unavailable: {err:#}", index + 1);
                last_error = Some(err);
                wifi.stop().ok();
            }
        }
    }
    let connected_network = connected_network.ok_or_else(|| {
        last_error.unwrap_or_else(|| anyhow!("no Wi-Fi profile configured"))
            .context("all configured Wi-Fi profiles failed")
    })?;
    let (latitude, longitude) = wifi_credentials.coordinates(connected_network)
        .ok_or_else(|| anyhow!("connected Wi-Fi profile has no coordinates"))?;
    if connected_network != preferred {
        if let Err(err) = wifi_store.set_preferred_network(connected_network) {
            warn!("could not remember working Wi-Fi profile: {err:#}");
        }
    }
    // Keep the radio fully awake while SNTP and MQTT are active.
    // Modem sleep here can stall short TCP sessions.
    esp_idf_sys::esp!(unsafe {
        esp_idf_sys::esp_wifi_set_ps(esp_idf_sys::wifi_ps_type_t_WIFI_PS_NONE)
    })
    .context("disable Wi-Fi modem sleep")?;
    let ip_info = wifi
        .wifi()
        .sta_netif()
        .get_ip_info()
        .context("read IP info")?;
    info!(
        "Wi-Fi connected to {}; IP={}",
        wifi_credentials.network(connected_network).unwrap().0, ip_info.ip
    );

    synchronize_clock()?;
    let now = current_unix_timestamp().ok_or_else(|| anyhow!("system clock is not valid"))?;
    queue.assign_missing_timestamps(now)?;

    let connected = Arc::new(AtomicBool::new(false));
    let callback_connected = connected.clone();
    let acknowledged = Arc::new(Mutex::new(Vec::new()));
    let callback_acknowledged = acknowledged.clone();
    let mut mqtt = EspMqttClient::new_cb(
        MQTT_URL,
        &MqttClientConfiguration {
            client_id: Some(MQTT_CLIENT_ID),
            username: Some(mqtt_username),
            password: Some(mqtt_password),
            crt_bundle_attach: None,
            server_certificate: Some(ROOT_YE_CERT),
            keep_alive_interval: Some(Duration::from_secs(30)),
            network_timeout: Duration::from_secs(15),
            reconnect_timeout: Some(Duration::from_secs(2)),
            ..Default::default()
        },
        move |event| match event.payload() {
            EventPayload::Connected(_) => {
                callback_connected.store(true, Ordering::Release);
                info!("MQTTS connected");
            }
            EventPayload::Disconnected => {
                callback_connected.store(false, Ordering::Release);
                warn!("MQTTS disconnected");
            }
            EventPayload::Error(err) => warn!("MQTTS error: {err:?}"),
            EventPayload::Published(message_id) => {
                if let Ok(mut acknowledgements) = callback_acknowledged.lock() {
                    if acknowledgements.len() == 16 {
                        acknowledgements.remove(0);
                    }
                    acknowledgements.push(message_id);
                }
            }
            _ => {}
        },
    )
    .context("create MQTTS client")?;

    for _ in 0..(MQTT_CONNECT_TIMEOUT_MS / 100) {
        if connected.load(Ordering::Acquire) {
            break;
        }
        FreeRtos::delay_ms(100);
    }
    if !connected.load(Ordering::Acquire) {
        bail!("MQTTS connection timeout");
    }

    info!("sending {} queued measurement(s)", queue.records.len());
    while let Some(mut record) = queue.records.first().copied() {
        let values = record.measurement.values();
        for (index, value) in values.into_iter().enumerate() {
            let bit = 1_u16 << index;
            if record.pending_mask & bit == 0 {
                continue;
            }
            let value = value.ok_or_else(|| anyhow!("queued metric bit {index} has no value"))?;
            let (metric, unit) = metric_metadata(index);
            if let Err(err) = publish_with_confirmation(
                &mut mqtt,
                &connected,
                &acknowledged,
                metric,
                value,
                unit,
                record.timestamp,
                latitude,
                longitude,
            ) {
                queue.records[0] = record;
                queue.save()?;
                return Err(err);
            }
            record.pending_mask &= !bit;
            queue.records[0] = record;
        }
        queue.records.remove(0);
        queue.save()?;
        info!(
            "queued measurement fully acknowledged; remaining={}",
            queue.records.len()
        );
    }
    info!("all stored measurements delivered with QoS 1 PUBACK");
    Ok(())
}

fn current_unix_timestamp() -> Option<i64> {
    let seconds = SystemTime::now().duration_since(UNIX_EPOCH).ok()?.as_secs();
    // Отсекаем несинхронизированную эпоху 1970 года.
    (seconds >= 1_704_067_200).then_some(seconds as i64) // 2024-01-01T00:00:00Z
}

fn synchronize_clock() -> Result<()> {
    let clock_was_valid = current_unix_timestamp().is_some();
    let sntp_conf = SntpConf {
        servers: ["time.cloudflare.com", "0.pool.ntp.org", "1.pool.ntp.org"],
        ..Default::default()
    };
    let sntp = EspSntp::new(&sntp_conf).context("start SNTP")?;
    let timeout_ms = if clock_was_valid {
        SNTP_RESYNC_TIMEOUT_MS
    } else {
        SNTP_INITIAL_SYNC_TIMEOUT_MS
    };
    info!(
        "SNTP: waiting up to {} s for {:?}",
        timeout_ms / 1_000,
        sntp_conf.servers
    );
    for _ in 0..(timeout_ms / 100) {
        if sntp.get_sync_status() == SyncStatus::Completed
            || (!clock_was_valid && current_unix_timestamp().is_some())
        {
            let timestamp = current_unix_timestamp()
                .ok_or_else(|| anyhow!("SNTP completed but system time is invalid"))?;
            info!("SNTP synchronized: {}", format_iso8601(timestamp).unwrap());
            return Ok(());
        }
        FreeRtos::delay_ms(100);
    }
    if clock_was_valid {
        warn!(
            "SNTP timeout after {} s; continuing with the valid system clock",
            timeout_ms / 1_000
        );
        Ok(())
    } else {
        bail!("SNTP timeout after {} s and no valid retained clock is available; check DNS and UDP port 123", timeout_ms / 1_000)
    }
}

fn format_iso8601(timestamp: i64) -> Option<String> {
    if timestamp < 0 {
        return None;
    }
    let days = timestamp.div_euclid(86_400);
    let seconds = timestamp.rem_euclid(86_400);
    // Gregorian conversion based on 400-year eras; valid without timezone tables because UTC.
    let z = days + 719_468;
    let era = z.div_euclid(146_097);
    let day_of_era = z - era * 146_097;
    let year_of_era =
        (day_of_era - day_of_era / 1_460 + day_of_era / 36_524 - day_of_era / 146_096) / 365;
    let mut year = year_of_era + era * 400;
    let day_of_year = day_of_era - (365 * year_of_era + year_of_era / 4 - year_of_era / 100);
    let month_prime = (5 * day_of_year + 2) / 153;
    let day = day_of_year - (153 * month_prime + 2) / 5 + 1;
    let month = month_prime + if month_prime < 10 { 3 } else { -9 };
    if month <= 2 {
        year += 1;
    }
    let hour = seconds / 3_600;
    let minute = seconds % 3_600 / 60;
    let second = seconds % 60;
    Some(format!(
        "{year:04}-{month:02}-{day:02}T{hour:02}:{minute:02}:{second:02}Z"
    ))
}

fn metric_metadata(index: usize) -> (&'static str, &'static str) {
    match index {
        0 => ("wind_speed_ms", "m/s"),
        1 => ("wind_force", "level"),
        2 => ("wind_direction_sector", "sector"),
        3 => ("wind_direction_deg", "deg"),
        4 => ("relative_humidity_pct", "%"),
        5 => ("air_temperature_c", "°C"),
        6 => ("air_pressure_kpa", "kPa"),
        7 => ("illuminance_lux", "lux"),
        8 => ("illuminance_hundreds_lux", "100 lux"),
        9 => ("rainfall_mm", "mm"),
        _ => unreachable!(),
    }
}

fn publish_with_confirmation(
    mqtt: &mut EspMqttClient<'_>,
    connected: &AtomicBool,
    acknowledged: &Mutex<Vec<u32>>,
    metric: &str,
    value: f32,
    unit: &str,
    timestamp: i64,
    latitude: f64,
    longitude: f64,
) -> Result<()> {
    let topic = format!("{MQTT_TOPIC_PREFIX}/{metric}");
    let ts = format_iso8601(timestamp).ok_or_else(|| anyhow!("invalid timestamp {timestamp}"))?;
    let payload = format!(
        "{{\"value\":{value:.4},\"unit\":\"{unit}\",\"ts\":\"{ts}\",\"lat\":{latitude:.7},\"lon\":{longitude:.7}}}"
    );
    let mut last_error = None;

    for attempt in 1..=MQTT_PUBLISH_ATTEMPTS {
        if !connected.load(Ordering::Acquire) {
            last_error = Some(anyhow!("MQTTS disconnected"));
        } else {
            match mqtt.publish(&topic, QoS::AtLeastOnce, false, payload.as_bytes()) {
                Ok(message_id) => {
                    let polls = MQTT_PUBACK_TIMEOUT_MS / 100;
                    for _ in 0..polls {
                        let was_acknowledged = {
                            let mut acknowledgements = acknowledged
                                .lock()
                                .map_err(|_| anyhow!("MQTT PUBACK state lock poisoned"))?;
                            if let Some(index) = acknowledgements
                                .iter()
                                .position(|acknowledged_id| *acknowledged_id == message_id)
                            {
                                acknowledgements.remove(index);
                                true
                            } else {
                                false
                            }
                        };
                        if was_acknowledged {
                            info!("MQTT PUBACK: {metric}={value:.4} {unit}, ts={ts} (attempt {attempt})");
                            return Ok(());
                        }
                        if !connected.load(Ordering::Acquire) {
                            break;
                        }
                        FreeRtos::delay_ms(100);
                    }
                    last_error = Some(anyhow!(
                        "PUBACK timeout for {metric}, message id {message_id}"
                    ));
                }
                Err(err) => {
                    last_error = Some(anyhow!(err).context(format!("publish {metric}")));
                }
            }
        }

        warn!(
            "MQTT publish failed for {metric} (attempt {attempt}/{MQTT_PUBLISH_ATTEMPTS}): {:#}",
            last_error.as_ref().unwrap()
        );
        if attempt < MQTT_PUBLISH_ATTEMPTS {
            reconnect_mqtt(mqtt, connected)
                .with_context(|| format!("restore MQTTS connection before retrying {metric}"))?;
        }
    }

    Err(last_error.unwrap_or_else(|| anyhow!("unknown MQTT publish failure")))
}

fn reconnect_mqtt(mqtt: &EspMqttClient<'_>, connected: &AtomicBool) -> Result<()> {
    connected.store(false, Ordering::Release);
    esp_idf_sys::esp!(unsafe { esp_idf_sys::esp_mqtt_client_reconnect(mqtt.handle()) })
        .context("request MQTTS reconnect")?;

    for _ in 0..(MQTT_RECONNECT_TIMEOUT_MS / 100) {
        if connected.load(Ordering::Acquire) {
            info!("MQTTS connection restored");
            return Ok(());
        }
        FreeRtos::delay_ms(100);
    }

    bail!("MQTTS reconnect timeout")
}

#[cfg(not(feature = "mains-powered"))]
pub fn enter_deep_sleep(period: Duration) -> ! {
    let micros = period.as_micros() as u64;
    info!("entering deep sleep for {} seconds", period.as_secs());
    unsafe { esp_idf_sys::esp_deep_sleep(micros) }
}
