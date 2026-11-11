use anyhow::{anyhow, Result};
use embedded_svc::{
    http::{Headers, Method},
    io::{Read, Write},
};
use esp_idf_hal::{delay::FreeRtos, modem::Modem};
use esp_idf_svc::{
    eventloop::EspSystemEventLoop,
    http::server::{Configuration as HttpConfiguration, EspHttpServer},
    nvs::{EspDefaultNvs, EspDefaultNvsPartition, EspNvs},
    wifi::{
        AccessPointConfiguration, AuthMethod, BlockingWifi, ClientConfiguration, Configuration,
        EspWifi,
    },
};
use log::{info, warn};
use std::sync::{Arc, Mutex};

const PAGE: &str = include_str!("provisioning.html");
const AP_SSID: &str = "Weather-Setup";
const AP_PASSWORD: &str = "weather-setup";

#[derive(Clone, Debug)]
pub struct WifiCredentials {
    pub ssid: String,
    pub password: String,
    pub alternate: Option<WifiNetwork>,
    pub latitude: f64,
    pub longitude: f64,
}

#[derive(Clone, Debug)]
pub struct WifiNetwork {
    pub ssid: String,
    pub password: String,
    pub latitude: f64,
    pub longitude: f64,
}

impl WifiCredentials {
    pub fn network(&self, index: u8) -> Option<(&str, &str)> {
        match index {
            0 => Some((&self.ssid, &self.password)),
            1 => self.alternate.as_ref().map(|network| (network.ssid.as_str(), network.password.as_str())),
            _ => None,
        }
    }
    pub fn coordinates(&self, index: u8) -> Option<(f64, f64)> {
        match index {
            0 => Some((self.latitude, self.longitude)),
            1 => self.alternate.as_ref().map(|network| (network.latitude, network.longitude)),
            _ => None,
        }
    }
}

pub struct WifiStore {
    nvs: EspDefaultNvs,
}
impl WifiStore {
    pub fn open(partition: EspDefaultNvsPartition) -> Result<Self> {
        Ok(Self {
            nvs: EspNvs::new(partition, "wifi_config", true)?,
        })
    }
    pub fn load(&self) -> Result<Option<WifiCredentials>> {
        let mut ssid = [0u8; 33];
        let mut password = [0u8; 65];
        let mut ssid2 = [0u8; 33];
        let mut password2 = [0u8; 65];
        let mut latitude = [0u8; 25];
        let mut longitude = [0u8; 25];
        let mut latitude2 = [0u8; 25];
        let mut longitude2 = [0u8; 25];
        let Some(ssid) = self.nvs.get_str("ssid", &mut ssid)? else {
            return Ok(None);
        };
        let password = self.nvs.get_str("password", &mut password)?.unwrap_or("");
        let Some(latitude) = self.nvs.get_str("latitude", &mut latitude)? else {
            return Ok(None);
        };
        let Some(longitude) = self.nvs.get_str("longitude", &mut longitude)? else {
            return Ok(None);
        };
        let latitude = parse_coordinate(latitude, "latitude", -90.0, 90.0)?;
        let longitude = parse_coordinate(longitude, "longitude", -180.0, 180.0)?;
        let alternate = match self.nvs.get_str("ssid2", &mut ssid2)? {
            Some(ssid) if !ssid.is_empty() => {
                let stored_latitude = self.nvs.get_str("latitude2", &mut latitude2)?;
                let stored_longitude = self.nvs.get_str("longitude2", &mut longitude2)?;
                match (stored_latitude, stored_longitude) {
                    (Some(latitude2), Some(longitude2)) => Some(WifiNetwork {
                        ssid: ssid.into(),
                        password: self.nvs.get_str("password2", &mut password2)?.unwrap_or("").into(),
                        latitude: parse_coordinate(latitude2, "latitude2", -90.0, 90.0)?,
                        longitude: parse_coordinate(longitude2, "longitude2", -180.0, 180.0)?,
                    }),
                    _ => {
                        warn!("second Wi-Fi profile has no coordinates; update WIFI_PROFILE_ID to install its location");
                        None
                    }
                }
            }
            _ => None,
        };
        Ok(Some(WifiCredentials {
            ssid: ssid.into(),
            password: password.into(),
            alternate,
            latitude,
            longitude,
        }))
    }
    /// Installs a profile embedded at build time once. The marker remains in
    /// NVS, so a profile later selected in the setup portal is not overwritten
    /// on every boot.
    pub fn apply_embedded_profile(&mut self) -> Result<bool> {
        let Some(profile_id) = option_env!("WIFI_PROFILE_ID") else {
            return Ok(false);
        };
        let mut stored_id = [0_u8; 64];
        if self.nvs.get_str("profile_id", &mut stored_id)? == Some(profile_id) {
            return Ok(false);
        }

        let ssid = option_env!("WIFI_SSID").ok_or_else(|| anyhow!("WIFI_SSID is missing"))?;
        let password =
            option_env!("WIFI_PASSWORD").ok_or_else(|| anyhow!("WIFI_PASSWORD is missing"))?;
        if ssid.is_empty() || ssid.len() > 32 {
            return Err(anyhow!("embedded Wi-Fi SSID length is invalid"));
        }
        if !password.is_empty() && !(8..=63).contains(&password.len()) {
            return Err(anyhow!("embedded WPA2 password must contain 8..=63 bytes"));
        }
        let alternate = match option_env!("WIFI_SSID_2") {
            Some(ssid2) if !ssid2.is_empty() => {
                let password2 = option_env!("WIFI_PASSWORD_2")
                    .ok_or_else(|| anyhow!("WIFI_PASSWORD_2 is missing"))?;
                if ssid2.len() > 32 || (!password2.is_empty() && !(8..=63).contains(&password2.len())) {
                    return Err(anyhow!("embedded secondary Wi-Fi credentials are invalid"));
                }
                if ssid2 == ssid {
                    return Err(anyhow!("embedded Wi-Fi SSIDs must be distinct"));
                }
                Some(WifiNetwork {
                    ssid: ssid2.into(),
                    password: password2.into(),
                    latitude: parse_coordinate(
                        option_env!("WIFI_LATITUDE_2").ok_or_else(|| anyhow!("WIFI_LATITUDE_2 is missing"))?,
                        "latitude2", -90.0, 90.0,
                    )?,
                    longitude: parse_coordinate(
                        option_env!("WIFI_LONGITUDE_2").ok_or_else(|| anyhow!("WIFI_LONGITUDE_2 is missing"))?,
                        "longitude2", -180.0, 180.0,
                    )?,
                })
            }
            _ => None,
        };
        let credentials = WifiCredentials {
            ssid: ssid.into(),
            password: password.into(),
            alternate,
            latitude: parse_coordinate(
                option_env!("WIFI_LATITUDE").ok_or_else(|| anyhow!("WIFI_LATITUDE is missing"))?,
                "latitude",
                -90.0,
                90.0,
            )?,
            longitude: parse_coordinate(
                option_env!("WIFI_LONGITUDE")
                    .ok_or_else(|| anyhow!("WIFI_LONGITUDE is missing"))?,
                "longitude",
                -180.0,
                180.0,
            )?,
        };
        self.save(&credentials)?;
        self.nvs.set_str("profile_id", profile_id)?;
        info!("installed embedded Wi-Fi profile with {} network(s)", if credentials.alternate.is_some() { 2 } else { 1 });
        Ok(true)
    }
    pub fn preferred_network(&self) -> Result<u8> {
        Ok(self.nvs.get_u8("preferred")?.unwrap_or(0).min(1))
    }
    pub fn set_preferred_network(&mut self, index: u8) -> Result<()> {
        self.nvs.set_u8("preferred", index)?;
        Ok(())
    }
    fn save(&mut self, value: &WifiCredentials) -> Result<()> {
        self.nvs.set_str("ssid", &value.ssid)?;
        self.nvs.set_str("password", &value.password)?;
        if let Some(network) = &value.alternate {
            self.nvs.set_str("ssid2", &network.ssid)?;
            self.nvs.set_str("password2", &network.password)?;
            self.nvs.set_str("latitude2", &format!("{:.7}", network.latitude))?;
            self.nvs.set_str("longitude2", &format!("{:.7}", network.longitude))?;
        } else {
            self.nvs.remove("ssid2")?;
            self.nvs.remove("password2")?;
            self.nvs.remove("latitude2")?;
            self.nvs.remove("longitude2")?;
        }
        self.nvs.set_u8("preferred", 0)?;
        self.nvs
            .set_str("latitude", &format!("{:.7}", value.latitude))?;
        self.nvs
            .set_str("longitude", &format!("{:.7}", value.longitude))?;
        Ok(())
    }
}

pub fn run(modem: Modem, partition: EspDefaultNvsPartition, store: &mut WifiStore) -> Result<()> {
    // Entering setup must not discard the working profile. Only replace it
    // after the new network has supplied a DHCP address.
    let page = Arc::new(setup_page(store.load()?.as_ref()));
    let sysloop = EspSystemEventLoop::take()?;
    let mut wifi = BlockingWifi::wrap(
        EspWifi::new(modem, sysloop.clone(), Some(partition))?,
        sysloop,
    )?;
    loop {
        wifi.set_configuration(&Configuration::AccessPoint(AccessPointConfiguration {
            ssid: AP_SSID.try_into().unwrap(),
            password: AP_PASSWORD.try_into().unwrap(),
            auth_method: AuthMethod::WPA2Personal,
            channel: 1,
            max_connections: 4,
            ..Default::default()
        }))?;
        wifi.start()?;
        let ip = wifi.wifi().ap_netif().get_ip_info()?.ip;
        info!("provisioning: connect to {AP_SSID} and open http://{ip}");

        let candidate = Arc::new(Mutex::new(None::<WifiCredentials>));
        let posted = candidate.clone();
        let mut server = EspHttpServer::new(&HttpConfiguration::default())?;
        let root_page = page.clone();
        server.fn_handler("/", Method::Get, move |req| {
            req.into_response(
                200,
                Some("OK"),
                &[
                    ("Content-Type", "text/html; charset=utf-8"),
                    ("Cache-Control", "no-store, no-cache, must-revalidate"),
                ],
            )?
            .write_all(root_page.as_bytes())
            .map(|_| ())
        })?;
        server.fn_handler::<anyhow::Error, _>("/save", Method::Post, move |mut req| {
            info!("provisioning: POST /save received");
            let len = (req.content_len().unwrap_or(0) as usize).min(512);
            let mut body = vec![0; len];
            req.read_exact(&mut body)?;
            let text = std::str::from_utf8(&body)?;
            let ssid = form_value(text, "ssid").ok_or_else(|| anyhow!("SSID is missing"))?;
            let password = form_value(text, "password").unwrap_or_default();
            let latitude = parse_coordinate(
                &form_value(text, "latitude").ok_or_else(|| anyhow!("latitude is missing"))?,
                "latitude",
                -90.0,
                90.0,
            )?;
            let longitude = parse_coordinate(
                &form_value(text, "longitude").ok_or_else(|| anyhow!("longitude is missing"))?,
                "longitude",
                -180.0,
                180.0,
            )?;
            if ssid.is_empty() || ssid.len() > 32 || password.len() > 64 {
                return Err(anyhow!("invalid Wi-Fi credentials"));
            }
            info!("provisioning: received Wi-Fi settings for SSID '{ssid}'");
            let selected = WifiCredentials {
                ssid,
                password,
                alternate: None,
                latitude,
                longitude,
            };
            {
                let mut response = req.into_response(
                    200,
                    Some("OK"),
                    &[
                        ("Content-Type", "text/plain; charset=utf-8"),
                        ("Cache-Control", "no-store"),
                        ("Connection", "close"),
                    ],
                )?;
                response.write_all(b"Testing Wi-Fi...")?;
                response.flush()?;
            }
            // Signal the main loop only after the browser has received the full
            // response. Otherwise stopping the setup AP races the HTTP request.
            *posted.lock().unwrap() = Some(selected);
            Ok(())
        })?;
        // Typical Android, Apple and Windows captive-portal probes.
        for path in [
            "/generate_204",
            "/hotspot-detect.html",
            "/connecttest.txt",
            "/ncsi.txt",
        ] {
            let probe_page = page.clone();
            server.fn_handler(path, Method::Get, move |req| {
                req.into_response(
                    200,
                    Some("OK"),
                    &[
                        ("Content-Type", "text/html; charset=utf-8"),
                        ("Cache-Control", "no-store, no-cache, must-revalidate"),
                    ],
                )?
                .write_all(probe_page.as_bytes())
                .map(|_| ())
            })?;
        }
        let selected = loop {
            if let Some(value) = candidate.lock().unwrap().take() {
                break value;
            }
            FreeRtos::delay_ms(100);
        };
        FreeRtos::delay_ms(800);
        drop(server);
        wifi.stop()?;
        info!("provisioning: testing Wi-Fi '{}'", selected.ssid);
        wifi.set_configuration(&Configuration::Client(ClientConfiguration {
            ssid: selected
                .ssid
                .as_str()
                .try_into()
                .map_err(|_| anyhow!("SSID too long"))?,
            password: selected
                .password
                .as_str()
                .try_into()
                .map_err(|_| anyhow!("password too long"))?,
            auth_method: if selected.password.is_empty() {
                AuthMethod::None
            } else {
                AuthMethod::WPA2Personal
            },
            ..Default::default()
        }))?;
        wifi.start()?;
        let connected = wifi.connect().and_then(|_| wifi.wait_netif_up());
        if connected.is_ok() {
            let ip = wifi.wifi().sta_netif().get_ip_info()?.ip;
            let mut selected = selected;
            selected.alternate = store.load()?.and_then(|saved| saved.alternate);
            store.save(&selected)?;
            info!(
                "provisioning complete: '{}' IP={ip}; restarting",
                selected.ssid
            );
            FreeRtos::delay_ms(500);
            unsafe { esp_idf_sys::esp_restart() }
        }
        warn!(
            "provisioning: connection to '{}' failed; reopening setup AP",
            selected.ssid
        );
        wifi.stop().ok();
    }
}

fn setup_page(saved: Option<&WifiCredentials>) -> String {
    let Some(saved) = saved else {
        return PAGE.to_owned();
    };
    // The SSID is user supplied: escape it before inserting an HTML attribute.
    // Never send the stored Wi-Fi password to the browser.
    let ssid = saved
        .ssid
        .replace('&', "&amp;")
        .replace('"', "&quot;")
        .replace('<', "&lt;")
        .replace('>', "&gt;");
    PAGE.replace(
        "id=\"ssid\" name=\"ssid\"",
        &format!("id=\"ssid\" name=\"ssid\" value=\"{ssid}\""),
    )
    .replace(
        "placeholder=\"55,7558000\"",
        &format!("value=\"{:.7}\" placeholder=\"55,7558000\"", saved.latitude),
    )
    .replace(
        "placeholder=\"37,6176000\"",
        &format!(
            "value=\"{:.7}\" placeholder=\"37,6176000\"",
            saved.longitude
        ),
    )
}

fn form_value(body: &str, key: &str) -> Option<String> {
    body.split('&').find_map(|field| {
        let (name, value) = field.split_once('=')?;
        (name == key).then(|| url_decode(value))
    })
}

fn parse_coordinate(value: &str, name: &str, minimum: f64, maximum: f64) -> Result<f64> {
    let value = value
        .trim()
        .replace(',', ".")
        .parse::<f64>()
        .map_err(|_| anyhow!("invalid {name}"))?;
    if !value.is_finite() || !(minimum..=maximum).contains(&value) {
        return Err(anyhow!("{name} is outside {minimum}..={maximum}"));
    }
    Ok(value)
}
fn url_decode(value: &str) -> String {
    let bytes = value.as_bytes();
    let mut out = Vec::with_capacity(bytes.len());
    let mut i = 0;
    while i < bytes.len() {
        match bytes[i] {
            b'+' => {
                out.push(b' ');
                i += 1;
            }
            b'%' if i + 2 < bytes.len() => {
                if let Ok(v) = u8::from_str_radix(&value[i + 1..i + 3], 16) {
                    out.push(v);
                    i += 3;
                } else {
                    out.push(bytes[i]);
                    i += 1;
                }
            }
            b => {
                out.push(b);
                i += 1;
            }
        }
    }
    String::from_utf8_lossy(&out).into_owned()
}
