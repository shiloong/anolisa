use std::os::unix::fs::{MetadataExt, PermissionsExt};
use std::path::{Path, PathBuf};

use anyhow::{bail, Context};
use async_trait::async_trait;
use nix::unistd::{chown, Gid, Uid};
use tokio::process::Command;
use tracing::{error, info, warn};

use ws_ckpt_common::backend::*;
use ws_ckpt_common::{DiffEntry, WorkspaceInfo, SNAPSHOTS_DIR};

use super::btrfs_common;
use btrfs_common::resolve_symlink_path;

pub struct BtrfsLoopBackend {
    pub mount_path: PathBuf,
    pub img_path: PathBuf,
    pub snapshots_dir: PathBuf,
}

impl BtrfsLoopBackend {
    pub fn new(mount_path: PathBuf, img_path: PathBuf) -> Self {
        let snapshots_dir = mount_path.join(SNAPSHOTS_DIR);
        Self {
            mount_path,
            img_path,
            snapshots_dir,
        }
    }

    /// Internal init implementation; caller wraps with cleanup-on-failure.
    async fn do_init_storage(
        &self,
        original_path: &str,
        ws_id: &str,
        subvol_path: &Path,
        snap_dir: &Path,
    ) -> anyhow::Result<()> {
        // 1. Create subvolume
        btrfs_common::create_subvolume(subvol_path).await?;

        // 2. Create snapshots dir
        tokio::fs::create_dir_all(snap_dir)
            .await
            .context("failed to create snapshots directory")?;

        // 3. rsync migration
        // --copy-unsafe-links: dereference symlinks that point outside the source tree
        // (e.g. symlinks to other ws-* subvolumes inside mount point)
        let src = format!("{}/", original_path); // trailing / is important
        let status = Command::new("rsync")
            .args([
                "-a",
                "--copy-unsafe-links",
                &src,
                &subvol_path.to_string_lossy(),
            ])
            .status()
            .await
            .context("failed to run rsync")?;
        if !status.success() {
            anyhow::bail!("rsync failed with exit code: {:?}", status.code());
        }

        // 3a. Flush dirty data to disk so subsequent snapshots are instant
        let sync_status = Command::new("btrfs")
            .args(["filesystem", "sync", &subvol_path.to_string_lossy()])
            .status()
            .await
            .context("failed to run btrfs filesystem sync")?;
        if !sync_status.success() {
            warn!("btrfs filesystem sync returned non-zero, falling back to sync()");
            Command::new("sync").status().await.ok();
        }

        // 4. Record original directory permissions before removal
        let orig_meta = tokio::fs::metadata(original_path)
            .await
            .context("failed to read original directory metadata")?;
        let orig_uid = orig_meta.uid();
        let orig_gid = orig_meta.gid();

        // 5. Remove original directory (data is safely in btrfs subvolume now)
        tokio::fs::remove_dir_all(original_path)
            .await
            .context("failed to remove original directory")?;

        // 6. Create symlink: user path -> btrfs subvolume
        if let Some(parent) = Path::new(original_path).parent() {
            tokio::fs::create_dir_all(parent)
                .await
                .context("failed to create parent directory for symlink")?;
        }
        tokio::fs::symlink(subvol_path, original_path)
            .await
            .context("failed to create symlink")?;

        // 6a. Restore ownership on the subvolume root to match original directory
        chown(
            subvol_path,
            Some(Uid::from_raw(orig_uid)),
            Some(Gid::from_raw(orig_gid)),
        )
        .context("failed to restore subvolume ownership")?;

        // 7. Verify symlink
        let link_target = tokio::fs::read_link(original_path)
            .await
            .context("symlink verification failed: cannot read link")?;
        if link_target != subvol_path {
            anyhow::bail!(
                "symlink verification failed: expected {:?}, got {:?}",
                subvol_path,
                link_target
            );
        }

        info!(
            "BtrfsLoopBackend: storage init complete for ws_id={}, subvol={}",
            ws_id,
            subvol_path.display()
        );
        Ok(())
    }

    /// Cleanup partially-created storage on init failure.
    async fn cleanup_init_storage(original_path: &str, subvol_path: &Path, snap_dir: &Path) {
        // Remove symlink if it exists
        let _ = tokio::fs::remove_file(original_path).await;

        // Remove snapshots dir
        let _ = tokio::fs::remove_dir_all(snap_dir).await;

        // Delete subvolume (best effort)
        if let Err(e) = btrfs_common::delete_subvolume(subvol_path).await {
            error!("cleanup: failed to delete subvolume: {}", e);
        }
    }
}

#[async_trait]
impl StorageBackend for BtrfsLoopBackend {
    fn backend_type(&self) -> BackendType {
        BackendType::BtrfsLoop
    }

    fn data_root(&self) -> &Path {
        &self.mount_path
    }

    fn snapshots_root(&self) -> &Path {
        &self.snapshots_dir
    }

    async fn init_workspace(
        &self,
        original_path: &str,
        ws_id: &str,
    ) -> anyhow::Result<WorkspaceInfo> {
        // Resolve symlink to real path to avoid copying the symlink itself
        let resolved = resolve_symlink_path(original_path).await?;
        let resolved_str = resolved.to_string_lossy().to_string();

        let subvol_path = self.mount_path.join(ws_id);
        let snap_dir = self.snapshots_dir.join(ws_id);

        if let Err(e) = self
            .do_init_storage(&resolved_str, ws_id, &subvol_path, &snap_dir)
            .await
        {
            error!("init_workspace storage failed, cleaning up: {:#}", e);
            Self::cleanup_init_storage(&resolved_str, &subvol_path, &snap_dir).await;
            return Err(e);
        }

        Ok(WorkspaceInfo {
            ws_id: ws_id.to_string(),
            path: resolved_str,
            snapshot_count: 0,
        })
    }

    async fn create_snapshot(&self, ws_id: &str, snapshot_id: &str) -> anyhow::Result<()> {
        let ws_subvol = self.mount_path.join(ws_id);
        let snap_path = self.snapshots_dir.join(ws_id).join(snapshot_id);
        btrfs_common::create_snapshot(&ws_subvol, &snap_path, true).await
    }

    async fn rollback(&self, ws_id: &str, snapshot_id: &str) -> anyhow::Result<PathBuf> {
        let ws_path = self.mount_path.join(ws_id);
        let tmp_path = self.mount_path.join(format!("{}.rollback-tmp", ws_id));
        let snap_path = self.snapshots_dir.join(ws_id).join(snapshot_id);

        // Verify ws_path is a real subvolume, not a symlink
        let metadata = tokio::fs::symlink_metadata(&ws_path)
            .await
            .context("Failed to read workspace metadata")?;
        if metadata.file_type().is_symlink() {
            bail!("workspace path {:?} is a symlink, expected btrfs subvolume; aborting rollback to prevent symlink chain corruption", ws_path);
        }

        // Warmup snapshot metadata cache
        btrfs_common::warmup_snapshot_metadata(&snap_path).await;

        // Move current workspace aside
        tokio::fs::rename(&ws_path, &tmp_path).await?;

        // Create writable snapshot from target
        match btrfs_common::create_snapshot(&snap_path, &ws_path, false).await {
            Ok(()) => {}
            Err(e) => {
                // Rollback protection: restore original workspace
                error!("rollback snapshot failed, restoring original: {}", e);
                tokio::fs::rename(&tmp_path, &ws_path).await?;
                return Err(e);
            }
        }

        // Clean up old subvolume (non-fatal)
        if let Err(e) = btrfs_common::delete_subvolume(&tmp_path).await {
            warn!("failed to delete old subvolume (non-fatal): {}", e);
        }

        Ok(ws_path)
    }

    async fn delete_snapshot(&self, ws_id: &str, snapshot_id: &str) -> anyhow::Result<()> {
        let snap_path = self.snapshots_dir.join(ws_id).join(snapshot_id);
        btrfs_common::delete_subvolume(&snap_path).await
    }

    async fn recover_workspace(&self, ws_id: &str, original_path: &str) -> anyhow::Result<()> {
        let subvol_path = self.mount_path.join(ws_id);
        let snap_base = self.snapshots_dir.join(ws_id);

        // 1. Remove symlink (skip if not a symlink)
        let is_symlink = match tokio::fs::symlink_metadata(original_path).await {
            Ok(meta) => meta.file_type().is_symlink(),
            Err(_) => false,
        };
        if is_symlink {
            tokio::fs::remove_file(original_path)
                .await
                .context("failed to remove symlink")?;
        }

        // 2. Rsync subvolume contents back to original path (restore as normal directory)
        let src = format!("{}/", subvol_path.to_string_lossy()); // trailing / is important

        // Record subvolume root permissions before rsync
        let subvol_meta = tokio::fs::metadata(&subvol_path)
            .await
            .context("failed to read subvolume metadata")?;
        let sv_uid = subvol_meta.uid();
        let sv_gid = subvol_meta.gid();
        let sv_mode = subvol_meta.mode();

        let rsync_status = Command::new("rsync")
            .args(["-a", "--delete", &src, original_path])
            .status()
            .await
            .context("failed to run rsync")?;
        if !rsync_status.success() {
            error!(
                "rsync failed restoring {} -> {}, exit: {:?}",
                src,
                original_path,
                rsync_status.code()
            );
        } else {
            // Restore directory ownership and permissions to match original
            if let Err(e) = chown(
                Path::new(original_path),
                Some(Uid::from_raw(sv_uid)),
                Some(Gid::from_raw(sv_gid)),
            ) {
                warn!("failed to restore ownership on {}: {}", original_path, e);
            }
            if let Err(e) =
                tokio::fs::set_permissions(original_path, std::fs::Permissions::from_mode(sv_mode))
                    .await
            {
                warn!("failed to restore permissions on {}: {}", original_path, e);
            }
            info!("restored workspace contents to {}", original_path);
        }

        // 3. Delete all snapshot subvolumes by scanning the filesystem directory
        if let Ok(mut entries) = tokio::fs::read_dir(&snap_base).await {
            while let Ok(Some(entry)) = entries.next_entry().await {
                let path = entry.path();
                if path.is_dir() {
                    if let Err(e) = btrfs_common::delete_subvolume(&path).await {
                        warn!("failed to delete snapshot subvolume {:?}: {:#}", path, e);
                    }
                }
            }
        }

        // 4. Delete workspace subvolume
        if let Err(e) = btrfs_common::delete_subvolume(&subvol_path).await {
            warn!("failed to delete workspace subvolume {}: {:#}", ws_id, e);
        }

        // 5. Remove snapshots/{ws_id} directory
        if let Err(e) = tokio::fs::remove_dir_all(&snap_base).await {
            warn!("failed to remove snapshots dir {:?}: {}", snap_base, e);
        }

        Ok(())
    }

    async fn diff(&self, ws_id: &str, from: &str, to: &str) -> anyhow::Result<Vec<DiffEntry>> {
        let snap_base = self.snapshots_dir.join(ws_id);
        let snap_from = snap_base.join(from);
        let snap_to = snap_base.join(to);
        btrfs_common::diff_between_snapshots(&snap_from, &snap_to).await
    }

    async fn cleanup_snapshots(
        &self,
        ws_id: &str,
        snapshot_ids: &[String],
    ) -> anyhow::Result<Vec<String>> {
        let snap_dir = self.snapshots_dir.join(ws_id);
        let mut removed = Vec::new();
        for snap_id in snapshot_ids {
            let snap_path = snap_dir.join(snap_id);
            match btrfs_common::delete_subvolume(&snap_path).await {
                Ok(()) => {
                    removed.push(snap_id.clone());
                    info!("cleanup: removed snapshot {}", snap_id);
                }
                Err(e) => {
                    warn!("cleanup: failed to delete snapshot {}: {:#}", snap_id, e);
                }
            }
        }
        Ok(removed)
    }

    async fn fork(&self, ws_id: &str, snapshot_id: &str, new_ws_id: &str) -> anyhow::Result<()> {
        let snap_path = self.snapshots_dir.join(ws_id).join(snapshot_id);
        let new_ws_path = self.mount_path.join(new_ws_id);
        btrfs_common::create_snapshot(&snap_path, &new_ws_path, false).await
    }

    async fn gc_generations(&self, _ws_id: &str) -> anyhow::Result<GcResult> {
        Ok(GcResult::default())
    }

    async fn check_environment(&self) -> anyhow::Result<EnvironmentStatus> {
        let mut details = Vec::new();
        let mut healthy = true;

        // Check btrfs-progs
        match Command::new("which").arg("btrfs").output().await {
            Ok(output) if output.status.success() => {
                details.push("btrfs-progs: installed".to_string())
            }
            _ => {
                healthy = false;
                details.push("btrfs-progs: NOT installed".to_string());
            }
        }

        // Check root privileges
        if nix::unistd::geteuid().is_root() {
            details.push("privileges: root".to_string());
        } else {
            healthy = false;
            details.push("privileges: NOT root".to_string());
        }

        // Check loop device availability
        match Command::new("losetup").arg("--list").output().await {
            Ok(output) if output.status.success() => {
                details.push("loop devices: available".to_string())
            }
            _ => {
                healthy = false;
                details.push("loop devices: NOT available".to_string());
            }
        }

        Ok(EnvironmentStatus {
            backend: BackendType::BtrfsLoop,
            healthy,
            details,
        })
    }

    async fn get_usage(&self) -> anyhow::Result<(u64, u64)> {
        // Any failure is treated as a real anomaly (manual umount, fs crash, etc.); attach mount path for context.
        btrfs_common::get_filesystem_usage(&self.mount_path)
            .await
            .with_context(|| {
                format!(
                    "failed to get btrfs filesystem usage at {}",
                    self.mount_path.display()
                )
            })
    }
}
