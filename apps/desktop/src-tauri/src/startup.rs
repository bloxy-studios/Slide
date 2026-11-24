use base64::{engine::general_purpose::STANDARD, Engine};

const PAGE: &str = include_str!("startup.html");

pub fn check_url() -> String {
    format!("data:text/html;charset=utf-8;base64,{}", STANDARD.encode(PAGE))
}

pub fn launch_tabs(urls: &[String], check: bool) -> (Vec<String>, bool) {
    if !check || urls.is_empty() {
        return (urls.to_vec(), check);
    }
    (std::iter::once(check_url()).chain(urls.iter().cloned()).collect(), false)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn startup_page_is_local_and_precedes_configured_tabs() {
        let configured = ["https://example.com/".to_string(), "http://127.0.0.1:4320/".to_string()];
        let (urls, deferred) = launch_tabs(&configured, true);
        assert!(!deferred);
        let payload = urls[0].strip_prefix("data:text/html;charset=utf-8;base64,").unwrap();
        assert_eq!(STANDARD.decode(payload).unwrap(), PAGE.as_bytes());
        assert_eq!(&urls[1..], &configured);
    }

    #[test]
    fn no_slide_urls_leaves_chromium_in_charge_of_startup() {
        assert_eq!(launch_tabs(&[], true), (Vec::new(), true));
        assert_eq!(launch_tabs(&[], false), (Vec::new(), false));
    }
}
