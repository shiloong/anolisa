use anyhow::{bail, Context};
use std::path::{Path, PathBuf};
use tokio::fs::File;
use tokio::io::{AsyncBufReadExt, BufReader};
use tokio::process::Command;
use tracing::{info, warn};

use crate::state::DaemonState;
use ws_ckpt_common::{DaemonConfig, SNAPSHOTS_DIR};

pub async fn bootstrap(config: &DaemonConfig) -> anyhow::Result<()> {
    // Derive image directory from configured image path
    let img_path = &config.img_path;
    let img_dir = std::path::Path::new(img_path)
        .parent()
        .map(|p| p.to_string_lossy().to_string())
        .unwrap_or_else(|| "/data/ws-ckpt".to_string());

    // 1. Ensure image directory exists
    tokio::fs::create_dir_all(&img_dir)
        .await
        .context("Failed to create ws-ckpt data directory")?;
    info!("Ensured image directory exists: {}", img_dir);

    // 2. Check if btrfs image file exists; create if not
    match tokio::fs::metadata(img_path).await {
        Ok(_) => {
            info!("Btrfs image already exists: {}", img_path);
        }
        Err(_) => {
            info!("Btrfs image not found, creating...");

            // Get available space for the image directory
            let df_output = run_command("df", &["-B1", &img_dir])
                .await
                .context("Failed to get partition info")?;
            let avail = parse_df_available(&df_output).context("Failed to parse df output")?;

            // Calculate configured percentage of available space, with configured minimum,
            // but never consume more than SAFETY_RATIO of available space so the host
            // partition keeps headroom for other writes.
            const GB: f64 = 1024.0 * 1024.0 * 1024.0;
            const SAFETY_RATIO: f64 = 0.80;
            let min_size: u64 = config.img_min_size_gb * 1024 * 1024 * 1024;
            let capacity_fraction = config.img_capacity_percent / 100.0;
            let safety_cap = (avail as f64 * SAFETY_RATIO) as u64;

            let img_size = if avail < min_size {
                // Available space is below the configured floor: degrade image size
                // to `avail * capacity_fraction`, but still clamp by the safety cap so
                // we never try to grab more than 80% of what's actually free.
                let degraded = std::cmp::min((avail as f64 * capacity_fraction) as u64, safety_cap);
                warn!(
                    "Disk available ({:.1} GB) is smaller than configured img_min_size_gb ({} GB). \
                     Degrading image size to {:.1} GB (<= {:.0}% of available). \
                     Consider freeing disk space or increasing disk size for better performance.",
                    avail as f64 / GB,
                    config.img_min_size_gb,
                    degraded as f64 / GB,
                    SAFETY_RATIO * 100.0,
                );
                degraded
            } else {
                // Normal path: take max(avail*capacity_fraction, min_size) but never
                // exceed the safety cap (80% of available).
                let desired = std::cmp::max((avail as f64 * capacity_fraction) as u64, min_size);
                let capped = std::cmp::min(desired, safety_cap);
                if capped < desired {
                    warn!(
                        "Desired image size {:.1} GB exceeds {:.0}% of available ({:.1} GB); \
                         capping to {:.1} GB to preserve host headroom.",
                        desired as f64 / GB,
                        SAFETY_RATIO * 100.0,
                        avail as f64 / GB,
                        capped as f64 / GB,
                    );
                }
                capped
            };
            info!(
                "Creating sparse image of {} bytes ({:.1} GB), available {:.1} GB",
                img_size,
                img_size as f64 / GB,
                avail as f64 / GB,
            );

            // Create sparse file
            run_command_checked("truncate", &["-s", &img_size.to_string(), img_path])
                .await
                .context("Failed to create sparse image file")?;

            // Format as btrfs
            run_command_checked("mkfs.btrfs", &["-f", img_path])
                .await
                .context("Failed to format btrfs image")?;

            info!("Btrfs image created and formatted: {}", img_path);
        }
    }

    // 3. Ensure mount point directory exists
    tokio::fs::create_dir_all(&config.mount_path)
        .await
        .context("Failed to create mount point directory")?;
    info!("Ensured mount point exists: {:?}", config.mount_path);

    // 4. Check if already mounted
    let mount_path_str = config.mount_path.to_string_lossy().to_string();
    if !is_mounted(&mount_path_str).await? {
        info!("Mounting btrfs image at {:?}", config.mount_path);

        // Setup loop device
        let loop_device = run_command("losetup", &["--find", "--show", &config.img_path])
            .await
            .context("Failed to setup loop device")?;
        let loop_device = loop_device.trim().to_string();
        info!("Loop device: {}", loop_device);

        // Mount
        run_command_checked("mount", &[&loop_device, &mount_path_str])
            .await
            .context("Failed to mount btrfs image")?;
        info!("Mounted {} at {}", loop_device, mount_path_str);
    } else {
        info!("Already mounted at {:?}", config.mount_path);
    }

    // 5. Ensure snapshots directory exists
    let snapshots_dir = config.mount_path.join(SNAPSHOTS_DIR);
    tokio::fs::create_dir_all(&snapshots_dir)
        .await
        .context("Failed to create snapshots directory")?;
    info!("Ensured snapshots directory exists: {:?}", snapshots_dir);

    // 6. Orphan cleanup: remove *.rollback-tmp subvolumes
    cleanup_orphans(&config.mount_path).await;

    info!("Bootstrap complete");
    Ok(())
}

/// Ensure all registered workspaces have valid symlinks.
/// Called after bootstrap to recover symlinks that may have been lost (e.g. after reboot).
pub async fn ensure_symlinks(state: &DaemonState) {
    let all_ws = state.all_workspaces();
    for arc in all_ws {
        let ws = arc.read().await;
        let expected_subvol_path = state.mount_path.join(&ws.ws_id);
        let ws_path = ws.path.to_string_lossy().to_string();

        // Guard: subvolume must exist, otherwise we'd create a dangling symlink
        if !expected_subvol_path.exists() {
            warn!(
                "subvolume {:?} missing for workspace {}; skipping symlink recovery",
                expected_subvol_path, ws.ws_id
            );
            continue;
        }

        match tokio::fs::read_link(&ws_path).await {
            Ok(target) if target == expected_subvol_path => {
                info!("symlink OK for {}: -> {:?}", ws_path, target);
            }
            Ok(target) => {
                warn!(
                    "symlink {} points to {:?}, expected {:?}; rebuilding",
                    ws_path, target, expected_subvol_path
                );
                let tmp_path = format!("{}.tmp", ws_path);
                if let Err(e) = tokio::fs::symlink(&expected_subvol_path, &tmp_path).await {
                    warn!("failed to create temp symlink for {}: {}", ws_path, e);
                } else if let Err(e) = tokio::fs::rename(&tmp_path, &ws_path).await {
                    warn!(
                        "failed to atomically replace symlink for {}: {}",
                        ws_path, e
                    );
                    let _ = tokio::fs::remove_file(&tmp_path).await;
                } else {
                    info!("rebuilt symlink for {}", ws_path);
                }
            }
            Err(_) => {
                // Symlink doesn't exist or path is not a symlink; rebuild
                warn!("symlink missing or invalid for {}; rebuilding", ws_path);
                let tmp_path = format!("{}.tmp", ws_path);
                if let Err(e) = tokio::fs::symlink(&expected_subvol_path, &tmp_path).await {
                    warn!("failed to create temp symlink for {}: {}", ws_path, e);
                } else if let Err(e) = tokio::fs::rename(&tmp_path, &ws_path).await {
                    warn!(
                        "failed to atomically replace symlink for {}: {}",
                        ws_path, e
                    );
                    let _ = tokio::fs::remove_file(&tmp_path).await;
                } else {
                    info!("created symlink for {}", ws_path);
                }
            }
        }
    }
}

pub async fn is_mounted(mount_path: &str) -> anyhow::Result<bool> {
    let target = Path::new(mount_path);
    let target_norm = target.components().collect::<PathBuf>();

    let file = File::open("/proc/mounts")
        .await
        .context("Failed to open /proc/mounts")?;
    let mut reader = BufReader::new(file).lines();

    while let Some(line) = reader.next_line().await? {
        let parts: Vec<&str> = line.split_whitespace().collect();
        if let Some(mp) = parts.get(1) {
            let mp_path = Path::new(mp);
            if mp_path == target || mp_path.components().collect::<PathBuf>() == target_norm {
                return Ok(true);
            }
        }
    }

    Ok(false)
}

fn parse_df_available(output: &str) -> anyhow::Result<u64> {
    // df -B1 output format:
    // Filesystem     1B-blocks  Used Available Use% Mounted on
    // /dev/sda1      ...        ...  ...       ...  /data
    let line = output
        .lines()
        .nth(1)
        .context("df output has no data line")?;
    let avail_str = line
        .split_whitespace()
        .nth(3)
        .context("df output missing available column")?;
    avail_str
        .parse::<u64>()
        .context("Failed to parse available size from df output")
}

async fn run_command(cmd: &str, args: &[&str]) -> anyhow::Result<String> {
    let output = Command::new(cmd)
        .args(args)
        .output()
        .await
        .with_context(|| format!("Failed to execute: {} {:?}", cmd, args))?;

    if !output.status.success() {
        let stderr = String::from_utf8_lossy(&output.stderr);
        bail!(
            "Command `{} {:?}` failed with status {}: {}",
            cmd,
            args,
            output.status,
            stderr.trim()
        );
    }

    Ok(String::from_utf8_lossy(&output.stdout).to_string())
}

async fn run_command_checked(cmd: &str, args: &[&str]) -> anyhow::Result<()> {
    run_command(cmd, args).await?;
    Ok(())
}

async fn cleanup_orphans(mount_path: &std::path::Path) {
    let read_dir = match std::fs::read_dir(mount_path) {
        Ok(rd) => rd,
        Err(e) => {
            warn!("Cannot read mount path for orphan cleanup: {}", e);
            return;
        }
    };

    for entry in read_dir {
        let entry = match entry {
            Ok(e) => e,
            Err(_) => continue,
        };

        let name = entry.file_name();
        let name_str = name.to_string_lossy();
        if name_str.ends_with(".rollback-tmp") {
            let path = entry.path();
            let ft = entry.file_type();
            if ft.is_ok() && ft.unwrap().is_symlink() {
                // Orphan rollback-tmp is a dangling symlink; just remove it
                info!("Removing orphan symlink: {:?}", path);
                if let Err(e) = std::fs::remove_file(&path) {
                    warn!("Failed to remove orphan symlink {:?}: {}", path, e);
                }
            } else {
                // Real subvolume; delete via btrfs
                info!("Cleaning up orphan rollback-tmp subvolume: {:?}", path);
                let path_str = path.to_string_lossy().to_string();
                if let Err(e) = run_command("btrfs", &["subvolume", "delete", &path_str]).await {
                    warn!("Failed to delete orphan subvolume {:?}: {}", path, e);
                }
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::parse_df_available;

    #[test]
    fn parses_available_column_from_df_b1() {
        // df -B1 /data sample output
        let out = "Filesystem     1B-blocks        Used   Available Use% Mounted on\n\
                   /dev/sda1    107374182400 32212254720 75161927680  30% /data\n";
        assert_eq!(parse_df_available(out).unwrap(), 75_161_927_680u64);
    }

    #[test]
    fn returns_err_on_missing_data_line() {
        let out = "Filesystem 1B-blocks Used Available Use% Mounted on\n";
        assert!(parse_df_available(out).is_err());
    }

    #[test]
    fn returns_err_on_non_numeric_available() {
        let out = "Filesystem 1B-blocks Used Available Use% Mounted on\n\
                   /dev/sda1 100 10 NaN 10% /data\n";
        assert!(parse_df_available(out).is_err());
    }
}
