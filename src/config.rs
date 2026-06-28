use std::collections::HashMap;
use std::fs;
use std::path::PathBuf;

use anyhow::{anyhow, Result};
use serde::{Deserialize, Serialize};

/// 代理健康状态
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub enum ProxyStatus {
    #[serde(rename = "unknown")]
    Unknown,
    #[serde(rename = "healthy")]
    Healthy,
    #[serde(rename = "dead")]
    Dead,
}

impl Default for ProxyStatus {
    fn default() -> Self {
        Self::Unknown
    }
}

/// 单个代理运行时状态
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ProxyState {
    pub url: String,
    #[serde(default)]
    pub status: ProxyStatus,
    pub latency_ms: Option<u64>,
    pub last_checked: Option<String>,
    #[serde(default)]
    pub consecutive_failures: u32,
    #[serde(default)]
    pub total_successes: u64,
    #[serde(default)]
    pub total_failures: u64,
    pub dead_until: Option<String>,
}

impl ProxyState {
    pub fn new(url: &str) -> Self {
        Self {
            url: url.to_string(),
            status: ProxyStatus::Unknown,
            latency_ms: None,
            last_checked: None,
            consecutive_failures: 0,
            total_successes: 0,
            total_failures: 0,
            dead_until: None,
        }
    }

    /// 健康分数 (0.0 ~ 1.0)
    pub fn health_score(&self) -> f64 {
        let w = self.total_failures as f64 * 2.0;
        let t = self.total_successes as f64 + w;
        if t == 0.0 {
            return 0.5;
        }
        self.total_successes as f64 / t
    }
}

/// 用户配置
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Config {
    #[serde(default)]
    pub custom_proxies: Vec<String>,
    #[serde(default)]
    pub health: HealthConfig,
}

impl Default for Config {
    fn default() -> Self {
        Self {
            custom_proxies: Vec::new(),
            health: HealthConfig::default(),
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct HealthConfig {
    #[serde(default = "default_max_fails")]
    pub max_consecutive_failures: u32,
    #[serde(default = "default_cooldown")]
    pub cooldown_seconds: u64,
    #[serde(default = "default_fallback")]
    pub fallback_count: usize,
}

impl Default for HealthConfig {
    fn default() -> Self {
        Self {
            max_consecutive_failures: default_max_fails(),
            cooldown_seconds: default_cooldown(),
            fallback_count: default_fallback(),
        }
    }
}

fn default_max_fails() -> u32 { 3 }
fn default_cooldown() -> u64 { 300 }
fn default_fallback() -> usize { 3 }

/// 代理状态持久化存储
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ProxyStateStore {
    pub proxies: HashMap<String, ProxyState>,
}

/// 配置管理器
#[derive(Clone)]
pub struct ConfigManager {
    config_path: PathBuf,
    state_path: PathBuf,
}

impl ConfigManager {
    pub fn new() -> Result<Self> {
        let base = dirs::config_dir()
            .ok_or_else(|| anyhow!("无法获取配置目录"))?
            .join("ghdown");
        let config_path = base.join("config.toml");
        let state_path = base.join("proxy_state.json");
        fs::create_dir_all(&base)?;
        Ok(Self {
            config_path,
            state_path,
        })
    }

    pub fn load_config(&self) -> Result<Config> {
        if !self.config_path.exists() {
            let cfg = Config::default();
            self.save_config(&cfg)?;
            return Ok(cfg);
        }
        Ok(toml::from_str(&fs::read_to_string(&self.config_path)?)?)
    }

    pub fn save_config(&self, config: &Config) -> Result<()> {
        fs::write(&self.config_path, toml::to_string_pretty(config)?)?;
        Ok(())
    }

    pub fn load_proxy_state(&self) -> Result<ProxyStateStore> {
        if !self.state_path.exists() {
            return Ok(ProxyStateStore {
                proxies: HashMap::new(),
            });
        }
        Ok(serde_json::from_str(&fs::read_to_string(&self.state_path)?)?)
    }

    pub fn save_proxy_state(&self, store: &ProxyStateStore) -> Result<()> {
        fs::write(&self.state_path, serde_json::to_string_pretty(store)?)?;
        Ok(())
    }
}
