use std::time::Duration;

/// Normal interval between measurements and telemetry uploads.
pub const SAMPLE_PERIOD: Duration = Duration::from_secs(20 * 60);

pub const MQTT_URL: &str = "mqtts://mqtt.185-139-70-32.sslip.io:8883";
pub const MQTT_TOPIC_PREFIX: &str = match option_env!("MQTT_TOPIC_PREFIX") {
    Some(v) => v,
    None => "",
};
pub const MQTT_CLIENT_ID: &str = match option_env!("MQTT_CLIENT_ID") {
    Some(v) => v,
    None => "",
};
pub const MQTT_PUBLISH_ATTEMPTS: u8 = 3;
pub const MQTT_PUBACK_TIMEOUT_MS: u32 = 15_000;
pub const MQTT_CONNECT_TIMEOUT_MS: u32 = 90_000;
pub const MQTT_RECONNECT_TIMEOUT_MS: u32 = 45_000;
/// lwIP allows 15 s per SNTP server before trying the next one.
pub const SNTP_INITIAL_SYNC_TIMEOUT_MS: u32 = 50_000;
pub const SNTP_RESYNC_TIMEOUT_MS: u32 = 5_000;
pub const MEASUREMENT_QUEUE_CAPACITY: usize = 64;
pub const MEASUREMENT_FIELD_COUNT: usize = 10;
