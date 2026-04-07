fn main() {
    println!("cargo:rerun-if-changed=cfg.toml");

    // Load secrets from cfg.toml and make them available to env! macro
    let config = std::fs::read_to_string("cfg.toml").expect("Could not find cfg.toml");
    for line in config.lines() {
        if let Some((key, value)) = line.split_once('=') {
            let key = key.trim();
            let value = value.trim().trim_matches('"');
            // These lines tell the compiler to create an environment variable
            if key == "wifi_ssid" { println!("cargo:rustc-env=WIFI_SSID={}", value); }
            if key == "wifi_psk" { println!("cargo:rustc-env=WIFI_PSK={}", value); }
        }
    }

    embuild::espidf::sysenv::output();
}