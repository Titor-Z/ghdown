use std::fs::{self, OpenOptions};
use std::io::Write;
use std::path::{Path, PathBuf};

use anyhow::{anyhow, Context, Result};
use futures_util::StreamExt;
use indicatif::{MultiProgress, ProgressBar, ProgressDrawTarget, ProgressStyle};
use reqwest::Client;
use sha2::{Digest, Sha256};

use crate::proxy::ProxyManager;

/// 计算文件的 SHA256 并返回 "sha256:hex" 格式的字符串
fn sha256_file(path: &Path) -> Result<String> {
    let mut file = fs::File::open(path)
        .with_context(|| format!("无法打开文件做 checksum 校验: {}", path.display()))?;
    let mut hasher = Sha256::new();
    std::io::copy(&mut file, &mut hasher)?;
    let hash = hasher.finalize();
    let hex: String = hash.iter().map(|b| format!("{b:02x}")).collect();
    Ok(format!("sha256:{hex}"))
}

/// 校验文件 checksum，不匹配时打印警告（不返回错误）
pub fn verify_checksum(path: &Path, expected_digest: &str) {
    match sha256_file(path) {
        Ok(actual) if actual == expected_digest => {}
        Ok(actual) => {
            eprintln!(
                "  ⚠ checksum 不匹配，文件可能已损坏\n    期望: {expected_digest}\n    实际: {actual}"
            );
        }
        Err(e) => {
            eprintln!("  ⚠ checksum 验证失败: {e}");
        }
    }
}

pub struct DownloadManager {
    client: Client,
    quiet: bool,
}

impl DownloadManager {
    pub fn new(client: Client, quiet: bool) -> Self {
        Self { client, quiet }
    }

    /// 下载主入口（jobs>1 时自动尝试并发分片，不支持 Range 或小文件则回退单线程）
    /// 如果提供了 expected_digest，下载完成后自动校验 checksum（不匹配只警告）
    pub async fn download_with_fallback(
        &self,
        asset_url: &str,
        proxy_urls: &[String],
        proxy_mgr: &mut ProxyManager,
        output_path: &Path,
        jobs: usize,
        expected_digest: Option<&str>,
    ) -> Result<PathBuf> {
        if jobs > 1 {
            match self.probe_range_support(proxy_urls, asset_url).await {
                Ok((total_size, true)) if total_size >= 10 * 1024 * 1024 => {
                    let actual = jobs.min(total_size as usize / (5 * 1024 * 1024)).max(1);
                    if actual > 1 {
                        match self.download_concurrent(asset_url, proxy_urls, output_path, actual, total_size).await {
                            Ok(path) => {
                                proxy_mgr.record_success(&proxy_urls[0]);
                                let _ = proxy_mgr.persist().await;
                                if let Some(digest) = expected_digest {
                                    verify_checksum(&path, digest);
                                }
                                return Ok(path);
                            }
                            Err(e) => {
                                cleanup_part_files(output_path, actual);
                                if !self.quiet {
                                    eprintln!("  ℹ 并发下载失败 ({:#})，回退单线程", e);
                                }
                            }
                        }
                    }
                }
                Ok((_size, false)) => {
                    if !self.quiet {
                        eprintln!("  ℹ 服务器不支持 Range 请求，使用单线程下载");
                    }
                }
                _ => {}
            }
        }

        let result = self.download_sequential(asset_url, proxy_urls, proxy_mgr, output_path).await;
        if let Ok(ref path) = result {
            if let Some(digest) = expected_digest {
                verify_checksum(path, digest);
            }
        }
        result
    }

    /// HEAD 探测文件大小和 Range 支持
    async fn probe_range_support(&self, proxy_urls: &[String], asset_url: &str) -> Result<(u64, bool)> {
        for proxy in proxy_urls {
            let url = format!("{}{}", proxy, asset_url);
            let resp = self.client.head(&url).send().await?;
            if resp.status().is_success() || resp.status().as_u16() == 206 {
                let size = resp.content_length().unwrap_or(0);
                let range = resp.headers()
                    .get("accept-ranges")
                    .and_then(|v| v.to_str().ok())
                    .map(|v| v.contains("bytes"))
                    .unwrap_or(false);
                if size > 0 {
                    return Ok((size, range));
                }
            }
        }
        Err(anyhow!("无法从任何代理获取文件信息"))
    }

    /// 并发分片下载
    async fn download_concurrent(
        &self,
        asset_url: &str,
        proxy_urls: &[String],
        output_path: &Path,
        jobs: usize,
        total_size: u64,
    ) -> Result<PathBuf> {
        let seg_size = total_size / jobs as u64;
        let mp = MultiProgress::new();
        let mut handles = Vec::new();

        if !self.quiet {
            eprintln!("  ▸ 并发 {jobs} 线程分片下载 ({})", bytes_str(total_size));
        }

        for i in 0..jobs {
            let start = i as u64 * seg_size;
            let end = if i == jobs - 1 { total_size - 1 } else { start + seg_size - 1 };
            let seg_len = end - start + 1;
            let part_path = output_path.with_extension(format!("part.{i}"));
            let proxies = proxy_urls.to_vec();
            let client = self.client.clone();
            let url = asset_url.to_string();

            let pb = mp.add(ProgressBar::new(seg_len));
            if self.quiet {
                pb.set_draw_target(ProgressDrawTarget::hidden());
            } else {
                pb.set_prefix(format!("Seg {}/{}", i + 1, jobs));
                pb.set_style(
                    ProgressStyle::default_bar()
                        .template("{prefix} [{bar:30}] {bytes}/{total_bytes} {bytes_per_sec} ETA {eta}")
                        .unwrap()
                        .progress_chars("#>-"),
                );
            }

            handles.push(tokio::spawn(async move {
                let r = download_segment(&client, &proxies, &url, start, end, &part_path, &pb).await;
                pb.finish_and_clear();
                r
            }));
        }

        let mut errors = Vec::new();
        for handle in handles {
            match handle.await {
                Ok(Ok(_)) => {}
                Ok(Err(e)) => errors.push(e),
                Err(e) => errors.push(anyhow::Error::from(e)),
            }
        }

        if !errors.is_empty() {
            return Err(anyhow!("{} 个分片下载失败: {:#}", errors.len(), errors[0]));
        }

        merge_segments(output_path, jobs)?;
        if !self.quiet {
            eprintln!("  ▸ 合并完成");
        }
        Ok(output_path.to_path_buf())
    }

    /// 单线程下载（支持续传 + 代理 failover）
    async fn download_sequential(
        &self,
        asset_url: &str,
        proxy_urls: &[String],
        proxy_mgr: &mut ProxyManager,
        output_path: &Path,
    ) -> Result<PathBuf> {
        let part_path = output_path.with_extension("part");
        let existing = if part_path.exists() {
            fs::metadata(&part_path).map(|m| m.len()).unwrap_or(0)
        } else {
            0
        };
        if existing > 0 && !self.quiet {
            eprintln!("  ℹ 发现未完成的下载 ({})，尝试续传", bytes_str(existing));
        }

        for (i, proxy) in proxy_urls.iter().enumerate() {
            if i > 0 && !self.quiet {
                eprintln!("  ℹ 切换到备选代理 #{i}: {proxy}");
            }
            match self.download_one(asset_url, proxy, output_path, &part_path, if i > 0 { 0 } else { existing }).await {
                Ok(_) => {
                    proxy_mgr.record_success(proxy);
                    let _ = proxy_mgr.persist().await;
                    if part_path.exists() {
                        fs::rename(&part_path, output_path)?;
                    }
                    return Ok(output_path.to_path_buf());
                }
                Err(e) => {
                    proxy_mgr.record_failure(proxy);
                    let _ = proxy_mgr.persist().await;
                    if !self.quiet {
                        eprintln!("  ✗ {proxy} 失败: {e}");
                    }
                }
            }
        }
        Err(anyhow!("所有代理均下载失败"))
    }

    /// 通过单个代理下载（支持 Range 续传）
    async fn download_one(
        &self,
        asset_url: &str,
        proxy_url: &str,
        _output_path: &Path,
        part_path: &Path,
        existing: u64,
    ) -> Result<u64> {
        let full = format!("{}{}", proxy_url, asset_url);
        let mut req = self.client.get(&full);
        if existing > 0 {
            req = req.header("Range", format!("bytes={}-", existing));
        }
        let resp = req.send().await?;
        let status = resp.status();
        if !status.is_success() && status != 206 {
            return Err(anyhow!("HTTP {status}"));
        }

        let total = if status == 206 {
            resp.headers()
                .get("content-range")
                .and_then(|v| v.to_str().ok())
                .and_then(|cr| cr.split('/').nth(1))
                .and_then(|s| s.parse::<u64>().ok())
        } else {
            resp.content_length()
        }.unwrap_or(0);

        let pb = if self.quiet {
            let pb = ProgressBar::new(total);
            pb.set_draw_target(ProgressDrawTarget::hidden());
            pb
        } else if total > 0 {
            let pb = ProgressBar::new(total);
            pb.set_style(
                ProgressStyle::default_bar()
                    .template("{spinner:.green} [{elapsed_precise}] [{bar:40.cyan/blue}] {bytes}/{total_bytes} ({bytes_per_sec}) ETA {eta}")
                    .unwrap()
                    .progress_chars("#>-"),
            );
            pb.set_position(existing);
            pb
        } else {
            let pb = ProgressBar::new_spinner();
            pb.set_style(
                ProgressStyle::default_spinner()
                    .template("{spinner:.green} [{elapsed_precise}] {bytes} ({bytes_per_sec})")
                    .unwrap(),
            );
            pb
        };

        let mut file = OpenOptions::new()
            .create(true)
            .append(true)
            .open(part_path)
            .with_context(|| format!("无法创建文件: {}", part_path.display()))?;

        let mut stream = resp.bytes_stream();
        let mut written = existing;
        while let Some(chunk) = stream.next().await {
            let chunk = chunk?;
            file.write_all(&chunk)?;
            written += chunk.len() as u64;
            pb.set_position(written);
        }
        pb.finish_and_clear();
        Ok(written)
    }
}

/// 下载单个分片（通过 Range 请求）
async fn download_segment(
    client: &Client,
    proxy_urls: &[String],
    asset_url: &str,
    start: u64,
    end: u64,
    part_path: &Path,
    pb: &ProgressBar,
) -> Result<()> {
    for proxy in proxy_urls {
        let full = format!("{}{}", proxy, asset_url);
        let resp = client.get(&full)
            .header("Range", format!("bytes={}-{}", start, end))
            .send()
            .await?;

        if resp.status() != 206 {
            continue;
        }

        let mut file = OpenOptions::new()
            .create(true)
            .write(true)
            .truncate(true)
            .open(part_path)?;

        let mut stream = resp.bytes_stream();
        let mut written = 0u64;
        while let Some(chunk) = stream.next().await {
            let chunk = chunk?;
            file.write_all(&chunk)?;
            written += chunk.len() as u64;
            pb.set_position(written);
        }
        pb.finish_and_clear();
        return Ok(());
    }
    Err(anyhow!("所有代理均无法下载此分片"))
}

/// 按顺序合并分片文件
fn merge_segments(output_path: &Path, jobs: usize) -> Result<()> {
    let mut out = OpenOptions::new()
        .create(true)
        .write(true)
        .truncate(true)
        .open(output_path)?;

    for i in 0..jobs {
        let part_path = output_path.with_extension(format!("part.{i}"));
        if !part_path.exists() {
            return Err(anyhow!("分片文件缺失: {}", part_path.display()));
        }
        let mut part = fs::File::open(&part_path)?;
        std::io::copy(&mut part, &mut out)?;
        drop(part);
        fs::remove_file(&part_path)?;
    }
    Ok(())
}

/// 清理分片临时文件（失败时调用）
fn cleanup_part_files(output_path: &Path, jobs: usize) {
    for i in 0..jobs {
        let p = output_path.with_extension(format!("part.{i}"));
        let _ = fs::remove_file(&p);
    }
}

pub fn bytes_str(b: u64) -> String {
    if b < 1024 {
        format!("{b} B")
    } else if b < 1024 * 1024 {
        format!("{:.1} KB", b as f64 / 1024.0)
    } else if b < 1024 * 1024 * 1024 {
        format!("{:.1} MB", b as f64 / (1024.0 * 1024.0))
    } else {
        format!("{:.2} GB", b as f64 / (1024.0 * 1024.0 * 1024.0))
    }
}
