use std::collections::HashMap;
use std::time::Instant;

use anyhow::Result;
use chrono::Utc;
use reqwest::Client;

use crate::config::{ConfigManager, ProxyState, ProxyStateStore, ProxyStatus};

pub const BUILTIN_PROXIES: &[&str] = &[
    "https://777.z321.cc.cd/",
    "https://cdn.akaere.online/",
    "https://cdn.gh-proxy.com/",
    "https://cdn.gh-proxy.org/",
    "https://down.mxw.qzz.io/",
    "https://down.mxw.xx.kg/",
    "https://fastgit.cc/",
    "https://free.cn.eu.org/",
    "https://g.blfrp.cn/",
    "https://g.z321.cc.cd/",
    "https://gg.z321.cc.cd/",
    "https://gh-proxy.com/",
    "https://gh-proxy.net/",
    "https://gh-proxy.org/",
    "https://gh.927223.xyz/",
    "https://gh.acmsz.top/",
    "https://gh.b52m.cn/",
    "https://gh.bugdey.us.kg/",
    "https://gh.catmak.name/",
    "https://gh.chjina.com/",
    "https://gh.ddlc.top/",
    "https://gh.dpik.top/",
    "https://gh.felicity.ac.cn/",
    "https://gh.h233.eu.org/",
    "https://gh.idayer.com/",
    "https://gh.inkchills.cn/",
    "https://gh.jasonzeng.dev/",
    "https://gh.jjj.gv.uy/",
    "https://gh.llkk.cc/",
    "https://gh.meali.top/",
    "https://gh.monlor.com/",
    "https://gh.noki.icu/",
    "https://gh.sixyin.com/",
    "https://gh.tryxd.cn/",
    "https://gh.zwy.one/",
    "https://ghf.xn--6qq986b.top/",
    "https://ghfast.top/",
    "https://ghfile.geekertao.top/",
    "https://ghm.078465.xyz/",
    "https://ghp.arslantu.xyz/",
    "https://ghp.keleyaa.com/",
    "https://ghpr.cc/",
    "https://ghproxy.1888866.xyz/",
    "https://ghproxy.cxkpro.top/",
    "https://ghproxy.imciel.com/",
    "https://ghproxy.monkeyray.net/",
    "https://ghproxy.net/",
    "https://ghpxy.hwinzniej.top/",
    "https://git.669966.xyz/",
    "https://git.yylx.win/",
    "https://github-proxy.memory-echoes.cn/",
    "https://github.akams.cn/",
    "https://github.chenc.dev/",
    "https://github.dpik.top/",
    "https://github.ednovas.xyz/",
    "https://github.geekery.cn/",
    "https://github.mxw.qzz.io/",
    "https://github.starrlzy.cn/",
    "https://github.tbedu.top/",
    "https://github.tmby.shop/",
    "https://github.xxlab.tech/",
    "https://githubdog.com/",
    "https://gitproxy.127731.xyz/",
    "https://gitproxy.click/",
    "https://gitproxy.mrhjx.cn/",
    "https://gp.zkitefly.eu.org/",
    "https://j.1lin.dpdns.org/",
    "https://j.1win.ggff.net/",
    "https://jiashu.1win.eu.org/",
    "https://mirror.ghproxy.com/",
    "https://proxy.yaoyaoling.net/",
    "https://slink.ltd/",
    "https://tvv.tw/",
    "https://v4.gh-proxy.org/",
    "https://v6.gh-proxy.org/",
];

const PROBE_URL: &str = "https://github.com/favicon.ico";

/// 代理报告行（用于 CLI 表格展示）
pub struct ProxyReportRow {
    pub url: String,
    pub status: ProxyStatus,
    pub latency_ms: Option<u64>,
    pub health_score: f64,
    pub consecutive_failures: u32,
    pub total_successes: u64,
    pub total_failures: u64,
    pub dead_until: Option<String>,
}

pub struct ProxyManager {
    pub all_urls: Vec<String>,
    pub states: HashMap<String, ProxyState>,
    config_mgr: ConfigManager,
    client: Client,
    max_fails: u32,
    cooldown: u64,
    pub fallback_count: usize,
}

impl ProxyManager {
    pub async fn new(config_mgr: ConfigManager) -> Result<Self> {
        let config = config_mgr.load_config()?;
        let store = config_mgr.load_proxy_state()?;

        let mut urls: Vec<String> = config.custom_proxies.clone();
        for b in BUILTIN_PROXIES {
            if !urls.iter().any(|u| u == b) {
                urls.push(b.to_string());
            }
        }

        let client = Client::builder()
            .timeout(std::time::Duration::from_secs(10))
            .user_agent("ghdown/0.1.0")
            .build()?;

        Ok(Self {
            all_urls: urls,
            states: store.proxies,
            config_mgr,
            client,
            max_fails: config.health.max_consecutive_failures,
            cooldown: config.health.cooldown_seconds,
            fallback_count: config.health.fallback_count,
        })
    }

    pub async fn persist(&self) -> Result<()> {
        self.config_mgr
            .save_proxy_state(&ProxyStateStore {
                proxies: self.states.clone(),
            })
    }

    /// 并行探测所有代理
    pub async fn probe_all(&mut self) -> Result<()> {
        let urls: Vec<String> = self.all_urls.clone();
        let mut handles = Vec::new();
        for url in &urls {
            let u = url.clone();
            let c = self.client.clone();
            handles.push(tokio::spawn(async move {
                let target = format!("{}{}", u.trim_end_matches('/'), PROBE_URL);
                let start = Instant::now();
                match c.head(&target).send().await {
                    Ok(r) => {
                        let ms = start.elapsed().as_millis() as u64;
                        let ok = r.status().is_success() || r.status().as_u16() == 302;
                        (u, Some(ms), ok)
                    }
                    Err(_) => (u, None, false),
                }
            }));
        }

        for h in handles {
            if let Ok((url, ms, ok)) = h.await {
                let s = self.state(&url);
                s.last_checked = Some(Utc::now().naive_utc().to_string());
                s.latency_ms = ms;
                if ok {
                    s.status = ProxyStatus::Healthy;
                    s.consecutive_failures = 0;
                } else {
                    s.status = ProxyStatus::Dead;
                    s.consecutive_failures += 1;
                    s.total_failures += 1;
                }
            }
        }
        self.persist().await?;
        Ok(())
    }

    fn state(&mut self, url: &str) -> &mut ProxyState {
        if !self.states.contains_key(url) {
            self.states.insert(url.to_string(), ProxyState::new(url));
        }
        self.states.get_mut(url).unwrap()
    }

    fn available(&self, s: &ProxyState) -> bool {
        if s.status != ProxyStatus::Dead {
            return true;
        }
        if let Some(ref until) = s.dead_until {
            if let Ok(t) = until.parse::<chrono::NaiveDateTime>() {
                return Utc::now().naive_utc() < t;
            }
        }
        false
    }

    /// 获取按健康排序的可用代理列表
    pub fn get_sorted_healthy(&self) -> Vec<&ProxyState> {
        let mut v: Vec<&ProxyState> = self.states.values().filter(|s| self.available(s)).collect();
        v.sort_by(|a, b| {
            b.health_score()
                .partial_cmp(&a.health_score())
                .unwrap_or(std::cmp::Ordering::Equal)
                .then_with(|| {
                    let la = a.latency_ms.unwrap_or(u64::MAX);
                    let lb = b.latency_ms.unwrap_or(u64::MAX);
                    la.cmp(&lb)
                })
        });
        v
    }

    pub fn get_best_proxy_url(&self) -> Option<String> {
        self.get_sorted_healthy().first().map(|s| s.url.clone())
    }

    pub fn get_fallback_proxies(&self, exclude: &str) -> Vec<String> {
        self.get_sorted_healthy()
            .iter()
            .filter(|s| s.url != exclude)
            .take(self.fallback_count)
            .map(|s| s.url.clone())
            .collect()
    }

    pub fn record_success(&mut self, url: &str) {
        if let Some(s) = self.states.get_mut(url) {
            s.total_successes += 1;
            s.consecutive_failures = 0;
            s.status = ProxyStatus::Healthy;
        }
    }

    pub fn record_failure(&mut self, url: &str) {
        if let Some(s) = self.states.get_mut(url) {
            s.consecutive_failures += 1;
            s.total_failures += 1;
            s.status = ProxyStatus::Dead;
            if s.consecutive_failures >= self.max_fails {
                let end = Utc::now() + chrono::Duration::seconds(self.cooldown as i64);
                s.dead_until = Some(end.naive_utc().to_string());
            }
        }
    }

    /// 针对下载 URL 并行探测代理，返回按目标延迟排序的列表
    /// 成功(success/206)在前按延迟升序，失败在后保持原序
    pub async fn probe_for_url(&self, asset_url: &str, urls: &[String]) -> Vec<String> {
        let mut handles = Vec::new();
        for url in urls {
            let u = url.clone();
            let c = self.client.clone();
            let target = asset_url.to_string();
            handles.push(tokio::spawn(async move {
                let full = format!("{}{}", u, target);
                let start = Instant::now();
                match c.head(&full).send().await {
                    Ok(r) if r.status().is_success() || r.status().as_u16() == 206 || r.status().as_u16() == 302 => {
                        let ms = start.elapsed().as_millis() as u64;
                        (u, Some(ms), true)
                    }
                    _ => (u, None, false),
                }
            }));
        }

        let mut results: Vec<(String, Option<u64>, bool)> = Vec::new();
        for h in handles {
            if let Ok(r) = h.await {
                results.push(r);
            }
        }

        results.sort_by(|a, b| match (a.2, b.2) {
            (true, true) => a.1.unwrap_or(u64::MAX).cmp(&b.1.unwrap_or(u64::MAX)),
            (true, false) => std::cmp::Ordering::Less,
            (false, true) => std::cmp::Ordering::Greater,
            (false, false) => std::cmp::Ordering::Equal,
        });

        results.into_iter().map(|(u, _, _)| u).collect()
    }

    // ---- 运维管理 ----

    /// 添加自定义代理（同时持久化到 config.toml）
    pub fn add_custom(&mut self, url: &str) -> Result<()> {
        if !self.all_urls.iter().any(|u| u == url) {
            self.all_urls.push(url.to_string());
            self.states.insert(url.to_string(), ProxyState::new(url));
            let mut config = self.config_mgr.load_config()?;
            if !config.custom_proxies.iter().any(|u| u == url) {
                config.custom_proxies.push(url.to_string());
                self.config_mgr.save_config(&config)?;
            }
        }
        Ok(())
    }

    /// 删除代理
    pub fn remove_custom(&mut self, url: &str) -> Result<()> {
        self.all_urls.retain(|u| u != url);
        self.states.remove(url);
        let mut config = self.config_mgr.load_config()?;
        config.custom_proxies.retain(|u| u != url);
        self.config_mgr.save_config(&config)
    }

    /// 重置代理状态（清除 dead 标记）
    pub fn reset(&mut self, url: &str) {
        if let Some(s) = self.states.get_mut(url) {
            s.status = ProxyStatus::Unknown;
            s.consecutive_failures = 0;
            s.dead_until = None;
        }
    }

    /// 生成代理状态报告列表
    pub fn generate_report(&self) -> Vec<ProxyReportRow> {
        let mut rows: Vec<ProxyReportRow> = self
            .states
            .values()
            .map(|s| ProxyReportRow {
                url: s.url.clone(),
                status: s.status.clone(),
                latency_ms: s.latency_ms,
                health_score: s.health_score(),
                consecutive_failures: s.consecutive_failures,
                total_successes: s.total_successes,
                total_failures: s.total_failures,
                dead_until: s.dead_until.clone(),
            })
            .collect();

        rows.sort_by(|a, b| {
            let a_h = matches!(a.status, ProxyStatus::Healthy);
            let b_h = matches!(b.status, ProxyStatus::Healthy);
            b_h.cmp(&a_h).then_with(|| {
                let la = a.latency_ms.unwrap_or(u64::MAX);
                let lb = b.latency_ms.unwrap_or(u64::MAX);
                la.cmp(&lb)
            })
        });
        rows
    }


}
