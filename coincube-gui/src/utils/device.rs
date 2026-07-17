use std::fs;
use std::path::PathBuf;
use uuid::Uuid;

fn device_fingerprint_path() -> PathBuf {
    let base = dirs::config_dir().unwrap_or_else(std::env::temp_dir);
    base.join("coincube").join("device_fingerprint")
}

pub fn get_or_create_device_fingerprint() -> String {
    let path = device_fingerprint_path();
    if let Ok(existing) = fs::read_to_string(&path) {
        let trimmed = existing.trim();
        if !trimmed.is_empty() {
            return trimmed.to_string();
        }
    }
    let fingerprint = Uuid::new_v4().to_string();
    let _ = fs::create_dir_all(path.parent().unwrap());
    let _ = fs::write(&path, &fingerprint);
    fingerprint
}

pub fn device_label() -> String {
    let host = hostname::get()
        .map(|h| h.to_string_lossy().to_string())
        .unwrap_or_else(|_| "Unknown Host".to_string());
    let os = std::env::consts::OS; // "macos", "windows", "linux"
    let os_label = match os {
        "macos" => "macOS",
        "windows" => "Windows",
        "linux" => "Linux",
        other => other,
    };
    format!("{host} ({os_label})")
}

pub fn device_headers() -> reqwest::header::HeaderMap {
    let mut headers = reqwest::header::HeaderMap::new();
    headers.insert(
        "X-Device-Fingerprint",
        reqwest::header::HeaderValue::from_str(&get_or_create_device_fingerprint())
            .unwrap_or_else(|_| reqwest::header::HeaderValue::from_static("unknown")),
    );
    headers.insert(
        "X-Device-Name",
        reqwest::header::HeaderValue::from_str(&device_label())
            .unwrap_or_else(|_| reqwest::header::HeaderValue::from_static("unknown")),
    );
    headers.insert(
        "X-Platform",
        reqwest::header::HeaderValue::from_static("desktop"),
    );
    headers
}
