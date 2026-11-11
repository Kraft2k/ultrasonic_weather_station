fn main() {
    embuild::espidf::sysenv::output();

    // Credentials are device-specific and must only come from the ignored
    // local file. Reusing credentials from another firmware breaks MQTT ACL
    // isolation between devices and tenants.
    let path = "device.env";
    println!("cargo:rerun-if-changed={path}");
    if let Ok(contents) = std::fs::read_to_string(path) {
        for line in contents.lines() {
            let line = line.trim();
            if line.is_empty() || line.starts_with('#') {
                continue;
            }
            if let Some((key, value)) = line.split_once('=') {
                println!("cargo:rustc-env={}={}", key.trim(), value.trim());
            }
        }
    }
}
