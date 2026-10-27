//! Locations inside the service's private support directory, and the build
//! identity the host compares before forwarding to a running service.

use std::fs;
use std::io::ErrorKind;
use std::os::unix::fs::{DirBuilderExt, MetadataExt, PermissionsExt};
use std::path::PathBuf;

use anyhow::{Context, Result, bail};

use crate::brand::{AGENT_ENV_NAME, RUNS_DIR, SOCKET_NAME};
use crate::config::ConfigStore;

/// macOS `sun_path` holds 104 bytes including the trailing NUL.
const SUN_PATH_CAPACITY: usize = 104;

/// The service's support directory: the directory holding `config.toml`.
#[derive(Debug, Clone)]
pub struct ServicePaths {
    support: PathBuf,
}

impl ServicePaths {
    pub fn for_config(config: &ConfigStore) -> Result<Self> {
        Ok(Self::at(config.dir()?.to_path_buf()))
    }

    pub fn at(support: PathBuf) -> Self {
        Self { support }
    }

    /// Create the support directory if needed. It must be a real directory
    /// owned by this user, never a symlink; its mode is narrowed to 0700.
    pub fn ensure_private(&self) -> Result<()> {
        let support = &self.support;
        let uid = rustix::process::getuid().as_raw();
        match fs::symlink_metadata(support) {
            Ok(metadata) => {
                if metadata.file_type().is_symlink() {
                    bail!("{} is a symlink, not a directory", support.display());
                }
                if !metadata.is_dir() {
                    bail!("{} is not a directory", support.display());
                }
                if metadata.uid() != uid {
                    bail!("{} is owned by another user", support.display());
                }
            }
            Err(error) if error.kind() == ErrorKind::NotFound => {
                if let Some(parent) = support.parent() {
                    fs::create_dir_all(parent)
                        .with_context(|| format!("creating {}", parent.display()))?;
                }
                match fs::DirBuilder::new().mode(0o700).create(support) {
                    // Another process created it first; check it again.
                    Err(error) if error.kind() == ErrorKind::AlreadyExists => {
                        return self.ensure_private();
                    }
                    result => result.with_context(|| format!("creating {}", support.display()))?,
                }
            }
            Err(error) => {
                return Err(error).with_context(|| format!("reading {}", support.display()));
            }
        }
        fs::set_permissions(support, fs::Permissions::from_mode(0o700))
            .with_context(|| format!("securing {}", support.display()))
    }

    /// The Unix socket path, refused when it cannot fit in `sun_path`.
    pub fn socket(&self) -> Result<PathBuf> {
        let socket = self.support.join(SOCKET_NAME);
        if socket.as_os_str().len() >= SUN_PATH_CAPACITY {
            bail!("support directory path is too long for a Unix socket");
        }
        Ok(socket)
    }

    pub fn runs(&self) -> PathBuf {
        self.support.join(RUNS_DIR)
    }

    pub fn agent_env(&self) -> PathBuf {
        self.support.join(AGENT_ENV_NAME)
    }

    pub fn lock(&self) -> PathBuf {
        self.support.join("service.lock")
    }

    pub fn pid(&self) -> PathBuf {
        self.support.join("service.pid")
    }
}

/// `"<version>:<dev>:<ino>:<mtime_ns>"` of the canonical running executable.
/// The host and the service run the same binary path, so equal ids mean the
/// same build. When the executable cannot be inspected, the id carries this
/// process's pid instead, so it never matches another process's id.
pub fn build_id() -> String {
    let version = env!("CARGO_PKG_VERSION");
    let metadata = std::env::current_exe()
        .and_then(fs::canonicalize)
        .and_then(fs::metadata);
    match metadata {
        Ok(metadata) => {
            let mtime_ns =
                i128::from(metadata.mtime()) * 1_000_000_000 + i128::from(metadata.mtime_nsec());
            format!("{version}:{}:{}:{mtime_ns}", metadata.dev(), metadata.ino())
        }
        Err(_) => format!("{version}:unavailable:{}", std::process::id()),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::brand::SERVICE_LABEL;
    use std::os::unix::fs::PermissionsExt;
    use std::path::PathBuf;

    #[test]
    fn ensure_private_narrows_mode_to_0700() {
        let temp = tempfile::tempdir().unwrap();
        let support = temp.path().join("support");
        std::fs::create_dir(&support).unwrap();
        std::fs::set_permissions(&support, std::fs::Permissions::from_mode(0o755)).unwrap();
        ServicePaths::at(support.clone()).ensure_private().unwrap();
        let mode = std::fs::metadata(&support).unwrap().permissions().mode() & 0o777;
        assert_eq!(mode, 0o700);

        let fresh = temp.path().join("nested").join("fresh");
        ServicePaths::at(fresh.clone()).ensure_private().unwrap();
        let mode = std::fs::metadata(&fresh).unwrap().permissions().mode() & 0o777;
        assert_eq!(mode, 0o700);
    }

    #[test]
    fn ensure_private_refuses_a_symlinked_support_dir() {
        let temp = tempfile::tempdir().unwrap();
        let real = temp.path().join("real");
        std::fs::create_dir(&real).unwrap();
        std::fs::set_permissions(&real, std::fs::Permissions::from_mode(0o755)).unwrap();
        let link = temp.path().join("link");
        std::os::unix::fs::symlink(&real, &link).unwrap();
        assert!(ServicePaths::at(link).ensure_private().is_err());
        // The symlink's target is left untouched.
        let mode = std::fs::metadata(&real).unwrap().permissions().mode() & 0o777;
        assert_eq!(mode, 0o755);
    }

    #[test]
    fn socket_path_over_103_bytes_is_refused() {
        let long = PathBuf::from(format!("/tmp/{}", "a".repeat(100)));
        assert!(
            ServicePaths::at(long)
                .socket()
                .unwrap_err()
                .to_string()
                .contains("too long")
        );
        let short = ServicePaths::at(PathBuf::from("/tmp/x"));
        assert_eq!(
            short.socket().unwrap(),
            PathBuf::from("/tmp/x/service.sock")
        );
    }

    #[test]
    fn named_paths_live_in_the_support_dir() {
        let paths = ServicePaths::at(PathBuf::from("/s"));
        assert_eq!(paths.runs(), PathBuf::from("/s/runs"));
        assert_eq!(paths.agent_env(), PathBuf::from("/s/agent.env"));
        assert_eq!(paths.lock(), PathBuf::from("/s/service.lock"));
        assert_eq!(paths.pid(), PathBuf::from("/s/service.pid"));
    }

    #[test]
    fn build_id_is_stable_within_a_process_and_names_the_version() {
        assert_eq!(build_id(), build_id());
        assert!(build_id().starts_with(env!("CARGO_PKG_VERSION")));
    }

    #[test]
    fn label_is_reverse_dns_and_matches_brand_ts() {
        assert_eq!(SERVICE_LABEL, "com.rauser.brauser.service");
        assert!(
            include_str!("../../../extension/brand.ts")
                .contains(r#"SERVICE_LABEL = "com.rauser.brauser.service""#)
        );
    }
}
