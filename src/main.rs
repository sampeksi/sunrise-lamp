use anyhow::Result;
use esp_idf_hal::ledc::*;
use esp_idf_hal::peripherals::Peripherals;
use esp_idf_hal::units::FromValueType;
use esp_idf_svc::eventloop::EspSystemEventLoop;
use esp_idf_svc::nvs::EspDefaultNvsPartition;
use esp_idf_svc::sntp::{EspSntp, SyncStatus};
use esp_idf_svc::wifi::{AuthMethod, ClientConfiguration, Configuration, EspWifi, PmfConfiguration};
use std::time::Duration;
use std::thread;

fn main() -> Result<()> {
    esp_idf_svc::sys::link_patches();

    let peripherals = Peripherals::take()?;
    let sys_loop = EspSystemEventLoop::take()?;
    let nvs = EspDefaultNvsPartition::take()?;

    // 1. Setup PWM (GPIO 5)
    let timer = LedcTimerDriver::new(
        peripherals.ledc.timer0,
        &config::TimerConfig::new().frequency(5_u32.kHz().into()).resolution(Resolution::Bits12),
    )?;
    let mut channel = LedcDriver::new(peripherals.ledc.channel0, timer, peripherals.pins.gpio5)?;

    // 2. Setup Wi-Fi (The "Proven Template" Version)
    let mut wifi = EspWifi::new(peripherals.modem, sys_loop, Some(nvs))?;

    let ssid = env!("WIFI_SSID");
    let pass = env!("WIFI_PSK");

    wifi.set_configuration(&Configuration::Client(ClientConfiguration {
        ssid: ssid.try_into().unwrap(),
        password: pass.try_into().unwrap(),
        auth_method: AuthMethod::WPA2Personal,
        pmf_cfg: PmfConfiguration::NotCapable, 
        ..Default::default()
    }))?;

    wifi.start()?;

    // Keep power low for the flash to avoid the "Broken Pipe" crash
    unsafe { esp_idf_svc::sys::esp_wifi_set_max_tx_power(8 * 4); }

    println!("--- ATTEMPTING ROUTER CONNECTION (APRIL 6 VERSION) ---");
    wifi.connect()?;

    // 3. Connection & Visual Feedback
    let mut connected = false;
    for i in 1..61 { 
        if wifi.is_connected()? {
            println!("SUCCESS! Handshake complete.");
            connected = true;
            break;
        }
        println!("Handshaking... attempt {}/60", i);
        thread::sleep(Duration::from_secs(1));
    }

    if !connected {
        // SOS Signal: Slow blinks (1s on, 1s off)
        loop {
            channel.set_duty(500)?;
            thread::sleep(Duration::from_millis(1000));
            channel.set_duty(0)?;
            thread::sleep(Duration::from_millis(1000));
        }
    }

    // SUCCESS SIGNAL: 3 Quick, Bright Blinks
    for _ in 0..3 {
        channel.set_duty(2000)?; 
        thread::sleep(Duration::from_millis(200));
        channel.set_duty(0)?;
        thread::sleep(Duration::from_millis(200));
    }

    // 4. Sync Time (NTP)
    let _sntp = EspSntp::new_default()?;
    while _sntp.get_sync_status() != SyncStatus::Completed {
        thread::sleep(Duration::from_millis(500));
    }

    // 5. Sunrise Logic
    let total_steps = 20 * 60 * 50; 
    let mut current_step = 0;
    loop {
        let brightness_pct = (current_step as f32 / total_steps as f32) * 100.0;
        let duty = ((brightness_pct / 100.0).powf(2.2) * 4095.0) as u32;
        channel.set_duty(duty)?;
        if current_step < total_steps { current_step += 1; }
        thread::sleep(Duration::from_millis(20)); 
    }
}