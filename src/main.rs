use anyhow::Result;
use chrono::{Local, Timelike};
use esp_idf_hal::ledc; // Specific import to avoid 'config' clash
use esp_idf_hal::peripherals::Peripherals;
use esp_idf_hal::units::FromValueType; 
use esp_idf_hal::io::Write;           
use esp_idf_svc::http::Method; 
use esp_idf_svc::http::server::{Configuration as HttpConfig, EspHttpServer};
use esp_idf_svc::nvs::{EspDefaultNvsPartition, EspNvs};
use esp_idf_svc::wifi::{AuthMethod, ClientConfiguration, Configuration, EspWifi};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};
use std::thread;

const INDEX_HTML: &str = include_str!("index.html");

struct LampState {
    hour: u8,
    min: u8,
    tone: u8,
    preview_until: Option<Instant>,
}

fn main() -> Result<()> {
    esp_idf_svc::sys::link_patches();
    let peripherals = Peripherals::take()?;
    let sys_loop = esp_idf_svc::eventloop::EspSystemEventLoop::take()?;
    let nvs_default = EspDefaultNvsPartition::take()?;

    // --- 1. QUIET BOOT: KILL LIGHTS IMMEDIATELY ---
    let timer = ledc::LedcTimerDriver::new(
        peripherals.ledc.timer0, 
        &ledc::config::TimerConfig::new()
            .frequency(5_u32.kHz().into())
            .resolution(ledc::Resolution::Bits12)
    )?;
    let mut warm_ch = ledc::LedcDriver::new(peripherals.ledc.channel0, &timer, peripherals.pins.gpio5)?;
    let mut cool_ch = ledc::LedcDriver::new(peripherals.ledc.channel1, &timer, peripherals.pins.gpio6)?;
    
    // Silence at power-on
    warm_ch.set_duty(0)?;
    cool_ch.set_duty(0)?;

    // --- 2. WIFI (Reliable DHCP) ---
    let mut wifi = EspWifi::new(peripherals.modem, sys_loop, Some(nvs_default.clone()))?;
    wifi.set_configuration(&Configuration::Client(ClientConfiguration {
        ssid: env!("WIFI_SSID").try_into().unwrap(),
        password: env!("WIFI_PSK").try_into().unwrap(),
        auth_method: AuthMethod::WPA2Personal,
        ..Default::default()
    }))?;

    wifi.start()?;
    unsafe { esp_idf_svc::sys::esp_wifi_set_max_tx_power(8 * 4); }
    wifi.connect()?;

    println!("Connecting to Wi-Fi...");
    while !wifi.is_connected()? { thread::sleep(Duration::from_millis(500)); }
    
    // Wait for the Zyxel to actually hand over an IP
    let mut ip_info = wifi.sta_netif().get_ip_info()?;
    while ip_info.ip.is_unspecified() {
        thread::sleep(Duration::from_millis(500));
        ip_info = wifi.sta_netif().get_ip_info()?;
    }
    println!("--- LAMP ONLINE ---");
    println!("IP Address: {:?}", ip_info.ip);
    println!("Go to your router settings to reserve this IP permanently!");

    // --- 3. STATE & NVS ---
    let nvs_storage = EspNvs::new(nvs_default.clone(), "sunrise", true)?;
    let state = Arc::new(Mutex::new(LampState {
        hour: nvs_storage.get_u8("h")?.unwrap_or(7),
        min: nvs_storage.get_u8("m")?.unwrap_or(0),
        tone: nvs_storage.get_u8("c")?.unwrap_or(20),
        preview_until: None,
    }));

    // --- 4. SERVER & ROBUST PARSING ---
    let mut server = EspHttpServer::new(&HttpConfig::default())?;
    server.fn_handler("/", Method::Get, |req| -> anyhow::Result<()> {
        req.into_ok_response()?.write_all(INDEX_HTML.as_bytes())?;
        Ok(())
    })?;

    let state_save = state.clone();
    let nvs_partition = nvs_default.clone();
    server.fn_handler("/save", Method::Get, move |req| -> anyhow::Result<()> {
        let uri = req.uri();
        if let Some(query) = uri.split('?').nth(1) {
            let mut h = 7; let mut m = 0; let mut c = 20; let mut is_preview = false;

            // Robust Key-Value Parsing (Solves the Preview bug)
            for part in query.split('&') {
                if let Some(val) = part.strip_prefix("t=") {
                    h = val.get(0..2).and_then(|s| s.parse().ok()).unwrap_or(h);
                    m = val.get(3..5).and_then(|s| s.parse().ok()).unwrap_or(m);
                } else if let Some(val) = part.strip_prefix("c=") {
                    let digits: String = val.chars().take_while(|ch| ch.is_ascii_digit()).collect();
                    c = digits.parse().unwrap_or(c);
                } else if part == "p=1" {
                    is_preview = true;
                }
            }

            let mut data = state_save.lock().unwrap();
            data.hour = h; data.min = m; data.tone = c;
            
            if is_preview {
                data.preview_until = Some(Instant::now() + Duration::from_secs(2));
            } else {
                if let Ok(mut nvs) = EspNvs::new(nvs_partition.clone(), "sunrise", true) {
                    nvs.set_u8("h", h).ok(); nvs.set_u8("m", m).ok(); nvs.set_u8("c", c).ok();
                }
                println!("Alarm Saved: {:02}:{:02}, Tone: {}%", h, m, c);
            }
        }
        req.into_ok_response()?.write_all(b"OK")?;
        Ok(())
    })?;

    let _sntp = esp_idf_svc::sntp::EspSntp::new_default()?;

    // --- 5. THE STATE ENGINE (Non-Blocking) ---
    loop {
        let now = Local::now();
        let (h, m, c, preview) = {
            let data = state.lock().unwrap();
            (data.hour, data.min, data.tone, data.preview_until)
        };

        let target_mins = (h as u32 * 60) + m as u32;
        let current_mins = (now.hour() * 60) + now.minute();
        let start_mins = target_mins.saturating_sub(20);

        if let Some(until) = preview {
            if Instant::now() < until {
                // PREVIEW MODE: 100% Brightness for the test
                set_lamp(&mut warm_ch, &mut cool_ch, 1.0, c as f32 / 100.0)?;
            } else {
                state.lock().unwrap().preview_until = None;
            }
        } else if current_mins >= start_mins && current_mins < target_mins {
            // SUNRISE MODE
            let elapsed_secs = ((current_mins - start_mins) * 60) + now.second() as u32;
            let progress = elapsed_secs as f32 / 1200.0; 
            set_lamp(&mut warm_ch, &mut cool_ch, progress.powf(2.2), progress * (c as f32 / 100.0))?;
        } else if current_mins >= target_mins && current_mins < target_mins + 30 {
            // HOLD MODE: On for 30 mins after wake-up
            set_lamp(&mut warm_ch, &mut cool_ch, 1.0, c as f32 / 100.0)?;
        } else {
            // OFF MODE
            warm_ch.set_duty(0)?;
            cool_ch.set_duty(0)?;
        }

        thread::sleep(Duration::from_millis(200));
    }
}

fn set_lamp(warm: &mut ledc::LedcDriver, cool: &mut ledc::LedcDriver, bri: f32, tone: f32) -> Result<()> {
    warm.set_duty((bri * (1.0 - tone) * 4095.0) as u32)?;
    cool.set_duty((bri * tone * 4095.0) as u32)?;
    Ok(())
}