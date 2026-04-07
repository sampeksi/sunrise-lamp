use anyhow::Result;
use esp_idf_hal::ledc::*;
use esp_idf_hal::peripherals::Peripherals;
use esp_idf_hal::units::FromValueType;
use esp_idf_svc::eventloop::EspSystemEventLoop;
use esp_idf_svc::nvs::EspDefaultNvsPartition;
use esp_idf_svc::wifi::{AuthMethod, ClientConfiguration, Configuration, EspWifi, PmfConfiguration};
use std::time::Duration;
use std::thread;

fn main() -> Result<()> {
    esp_idf_svc::sys::link_patches();

    let peripherals = Peripherals::take()?;
    let sys_loop = EspSystemEventLoop::take()?;
    let nvs = EspDefaultNvsPartition::take()?;

    // 1. Setup Timer (The Drumbeat)
    let timer = LedcTimerDriver::new(
        peripherals.ledc.timer0,
        &config::TimerConfig::new()
            .frequency(5_u32.kHz().into())
            .resolution(Resolution::Bits12),
    )?;

    // 2. Setup Channels (The Musicians)
    // Pass the timer by reference (&timer) instead of cloning
    let mut channel_white_wire = LedcDriver::new(peripherals.ledc.channel0, &timer, peripherals.pins.gpio5)?;
    let mut channel_yellow_wire = LedcDriver::new(peripherals.ledc.channel1, &timer, peripherals.pins.gpio6)?;

    // 2. Wi-Fi Setup (Standard Routine)
    let mut wifi = EspWifi::new(peripherals.modem, sys_loop, Some(nvs))?;
    wifi.set_configuration(&Configuration::Client(ClientConfiguration {
        ssid: env!("WIFI_SSID").try_into().unwrap(),
        password: env!("WIFI_PSK").try_into().unwrap(),
        auth_method: AuthMethod::WPA2Personal,
        pmf_cfg: PmfConfiguration::NotCapable, 
        ..Default::default()
    }))?;

    wifi.start()?;
    unsafe { esp_idf_svc::sys::esp_wifi_set_max_tx_power(8 * 4); }
    wifi.connect()?;

    // Handshake Check
    for _ in 1..61 { 
        if wifi.is_connected()? { break; }
        thread::sleep(Duration::from_secs(1));
    }

    // SUCCESS SIGNAL: 3 Quick Blinks (Using both channels for maximum punch)
    for _ in 0..3 {
        channel_white_wire.set_duty(2000)?;
        channel_yellow_wire.set_duty(2000)?;
        thread::sleep(Duration::from_millis(200));
        channel_white_wire.set_duty(0)?;
        channel_yellow_wire.set_duty(0)?;
        thread::sleep(Duration::from_millis(200));
    }

    println!("--- CCT DISCOVERY MODE ---");
    println!("Watch the strip to see which color is which!");

    loop {
        // Test White Wire (GPIO 5)
        println!("Testing WHITE wire (GPIO 5)...");
        channel_white_wire.set_duty(1000)?; // ~25% brightness
        thread::sleep(Duration::from_secs(3));
        channel_white_wire.set_duty(0)?;

        thread::sleep(Duration::from_millis(500));

        // Test Yellow Wire (GPIO 6)
        println!("Testing YELLOW wire (GPIO 6)...");
        channel_yellow_wire.set_duty(1000)?;
        thread::sleep(Duration::from_secs(3));
        channel_yellow_wire.set_duty(0)?;

        thread::sleep(Duration::from_millis(2000));
    }
}