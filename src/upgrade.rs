use std::io::Write;

use anyhow::{anyhow, Result};
use futures_util::StreamExt;
use indicatif::{ProgressBar, ProgressDrawTarget, ProgressStyle};
use reqwest::Client;
use sha2::{Digest, Sha256};

use crate::proxy::ProxyManager;

/// 当前平台的扩展名
fn platform_ext() -> &'static str {
    if cfg!(windows) { ".exe" } else { "" }
}

/// 当前平台的 asset 候选名列表（优先匹配带平台后缀的全名，降级到简单名）
fn candidate_asset_names() -> Result<Vec<String>> {
    let arch = match std::env::consts::ARCH {
        "x86_64" => "x64",
        "aarch64" => "aarch64",
        "arm" => "arm",
        other => return Err(anyhow!("不支持的架构: {other}")),
    };
    let platform = match std::env::consts::OS {
        "windows" => "windows",
        "linux" => "linux",
        "macos" => "mac",
        other => return Err(anyhow!("不支持的操作系统: {other}")),
    };
    let ext = platform_ext();
    Ok(vec![
        format!("ghdown-{arch}-{platform}{ext}"),
        format!("ghdown{ext}"),
    ])
}

/// 从 GitHub API 获取最新 release 信息，返回 (tag, asset 名, 下载 URL, 可选 digest)
async fn fetch_latest_release(
    client: &Client,
    proxy_urls: &[String],
) -> Result<(String, String, String, Option<String>)> {
    let api = "https://api.github.com/repos/Titor-Z/ghdown/releases/latest";
    let candidates = candidate_asset_names()?;

    let mut urls: Vec<String> = proxy_urls.iter().map(|p| format!("{}{}", p, api)).collect();
    urls.push(api.to_string());

    for url in &urls {
        let resp = match client
            .get(url)
            .header("Accept", "application/vnd.github.v3+json")
            .send()
            .await
        {
            Ok(r) if r.status().is_success() => r,
            _ => continue,
        };
        let json: serde_json::Value = match resp.json().await {
            Ok(j) => j,
            Err(_) => continue,
        };
        let tag = match json["tag_name"].as_str() {
            Some(t) => t.to_string(),
            None => continue,
        };
        let assets = match json["assets"].as_array() {
            Some(a) => a,
            None => continue,
        };
        for name in &candidates {
            if let Some((asset_url, digest)) = assets.iter().find(|a| a["name"].as_str() == Some(name)).map(|a| {
                let url = a["browser_download_url"].as_str().unwrap_or("");
                let dig = a["digest"].as_str().map(|d| d.to_string());
                (url, dig)
            }) {
                if !asset_url.is_empty() {
                    return Ok((tag, name.clone(), asset_url.to_string(), digest));
                }
            }
        }
    }
    Err(anyhow!("无法获取最新 release 信息，请检查网络"))
}

/// 计算文件的 SHA256 十六进制字符串
fn sha256_file(path: &std::path::Path) -> Result<String> {
    let mut file = std::fs::File::open(path)?;
    let mut hasher = Sha256::new();
    std::io::copy(&mut file, &mut hasher)?;
    let hash = hasher.finalize();
    let hex: String = hash.iter().map(|b| format!("{b:02x}")).collect();
    Ok(format!("sha256:{hex}"))
}

/// 替换当前可执行文件：Unix rename / Windows rename+copy
#[cfg(unix)]
fn replace_exe(tmp_path: &std::path::Path, exe_path: &std::path::Path) -> Result<()> {
    use std::os::unix::fs::PermissionsExt;
    std::fs::set_permissions(tmp_path, std::fs::Permissions::from_mode(0o755))
        .map_err(|e| anyhow!("无法设置可执行权限: {e}"))?;
    std::fs::rename(tmp_path, exe_path)
        .map_err(|e| anyhow!("无法替换旧文件: {e}"))
}

/// 替换当前可执行文件：Windows rename+copy（覆写会被锁，先把自己挪走）
#[cfg(windows)]
fn replace_exe(tmp_path: &std::path::Path, exe_path: &std::path::Path) -> Result<()> {
    let backup = exe_path.with_extension("old");

    std::fs::rename(exe_path, &backup)
        .map_err(|e| anyhow!("无法备份旧文件: {e}"))?;

    if let Err(e) = std::fs::copy(tmp_path, exe_path) {
        let _ = std::fs::rename(&backup, exe_path);
        let _ = std::fs::remove_file(tmp_path);
        return Err(anyhow!("无法写入新文件: {e}"));
    }

    let _ = std::fs::remove_file(tmp_path);
    let _ = std::fs::remove_file(&backup);
    Ok(())
}

/// 自动升级到最新版本
pub async fn upgrade(
    client: Client,
    proxy_urls: &[String],
    proxy_mgr: &mut ProxyManager,
    current_version: &str,
    quiet: bool,
) -> Result<()> {
    if !quiet {
        eprintln!("  ℹ 当前版本: v{current_version}");
    }

    let (latest_tag, asset_name, asset_url, expected_digest) =
        fetch_latest_release(&client, proxy_urls).await?;
    let latest_ver = latest_tag.trim_start_matches('v');

    if latest_ver == current_version {
        if !quiet {
            eprintln!("  ✓ 已是最新版本 v{current_version}");
        }
        return Ok(());
    }

    if !quiet {
        eprintln!("  ▸ 发现新版本: {latest_tag} ({asset_name})");
    }

    let exe_path = std::env::current_exe()
        .map_err(|e| anyhow!("无法获取可执行文件路径: {e}"))?;
    let parent = exe_path
        .parent()
        .ok_or_else(|| anyhow!("无法获取安装目录"))?;
    let tmp_path = parent.join(".ghdown.update.tmp");

    let mut downloaded = false;
    for (i, proxy) in proxy_urls.iter().enumerate() {
        if i > 0 && !quiet {
            eprintln!("  ℹ 切换到备选代理 #{i}: {proxy}");
        }
        let full = format!("{}{}", proxy, asset_url);
        match client.get(&full).send().await {
            Ok(resp) if resp.status().is_success() => {
                let total = resp.content_length().unwrap_or(0);
                let pb = if quiet {
                    let pb = ProgressBar::new(total);
                    pb.set_draw_target(ProgressDrawTarget::hidden());
                    pb
                } else {
                    let pb = ProgressBar::new(total);
                    pb.set_style(
                        ProgressStyle::default_bar()
                            .template(
                                "{spinner:.green} [{elapsed_precise}] [{bar:40.cyan/blue}] {bytes}/{total_bytes} ({bytes_per_sec}) ETA {eta}",
                            )
                            .unwrap()
                            .progress_chars("#>-"),
                    );
                    pb
                };

                let mut file = std::fs::File::create(&tmp_path)
                    .map_err(|e| anyhow!("无法创建临时文件: {e}"))?;
                let mut stream = resp.bytes_stream();
                let mut written = 0u64;
                while let Some(chunk) = stream.next().await {
                    let chunk = chunk?;
                    file.write_all(&chunk)?;
                    written += chunk.len() as u64;
                    pb.set_position(written);
                }
                pb.finish_and_clear();
                proxy_mgr.record_success(proxy);
                let _ = proxy_mgr.persist().await;
                downloaded = true;
                break;
            }
            Ok(resp) => {
                proxy_mgr.record_failure(proxy);
                if !quiet {
                    eprintln!("  ✗ {proxy} HTTP {}", resp.status());
                }
            }
            Err(e) => {
                proxy_mgr.record_failure(proxy);
                if !quiet {
                    eprintln!("  ✗ {proxy} 失败: {e}");
                }
            }
        }
    }

    if !downloaded {
        return Err(anyhow!("所有代理均下载失败"));
    }

    // checksum 校验：API 提供了 digest 则强制验证
    if let Some(ref expected) = expected_digest {
        let actual = sha256_file(&tmp_path)?;
        if *expected != actual {
            let _ = std::fs::remove_file(&tmp_path);
            return Err(anyhow!(
                "checksum 不匹配，已删除损坏文件\n  期望: {expected}\n  实际: {actual}"
            ));
        }
        if !quiet {
            eprintln!("  ✓ checksum 验证通过");
        }
    }

    replace_exe(&tmp_path, &exe_path)?;

    if !quiet {
        eprintln!("  ✓ 已升级到 {latest_tag}");
    }

    Ok(())
}
