use anyhow::Result;
use chrono::{Local, Timelike};
use esp_idf_hal::ledc::*;
use esp_idf_hal::peripherals::Peripherals;
use esp_idf_hal::units::FromValueType; // Required for .kHz()
use esp_idf_hal::io::Write;           // Required for .write_all()
use esp_idf_svc::http::server::{Configuration as HttpConfig, EspHttpServer};
use esp_idf_svc::nvs::{EspDefaultNvsPartition, EspNvs};
use esp_idf_svc::wifi::{AuthMethod, ClientConfiguration, Configuration, EspWifi, PmfConfiguration};
use std::sync::{Arc, Mutex};
use std::time::Duration;
use std::thread;

// Bake the HTML into the binary. 
// Ensure src/index.html exists!
const INDEX_HTML: &str = include_str!("index.html");

fn main() -> Result<()> {
    esp_idf_svc::sys::link_patches();
    let peripherals = Peripherals::take()?;
    let sys_loop = esp_idf_svc::eventloop::EspSystemEventLoop::take()?;
    let nvs_default = EspDefaultNvsPartition::take()?;

    // --- 1. WIFI INITIALIZATION (Priority #1) ---
    // Pass the partition to Wifi first, before anything else touches it
    let mut wifi = EspWifi::new(peripherals.modem, sys_loop, Some(nvs_default.clone()))?;
    
    wifi.set_configuration(&Configuration::Client(ClientConfiguration {
        ssid: env!("WIFI_SSID").try_into().unwrap(),
        password: env!("WIFI_PSK").try_into().unwrap(),
        auth_method: AuthMethod::WPA2Personal,
        pmf_cfg: PmfConfiguration::NotCapable, 
        ..Default::default()
    }))?;

    wifi.start()?;
    
    // CRITICAL: Lower power to keep the breadboard stable
    unsafe { esp_idf_svc::sys::esp_wifi_set_max_tx_power(8 * 4); }
    
    wifi.connect()?;

    println!("--- CONNECTING TO ZYXEL ---");
    let mut connected = false;
    for i in 1..61 { 
        if wifi.is_connected()? {
            println!("Connected! IP: {:?}", wifi.sta_netif().get_ip_info()?.ip);
            connected = true;
            break;
        }
        println!("Handshaking... {}/60", i);
        thread::sleep(Duration::from_secs(1));
    }

    if !connected {
        // SOS code...
    }

    // --- 2. NOW INITIALIZE OTHER SERVICES ---
    // Now that Wi-Fi is stable, we can open NVS and the Server
    let nvs_storage = EspNvs::new(nvs_default.clone(), "sunrise", true)?;
    let initial_h = nvs_storage.get_u8("h")?.unwrap_or(7);
    let initial_m = nvs_storage.get_u8("m")?.unwrap_or(0);
    let initial_c = nvs_storage.get_u8("c")?.unwrap_or(20);
    let state = Arc::new(Mutex::new((initial_h, initial_m, initial_c)));

    // Launch PWM
    let timer = LedcTimerDriver::new(peripherals.ledc.timer0, &config::TimerConfig::new().frequency(5_u32.kHz().into()).resolution(Resolution::Bits12))?;
    let mut warm_ch = LedcDriver::new(peripherals.ledc.channel0, &timer, peripherals.pins.gpio5)?;
    let mut cool_ch = LedcDriver::new(peripherals.ledc.channel1, &timer, peripherals.pins.gpio6)?;

    // Launch Server
    let mut server = EspHttpServer::new(&HttpConfig::default())?;

    // UI Handler
    server.fn_handler("/", esp_idf_svc::http::Method::Get, |req| -> anyhow::Result<()> {
        req.into_ok_response()?.write_all(INDEX_HTML.as_bytes())?;
        Ok(())
    })?;

    // Save Settings Handler (/save?t=07:30&c=20)
    // Save Settings Handler
    let state_save = state.clone();
    let nvs_partition = nvs_default.clone();
    server.fn_handler("/save", esp_idf_svc::http::Method::Get, move |req| -> anyhow::Result<()> {
        let uri = req.uri();
        if let Some(query) = uri.split('?').nth(1) {
            // Brute-force query parsing for H, M, and C (Coolness %)
            let h = query.split('t').nth(1).and_then(|s| s.get(1..3)).and_then(|s| s.parse::<u8>().ok()).unwrap_or(7);
            let m = query.split(':').nth(1).and_then(|s| s.get(0..2)).and_then(|s| s.parse::<u8>().ok()).unwrap_or(0);
            let c = query.split('c').nth(1).and_then(|s| s.get(1..)).and_then(|s| s.parse::<u8>().ok()).unwrap_or(20);

            // Update RAM
            {
                let mut data = state_save.lock().unwrap();
                *data = (h, m, c);
            }
            
            // Persist to NVS
            if let Ok(mut nvs) = EspNvs::new(nvs_partition.clone(), "sunrise", true) {
                nvs.set_u8("h", h).ok();
                nvs.set_u8("m", m).ok();
                nvs.set_u8("c", c).ok();
            }
            println!("Alarm updated: {:02}:{:02}, Tone: {}%", h, m, c);
        }
        req.into_ok_response()?.write_all(b"OK")?;
        Ok(())
    })?;

    // --- 5. TIME SYNC (Helsinki) ---
    let tz_key = std::ffi::CString::new("TZ")?;
    let tz_val = std::ffi::CString::new("EET-2EEST,M3.5.0/3,M10.5.0/4")?;
    unsafe {
        esp_idf_svc::sys::setenv(tz_key.as_ptr(), tz_val.as_ptr(), 1);
        esp_idf_svc::sys::tzset();
    }
    let _sntp = esp_idf_svc::sntp::EspSntp::new_default()?;
    println!("Time synced. System live.");

    // --- 6. THE DYNAMIC SCHEDULER LOOP ---
    let total_steps = 20 * 60 * 50; // 20 mins * 60 secs * 50Hz
    loop {
        let now = Local::now();
        let (target_h, target_m, target_c) = { *state.lock().unwrap() };

        let window_start_mins = (target_h as u32 * 60) + target_m as u32;
        let current_mins = (now.hour() * 60) + now.minute();

        // Trigger if we are within the 20-minute window
        if current_mins >= window_start_mins && current_mins < window_start_mins + 20 {
            println!("Triggering Sunrise Sequence...");
            
            // Calculate starting step based on seconds passed in the window
            let elapsed_secs = (current_mins - window_start_mins) * 60 + now.second() as u32;
            let mut step = elapsed_secs * 50;

            while step < total_steps {
                let progress = step as f32 / total_steps as f32;
                
                // Exponential Brightness (Gamma 2.2)
                let brightness = progress.powf(2.2);
                
                // Tone mix: Shifts from 100% Warm to the User's Target Tone
                let target_tone_factor = target_c as f32 / 100.0;
                let current_tone = progress * target_tone_factor; 

                let duty_warm = (brightness * (1.0 - current_tone) * 4095.0) as u32;
                let duty_cool = (brightness * current_tone * 4095.0) as u32;

                warm_ch.set_duty(duty_warm)?;
                cool_ch.set_duty(duty_cool)?;

                step += 1;
                thread::sleep(Duration::from_millis(20)); // 50Hz update rate
            }
            
            // Stay at full brightness until the window is over
            println!("Sunrise complete. Holding brightness.");
        }

        // Check every second while waiting
        thread::sleep(Duration::from_secs(1));
    }
}