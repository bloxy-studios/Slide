use base64::{engine::general_purpose::STANDARD, Engine};

const PAGE: &str = include_str!("startup.html");

pub fn prepend_check(urls: &[String]) -> Vec<String> {
    let local = format!("data:text/html;charset=utf-8;base64,{}", STANDARD.encode(PAGE));
    std::iter::once(local).chain(urls.iter().cloned()).collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn startup_page_is_local_and_precedes_configured_tabs() {
        let configured = ["https://example.com/".to_string(), "http://127.0.0.1:4320/".to_string()];
        let urls = prepend_check(&configured);
        let payload = urls[0].strip_prefix("data:text/html;charset=utf-8;base64,").unwrap();
        assert_eq!(STANDARD.decode(payload).unwrap(), PAGE.as_bytes());
        assert_eq!(&urls[1..], &configured);
    }
}
