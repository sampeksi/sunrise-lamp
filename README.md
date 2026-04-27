# Sunrise Lamp

A Rust-powered, web-controlled smart sunrise alarm clock built on the ESP32-C3.

This project simulates a natural sunrise by cross-fading between Warm White and Cool White LED channels using a gamma-corrected exponential curve. It features a non-blocking state engine, NTP time synchronization, and a mobile-optimized web interface.

## Features

- **Dual-Channel PWM**: Independent control of Warm and Cool LED strips (mapped to GPIO 5 and 6)

- **State-Engine Scheduler**: Non-blocking logic that calculates the exact brightness and tone required for the current time, making it resilient to reboots or mid-sunrise setting changes

- **Quiet Boot**: The lamp initializes to 0% brightness immediately upon power-up, ensuring no accidental flashes during Wi-Fi connection

- **Web UI**: Responsive mobile dashboard for setting alarm times and real-time "Tone Preview"

- **Persistence**: Settings (Alarm Time & Tone) are saved to the ESP32's NVS (Non-Volatile Storage)

- **NTP Sync**: Automatically fetches Helsinki time (EET/EEST) on startup
