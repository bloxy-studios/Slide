use serde::{Deserialize, Serialize};
use uuid::Uuid;

#[derive(Clone, Debug, Default, Deserialize, Serialize, PartialEq, Eq)]
#[serde(default, rename_all = "camelCase", deny_unknown_fields)]
pub struct Config {
    pub seed: Option<i32>,
    pub platform: String,
    pub platform_version: String,
    pub brand: String,
    pub brand_version: String,
    pub hardware_concurrency: Option<u8>,
    pub language: String,
    pub timezone: String,
    pub restrict_webrtc: bool,
    pub native_surfaces: Vec<String>,
    pub device_name: String,
    pub mac_address: String,
}

impl Config {
    pub fn ensure_seed(&mut self) -> bool {
        if self.seed.is_some() {
            return false;
        }
        self.seed = Some((Uuid::new_v4().as_u128() & i32::MAX as u128) as i32);
        true
    }

    pub fn validate(&self) -> Result<(), String> {
        fn token(value: &str, max: usize, punctuation: &[u8]) -> bool {
            value.len() <= max
                && value.bytes().all(|byte| byte.is_ascii_alphanumeric() || punctuation.contains(&byte))
        }
        if matches!(self.brand.as_str(), "Opera" | "Vivaldi") && self.brand_version.is_empty() {
            return Err(format!("Enter a browser version for {}.", self.brand));
        }
        if !matches!(self.platform.as_str(), "" | "windows" | "linux" | "macos")
            || !matches!(self.brand.as_str(), "" | "Chrome" | "Edge" | "Opera" | "Vivaldi")
            || !token(&self.platform_version, 32, b".")
            || !token(&self.brand_version, 32, b".")
            || !token(&self.language, 35, b"-")
            || !token(&self.timezone, 64, b"/_+-")
            || self.hardware_concurrency.is_some_and(|count| count == 0 || count > 128)
            || self.device_name.chars().count() > 100
            || self.device_name.chars().any(char::is_control)
            || !token(&self.mac_address, 17, b":-")
            || self.native_surfaces.len() > 5
            || self.native_surfaces.iter().any(|surface| !matches!(surface.as_str(), "canvas" | "gpu" | "audio" | "clientrects" | "font"))
        {
            return Err("Check the fingerprint fields, then save again.".into());
        }
        Ok(())
    }

    pub fn flags(&self, patched: bool) -> Vec<String> {
        let mut flags = Vec::new();
        if self.restrict_webrtc {
            flags.push(if patched { "--disable-non-proxied-udp" } else { "--force-webrtc-ip-handling-policy=disable_non_proxied_udp" }.into());
        }
        if !self.language.is_empty() {
            flags.extend([format!("--lang={}", self.language), format!("--accept-lang={}", self.language)]);
        }
        if !patched {
            return flags;
        }
        if !self.timezone.is_empty() {
            flags.push(format!("--timezone={}", self.timezone));
        }
        let Some(seed) = self.seed else { return flags };
        flags.push(format!("--fingerprint={seed}"));
        for (flag, value) in [
            ("fingerprint-platform", &self.platform),
            ("fingerprint-platform-version", &self.platform_version),
            ("fingerprint-brand", &self.brand),
            ("fingerprint-brand-version", &self.brand_version),
        ] {
            if !value.is_empty() {
                flags.push(format!("--{flag}={value}"));
            }
        }
        if let Some(cores) = self.hardware_concurrency {
            flags.push(format!("--fingerprint-hardware-concurrency={cores}"));
        }
        if !self.native_surfaces.is_empty() {
            flags.push(format!("--disable-spoofing={}", self.native_surfaces.join(",")));
        }
        flags
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn opera_and_vivaldi_require_explicit_versions() {
        for (brand, version) in [("Opera", "114.0.0.0"), ("Vivaldi", "7.0.0.0")] {
            let mut config = Config { brand: brand.into(), seed: Some(42), ..Config::default() };
            assert!(config.validate().unwrap_err().contains(brand));
            config.brand_version = version.into();
            config.validate().unwrap();
            assert!(config.flags(true).contains(&format!("--fingerprint-brand-version={version}")));
        }
    }

    #[test]
    fn only_patched_browser_receives_fingerprint_switches() {
        let config = Config {
            seed: Some(42), brand: "Chrome".into(), platform: "windows".into(),
            native_surfaces: vec!["canvas".into(), "gpu".into()],
            language: "en-US".into(), timezone: "America/Denver".into(),
            device_name: "Local QA computer".into(), ..Config::default()
        };
        config.validate().unwrap();
        assert_eq!(config.flags(false), ["--lang=en-US", "--accept-lang=en-US"]);
        assert_eq!(config.flags(true), [
            "--lang=en-US", "--accept-lang=en-US", "--timezone=America/Denver",
            "--fingerprint=42", "--fingerprint-platform=windows", "--fingerprint-brand=Chrome",
            "--disable-spoofing=canvas,gpu",
        ]);
    }
}
