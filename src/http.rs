use std::sync::OnceLock;

static GLOBAL_HTTP_CLIENT: OnceLock<reqwest::Client> = OnceLock::new();

pub fn client() -> &'static reqwest::Client {
    GLOBAL_HTTP_CLIENT.get_or_init(reqwest::Client::new)
}
