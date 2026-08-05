use serde::Deserialize;
use std::collections::HashMap;
use std::sync::{Arc, OnceLock, RwLock};

#[derive(Debug, Clone, Deserialize)]
pub struct RankPrice {
    pub original_toman: u64,
    pub final_toman: u64,
    pub discount_pct: u32,
    #[serde(default)]
    #[allow(dead_code)]
    pub is_free: bool,
    #[serde(default)]
    pub buy_url: Option<String>,
}

#[derive(Debug, Clone, Deserialize)]
pub struct RankPricesConfig {
    pub admin_username: String,
    pub ranks: HashMap<String, RankPrice>,
}

impl Default for RankPricesConfig {
    fn default() -> Self {
        let mut ranks = HashMap::new();
        ranks.insert(
            "dalavar".to_string(),
            RankPrice {
                original_toman: 0,
                final_toman: 0,
                discount_pct: 0,
                is_free: true,
                buy_url: None,
            },
        );
        ranks.insert(
            "sepahbod".to_string(),
            RankPrice {
                original_toman: 180000,
                final_toman: 150000,
                discount_pct: 16,
                is_free: false,
                buy_url: None,
            },
        );
        ranks.insert(
            "sohrab".to_string(),
            RankPrice {
                original_toman: 240000,
                final_toman: 200000,
                discount_pct: 16,
                is_free: false,
                buy_url: None,
            },
        );
        ranks.insert(
            "esfandyar".to_string(),
            RankPrice {
                original_toman: 300000,
                final_toman: 250000,
                discount_pct: 16,
                is_free: false,
                buy_url: None,
            },
        );
        ranks.insert(
            "rostam".to_string(),
            RankPrice {
                original_toman: 480000,
                final_toman: 400000,
                discount_pct: 16,
                is_free: false,
                buy_url: None,
            },
        );

        Self {
            admin_username: "mmahdi_sz".to_string(),
            ranks,
        }
    }
}

static PRICES: OnceLock<Arc<RwLock<RankPricesConfig>>> = OnceLock::new();

fn read_config_file() -> RankPricesConfig {
    let paths = ["config.yml", "config/config.yml", "config/rank_prices.yml"];
    for path in &paths {
        if let Ok(content) = std::fs::read_to_string(path) {
            if let Ok(cfg) = serde_yaml::from_str::<RankPricesConfig>(&content) {
                return cfg;
            }
        }
    }
    RankPricesConfig::default()
}

fn cache() -> &'static Arc<RwLock<RankPricesConfig>> {
    PRICES.get_or_init(|| Arc::new(RwLock::new(read_config_file())))
}

pub fn load() {
    let _ = cache();
}

pub fn reload() {
    let fresh = read_config_file();
    if let Ok(mut guard) = cache().write() {
        *guard = fresh;
        eprintln!("[prices] reloaded config.yml from disk");
    }
}

pub fn get() -> RankPricesConfig {
    cache()
        .read()
        .map(|guard| guard.clone())
        .unwrap_or_default()
}
