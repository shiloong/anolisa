//! Background scheduler: auto-cleanup, health check, and orphan recovery.

use std::path::Path;
use std::sync::Arc;

use tokio::time::Duration;
use tracing::{debug, error, info, warn};

use crate::btrfs_ops;
use crate::state::DaemonState;

/// Start background scheduler tasks: orphan cleanup on boot, periodic auto-cleanup,
/// and periodic health checks.
pub fn start_scheduler(state: Arc<DaemonState>) {
    // Startup orphan cleanup
    let mount_path = state.mount_path.clone();
    tokio::spawn(async move {
        if let Err(e) = cleanup_orphans(&mount_path).await {
            error!("Failed to cleanup orphans: {}", e);
        }
    });

    // Periodic auto-cleanup (configurable interval)
    let state_clone = state.clone();
    tokio::spawn(async move {
        loop {
            let interval_secs = state_clone
                .config
                .read()
                .unwrap()
                .auto_cleanup_interval_secs;
            tokio::time::sleep(Duration::from_secs(interval_secs)).await;
            auto_cleanup(&state_clone).await;
        }
    });

    // Periodic health check (configurable interval)
    let state_clone2 = state.clone();
    tokio::spawn(async move {
        loop {
            let interval_secs = state_clone2
                .config
                .read()
                .unwrap()
                .health_check_interval_secs;
            tokio::time::sleep(Duration::from_secs(interval_secs)).await;
            health_check(&state_clone2).await;
        }
    });

    info!("Background scheduler started");
}

/// Orphan recovery: clean up `.rollback-tmp` residual directories.
///
/// Scans the mount path for directories ending with `.rollback-tmp`
/// and removes them. Returns the list of cleaned-up paths.
pub async fn cleanup_orphans(mount_path: &Path) -> Result<Vec<String>, anyhow::Error> {
    let mut cleaned = Vec::new();

    let read_dir = match std::fs::read_dir(mount_path) {
        Ok(rd) => rd,
        Err(e) => {
            warn!("Cannot read mount path for orphan cleanup: {}", e);
            return Ok(cleaned);
        }
    };

    for entry in read_dir {
        let entry = match entry {
            Ok(e) => e,
            Err(_) => continue,
        };

        let name = entry.file_name();
        let name_str = name.to_string_lossy().to_string();
        let path = entry.path();

        if name_str.ends_with(".rollback-tmp") {
            info!("Cleaning up orphan directory: {:?}", path);

            // Try btrfs subvolume delete first, fall back to remove_dir_all
            match btrfs_ops::delete_subvolume(&path).await {
                Ok(()) => {
                    info!("Deleted orphan subvolume: {:?}", path);
                }
                Err(_) => {
                    // Fallback: try regular directory removal
                    if let Err(e) = tokio::fs::remove_dir_all(&path).await {
                        warn!("Failed to remove orphan directory {:?}: {}", path, e);
                        continue;
                    }
                    info!("Removed orphan directory: {:?}", path);
                }
            }

            cleaned.push(path.to_string_lossy().to_string());
        }
    }

    if !cleaned.is_empty() {
        info!("Orphan cleanup complete: {} items removed", cleaned.len());
    }

    Ok(cleaned)
}

/// Auto-cleanup: iterate all workspaces and remove old unpinned snapshots.
/// The keep count is read from `state.config.auto_cleanup_keep`.
/// Pinned snapshots are excluded from the count and always preserved.
async fn auto_cleanup(state: &DaemonState) {
    let keep = state.config.read().unwrap().auto_cleanup_keep as usize;
    info!("Running auto-cleanup (keep={})...", keep);
    let all_ws = state.all_workspaces();

    for ws_arc in &all_ws {
        let mut ws = ws_arc.write().await;
        let snap_dir = state.backend.snapshots_root().join(&ws.ws_id);

        // Collect non-pinned snapshots sorted by created_at ascending
        let mut unpinned: Vec<(String, chrono::DateTime<chrono::Utc>)> = ws
            .index
            .snapshots
            .iter()
            .filter(|(_, meta)| !meta.pinned)
            .map(|(id, meta)| (id.clone(), meta.created_at))
            .collect();
        unpinned.sort_by_key(|(_, ts)| *ts);

        if unpinned.len() <= keep {
            continue;
        }

        let to_remove = &unpinned[..unpinned.len() - keep];
        let mut removed_count = 0;

        for (snap_id, _) in to_remove {
            let snap_path = snap_dir.join(snap_id);
            match btrfs_ops::delete_subvolume(&snap_path).await {
                Ok(()) => {
                    ws.index.snapshots.remove(snap_id);
                    removed_count += 1;
                }
                Err(e) => {
                    warn!("auto-cleanup: failed to delete {}: {:#}", snap_id, e);
                }
            }
        }

        if removed_count > 0 {
            if let Err(e) = crate::index_store::save(&snap_dir, &ws.index).await {
                warn!(
                    "auto-cleanup: failed to save index for {}: {:#}",
                    ws.ws_id, e
                );
            }
            info!(
                "auto-cleanup: removed {} snapshots from {}",
                removed_count, ws.ws_id
            );
        }
    }
}

/// Health check: verify filesystem usage.
///
/// Skipped when no workspace is registered. WARN on usage above threshold;
/// ERROR when get_usage fails (umount, fs crash, etc.) so upstream monitors can catch it.
async fn health_check(state: &DaemonState) {
    if state.all_workspaces().is_empty() {
        debug!("Health check skipped: no workspace registered");
        return;
    }

    match state.backend.get_usage().await {
        Ok((total, used)) => {
            if total > 0 {
                let usage_pct = (used as f64 / total as f64) * 100.0;
                let threshold = state.config.read().unwrap().fs_warn_threshold_percent;
                if usage_pct > threshold {
                    warn!(
                        "Filesystem usage critical: {:.1}% ({} / {} bytes)",
                        usage_pct, used, total
                    );
                } else {
                    info!("Health check OK: filesystem usage {:.1}%", usage_pct);
                }
            }
        }
        Err(e) => {
            // `{:#}` prints the full anyhow cause chain (e.g. outer
            // `with_context` + inner `bail!`), not just the outermost message.
            error!(
                "Health check failed on backend {}: {:#}",
                state.backend.backend_type(),
                e
            );
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn cleanup_orphans_removes_rollback_tmp() {
        let dir = tempfile::tempdir().unwrap();
        let orphan1 = dir.path().join("ws-abc123.rollback-tmp");
        let normal = dir.path().join("ws-normal");

        std::fs::create_dir(&orphan1).unwrap();
        std::fs::create_dir(&normal).unwrap();

        let cleaned = cleanup_orphans(dir.path()).await.unwrap();

        assert_eq!(cleaned.len(), 1);
        assert!(!orphan1.exists(), "rollback-tmp should be removed");
        assert!(normal.exists(), "normal directory should remain");
    }

    #[tokio::test]
    async fn cleanup_orphans_empty_dir() {
        let dir = tempfile::tempdir().unwrap();
        let cleaned = cleanup_orphans(dir.path()).await.unwrap();
        assert!(cleaned.is_empty());
    }

    #[tokio::test]
    async fn cleanup_orphans_nonexistent_path() {
        let result = cleanup_orphans(Path::new("/nonexistent/path/12345")).await;
        assert!(result.is_ok());
        assert!(result.unwrap().is_empty());
    }

    #[tokio::test]
    async fn cleanup_orphans_only_normal_dirs() {
        let dir = tempfile::tempdir().unwrap();
        std::fs::create_dir(dir.path().join("ws-abc")).unwrap();
        std::fs::create_dir(dir.path().join("snapshots")).unwrap();

        let cleaned = cleanup_orphans(dir.path()).await.unwrap();
        assert!(cleaned.is_empty());
    }
}
