mod config;
mod download;
mod proxy;
mod upgrade;

use std::io::{IsTerminal, Write};
use std::path::PathBuf;

use anyhow::{anyhow, Result};
use clap::builder::styling::{AnsiColor, Effects, Styles};
use clap::{Parser, Subcommand};
use futures_util::StreamExt;
use reqwest::Client;

use crate::config::{ConfigManager, ProxyStatus};
use crate::download::DownloadManager;
use crate::proxy::ProxyManager;

pub const STYLES: Styles = Styles::styled()
    .header(AnsiColor::Yellow.on_default().effects(Effects::BOLD))
    .usage(AnsiColor::Green.on_default().effects(Effects::BOLD))
    .literal(AnsiColor::Cyan.on_default().effects(Effects::BOLD))
    .placeholder(AnsiColor::BrightBlack.on_default());

#[derive(Parser)]
#[command(name = "ghdown", version = env!("GHDOWN_VERSION"), about = "GitHub Release 加速下载工具", styles = STYLES)]
struct Cli {
    #[command(subcommand)]
    command: Option<Commands>,

    /// GitHub Release 下载 URL
    url: Option<String>,

    /// 输出路径
    #[arg(short = 'o', long = "output")]
    output: Option<String>,

    /// 指定代理 URL（跳过自动测速）
    #[arg(short = 'p', long = "proxy")]
    proxy: Option<String>,

    /// 强制写入文件（即使 stdout 被重定向）
    #[arg(long = "file")]
    file: bool,

    /// 不自动测速，直接按列表顺序尝试
    #[arg(long = "no-probe")]
    no_probe: bool,

    /// 并发下载线程数（默认 4，设为 1 关闭并发）
    #[arg(short = 'j', long = "jobs", default_value_t = 4)]
    jobs: usize,

    /// 静默模式：不输出任何信息到 stderr（错误除外）
    #[arg(short = 'q', long = "quiet")]
    quiet: bool,
}

#[derive(Subcommand)]
enum Commands {
    /// 代理运维管理
    Proxy {
        #[command(subcommand)]
        action: ProxyAction,
    },
    /// 自动升级到最新版本
    Upgrade,
}

#[derive(Subcommand)]
enum ProxyAction {
    /// 列出所有代理及健康状态
    List,
    /// 重新测速所有代理
    Test,
    /// 添加自定义代理
    Add { url: String },
    /// 删除代理
    Remove { url: String },
    /// 重置代理的 dead 状态
    Reset { url: String },
    /// 显示健康评分详情
    Health,
}

#[tokio::main]
async fn main() {
    if let Err(e) = run().await {
        eprintln!("\n  ✗ {e}");
        let mut src = e.source();
        while let Some(cause) = src {
            eprintln!("    原因: {cause}");
            src = cause.source();
        }
        std::process::exit(1);
    }
}

async fn run() -> Result<()> {
    let cli = Cli::parse();

    // ---- 子命令路由 ----
    if let Some(cmd) = &cli.command {
        match cmd {
            Commands::Proxy { action } => return handle_proxy(action).await,
            Commands::Upgrade => {
                let config_mgr = ConfigManager::new()?;
                let client = Client::builder().user_agent("ghdown/0.1.0").build()?;
                let mut proxy_mgr = ProxyManager::new(config_mgr).await?;
                let proxy_urls =
                    resolve_proxies(&mut proxy_mgr, cli.proxy.as_deref(), cli.no_probe).await?;
                let _ = proxy_mgr.persist().await;
                return crate::upgrade::upgrade(
                    client,
                    &proxy_urls,
                    &mut proxy_mgr,
                    env!("GHDOWN_VERSION"),
                    cli.quiet,
                )
                .await;
            }
        }
    }

    // ---- 下载模式（默认） ----
    let url = cli.url.as_ref().ok_or_else(|| {
        anyhow!("用法: ghdown <URL>\n  或: ghdown proxy list")
    })?;
    let url = url.trim();

    let config_mgr = ConfigManager::new()?;
    let client = Client::builder().user_agent("ghdown/0.1.0").build()?;

    // 确定代理
    let mut proxy_mgr = ProxyManager::new(config_mgr).await?;
    let mut proxy_urls = resolve_proxies(&mut proxy_mgr, cli.proxy.as_deref(), cli.no_probe).await?;

    // 针对下载 URL 做二次探测，筛选出真正能处理此路径的代理
    if cli.proxy.is_none() && !cli.no_probe {
        proxy_urls = proxy_mgr.probe_for_url(url, &proxy_urls).await;
    }

    if !cli.quiet {
        let best_name = if cli.proxy.is_some() {
            cli.proxy.as_deref().unwrap()
        } else {
            &proxy_urls[0]
        };
        eprintln!("  ▸ 代理: {best_name}");
        if proxy_urls.len() > 1 {
            eprintln!("  ▸ 备选: {} 个", proxy_urls.len() - 1);
        }
    }

    if !std::io::stdout().is_terminal() && !cli.file {
        if !cli.quiet {
            eprintln!("  ℹ 下载: {}", url.rsplit('/').next().unwrap_or("unknown"));
        }
        stream_to_stdout(&client, url, &proxy_urls, &mut proxy_mgr, cli.quiet).await?;
    } else {
        let filename = url.rsplit('/').next().filter(|s| !s.is_empty()).unwrap_or("unknown");
        let out_path: PathBuf = match &cli.output {
            Some(p) => PathBuf::from(p),
            None => std::env::current_dir()
                .unwrap_or_else(|_| PathBuf::from("."))
                .join(filename),
        };
        if let Some(parent) = out_path.parent() {
            if !parent.exists() {
                std::fs::create_dir_all(parent)?;
            }
        }
        let dl = DownloadManager::new(client, cli.quiet);
        dl.download_with_fallback(url, &proxy_urls, &mut proxy_mgr, &out_path, cli.jobs)
            .await?;
        if !cli.quiet {
            eprintln!("  ✓ 完成: {}", out_path.display());
        }
    }
    Ok(())
}

// ===== 代理子命令 =====

async fn handle_proxy(action: &ProxyAction) -> Result<()> {
    let config_mgr = ConfigManager::new()?;
    match action {
        ProxyAction::List => {
            let mgr = ProxyManager::new(config_mgr).await?;
            print_proxy_table(&mgr);
        }
        ProxyAction::Test => {
            let mut mgr = ProxyManager::new(config_mgr).await?;
            eprintln!("  ℹ 正在探测所有代理...");
            mgr.probe_all().await?;
            print_proxy_table(&mgr);
        }
        ProxyAction::Add { url } => {
            let mut mgr = ProxyManager::new(config_mgr).await?;
            mgr.add_custom(url)?;
            mgr.persist().await?;
            eprintln!("  ✓ 已添加: {url}");
        }
        ProxyAction::Remove { url } => {
            let mut mgr = ProxyManager::new(config_mgr).await?;
            mgr.remove_custom(url)?;
            eprintln!("  ✓ 已删除: {url}");
        }
        ProxyAction::Reset { url } => {
            let mut mgr = ProxyManager::new(config_mgr).await?;
            mgr.reset(url);
            mgr.persist().await?;
            eprintln!("  ✓ 已重置: {url}");
        }
        ProxyAction::Health => {
            let mgr = ProxyManager::new(config_mgr).await?;
            print_health_table(&mgr);
        }
    }
    Ok(())
}

// ===== 下载辅助 =====

async fn resolve_proxies(mgr: &mut ProxyManager, specified: Option<&str>, no_probe: bool) -> Result<Vec<String>> {
    if let Some(p) = specified {
        return Ok(vec![p.to_string()]);
    }
    if !no_probe {
        mgr.probe_all().await?;
    }
    let best = mgr.get_best_proxy_url();
    match best {
        Some(url) => {
            let mut list = vec![url.clone()];
            list.extend(mgr.get_fallback_proxies(&url));
            Ok(list)
        }
        None => Ok(mgr.all_urls.clone()),
    }
}

async fn stream_to_stdout(client: &Client, asset_url: &str, proxy_urls: &[String], mgr: &mut ProxyManager, quiet: bool) -> Result<()> {
    for (i, proxy) in proxy_urls.iter().enumerate() {
        if i > 0 && !quiet {
            eprintln!("  ℹ 切换到备选代理 #{i}: {proxy}");
        }
        let full = format!("{}{}", proxy, asset_url);
        match client.get(&full).send().await {
            Ok(resp) if resp.status().is_success() => {
                mgr.record_success(proxy);
                let mut stdout = std::io::stdout().lock();
                let mut stream = resp.bytes_stream();
                while let Some(chunk) = stream.next().await {
                    stdout.write_all(&chunk?)?;
                }
                stdout.flush()?;
                return Ok(());
            }
            Ok(resp) => {
                mgr.record_failure(proxy);
                if !quiet {
                    eprintln!("  ✗ {proxy} HTTP {}", resp.status());
                }
            }
            Err(e) => {
                mgr.record_failure(proxy);
                if !quiet {
                    eprintln!("  ✗ {proxy} 失败: {e}");
                }
            }
        }
    }
    Err(anyhow!("所有代理均下载失败"))
}

// ===== 表格打印 =====

fn print_proxy_table(mgr: &ProxyManager) {
    let rows = mgr.generate_report();
    if rows.is_empty() {
        eprintln!("  (无代理)");
        return;
    }
    eprintln!("  {:<45}  {:<7}  {:<7}  健康分", "代理", "状态", "延迟");
    eprintln!("  {}", "-".repeat(75));
    for r in &rows {
        let icon = match r.status {
            ProxyStatus::Healthy => "✓".to_string(),
            ProxyStatus::Dead => {
                if r.dead_until.is_some() { "✗ 冷却".into() } else { "✗".into() }
            }
            ProxyStatus::Unknown => "○".into(),
        };
        let lat = r.latency_ms.map(|m| format!("{m}ms")).unwrap_or_else(|| "-".into());
        eprintln!("  {:<45}  {:<7}  {:<7}  {:.2}", r.url, icon, lat, r.health_score);
    }
}

fn print_health_table(mgr: &ProxyManager) {
    let rows = mgr.generate_report();
    eprintln!("  {:<45}  {:<6} {:<6} {:<5} 健康分  冷却至", "代理", "成功", "失败", "连败");
    eprintln!("  {}", "-".repeat(90));
    for r in &rows {
        let cooldown = r.dead_until.as_deref().unwrap_or("-");
        eprintln!(
            "  {:<45}  {:<6} {:<6} {:<5} {:.2}   {}",
            r.url, r.total_successes, r.total_failures, r.consecutive_failures, r.health_score, cooldown,
        );
    }
}
