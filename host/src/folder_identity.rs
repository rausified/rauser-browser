//! Selected-folder identities come from an opened directory, never a
//! caller-supplied pathname. macOS device numbers are mount-session values;
//! a persistent volume UUID plus inode survives their renumbering.

use anyhow::{Context, Result, bail};
use cap_std::fs::Dir;

pub(crate) fn directory_identity(dir: &Dir, expected: Option<&str>) -> Result<String> {
    let handle = dir
        .try_clone()
        .context("cloning home folder handle")?
        .into_std_file();
    let metadata = handle.metadata().context("identifying home folder")?;
    if !metadata.is_dir() {
        bail!("selected home folder is not a directory");
    }
    #[cfg(unix)]
    {
        use std::os::unix::fs::MetadataExt;
        let legacy = unix_identity(metadata.dev(), metadata.ino());
        #[cfg(target_os = "macos")]
        {
            // Preserve a recorded legacy identity exactly. Reading an old
            // config does not authorize its replacement or silently migrate
            // its recovery receipts. Native reselection upgrades it once.
            if expected.is_some_and(|identity| identity.starts_with("unix:")) {
                return Ok(legacy);
            }
            let volume = macos::volume_uuid(&handle, &metadata)?;
            identity_from_observation(metadata.dev(), metadata.ino(), volume, expected)
        }
        #[cfg(not(target_os = "macos"))]
        {
            let _ = expected;
            Ok(legacy)
        }
    }
    #[cfg(windows)]
    {
        let _ = expected;
        // winapi-util owns the unsafe by-handle platform API.
        let info = winapi_util::file::information(&handle)
            .context("identifying home folder on Windows")?;
        Ok(format!(
            "windows:{:08x}:{:016x}",
            info.volume_serial_number(),
            info.file_index()
        ))
    }
}

/// Recovery may encounter obsolete or malformed foreign-folder records.
/// They are a non-match, not a reason to fail another folder's lookup.
pub(crate) fn has_known_shape(identity: &str) -> bool {
    let mut parts = identity.split(':');
    let Some(kind) = parts.next() else {
        return false;
    };
    let Some(volume) = parts.next() else {
        return false;
    };
    let Some(file) = parts.next() else {
        return false;
    };
    if parts.next().is_some()
        || file.len() != 16
        || !file.bytes().all(|byte| byte.is_ascii_hexdigit())
    {
        return false;
    }
    match kind {
        "unix" => volume.len() == 16 && volume.bytes().all(|byte| byte.is_ascii_hexdigit()),
        "windows" => volume.len() == 8 && volume.bytes().all(|byte| byte.is_ascii_hexdigit()),
        "macos" => uuid::Uuid::parse_str(volume)
            .is_ok_and(|value| !value.is_nil() && value.hyphenated().to_string() == volume),
        _ => false,
    }
}

#[cfg(any(unix, test))]
fn unix_identity(device: u64, inode: u64) -> String {
    format!("unix:{device:016x}:{inode:016x}")
}

/// No persistent UUID means an explicit legacy selection on an unsupported
/// filesystem. A stable identity that cannot be read is never downgraded.
#[cfg(any(target_os = "macos", test))]
fn identity_from_observation(
    device: u64,
    inode: u64,
    volume: Option<[u8; 16]>,
    expected: Option<&str>,
) -> Result<String> {
    if expected.is_some_and(|identity| identity.starts_with("unix:")) {
        return Ok(unix_identity(device, inode));
    }
    match volume {
        Some(uuid) => Ok(format!(
            "macos:{}:{inode:016x}",
            uuid::Uuid::from_bytes(uuid)
        )),
        None if expected.is_some_and(|identity| identity.starts_with("macos:")) => {
            bail!("the selected folder's persistent volume identity is unavailable")
        }
        None => Ok(unix_identity(device, inode)),
    }
}

#[cfg(target_os = "macos")]
mod macos {
    use super::*;
    use std::fs::{self, File, Metadata};
    use std::os::unix::ffi::OsStrExt;
    use std::os::unix::fs::{FileTypeExt, MetadataExt};
    use std::path::Path;

    /// Bind the safe reader's own observation to our held descriptor. The
    /// device-node check is necessary: pathname checks before and after a
    /// read alone allow another volume to occupy a path during that read.
    pub(super) fn volume_uuid(handle: &File, opened: &Metadata) -> Result<Option<[u8; 16]>> {
        let filesystem =
            rustix::fs::fstatfs(handle).context("identifying the opened home folder's mount")?;
        let native_path = rustix::fs::getpath(handle).context("locating the opened home folder")?;
        let path = Path::new(std::ffi::OsStr::from_bytes(native_path.to_bytes()));
        same_directory(
            opened,
            &fs::metadata(path).context("checking the opened home folder path")?,
        )?;
        let observed =
            whichdisk::resolve(path).context("reading the home folder's volume identity")?;
        // APFS's sealed and data mounts can share a device number. Bind both
        // mount names as well, instead of treating st_dev alone as a mount.
        let source = terminated_bytes(&filesystem.f_mntfromname);
        let mounted_at = terminated_bytes(&filesystem.f_mntonname);
        if !mount_names_match(
            &source,
            &mounted_at,
            observed.device().as_bytes(),
            observed.mount_point().as_os_str().as_bytes(),
        ) {
            bail!("the home folder's mount changed while reading its identity");
        }
        let reading = observed.volume_identity();
        let uuid = match reading {
            Some(reading) if !reading.is_vouched() => {
                bail!("the home folder's volume identity could not be verified")
            }
            Some(reading) => match reading.identity() {
                whichdisk::VolumeIdentity::FsUuid(uuid) if uuid != [0; 16] => Some(uuid),
                _ => None,
            },
            None => None,
        };
        let source = Path::new(observed.device());
        let uuid = if uuid.is_some() && source.starts_with("/dev") {
            let node =
                fs::symlink_metadata(source).context("checking the home folder's volume device")?;
            if !(node.file_type().is_block_device() || node.file_type().is_char_device())
                || node.rdev() != opened.dev()
            {
                bail!("the home folder's volume changed while reading its identity");
            }
            uuid
        } else {
            // Network/pseudo-filesystems do not have a device node that can
            // bind this independent observation to the opened directory.
            None
        };
        same_directory(
            opened,
            &handle
                .metadata()
                .context("rechecking the opened home folder")?,
        )?;
        let current_mount =
            rustix::fs::fstatfs(handle).context("rechecking the opened home folder's mount")?;
        if filesystem.f_type != current_mount.f_type
            || filesystem.f_mntfromname != current_mount.f_mntfromname
            || filesystem.f_mntonname != current_mount.f_mntonname
        {
            bail!("the home folder's mount changed while reading its identity");
        }
        same_directory(
            opened,
            &fs::metadata(observed.canonical_path()).context("rechecking the home folder path")?,
        )?;
        Ok(uuid)
    }

    fn terminated_bytes(value: &[std::os::raw::c_char]) -> Vec<u8> {
        value
            .iter()
            .map(|byte| *byte as u8)
            .take_while(|byte| *byte != 0)
            .collect()
    }

    fn mount_names_match(
        held_source: &[u8],
        held_mountpoint: &[u8],
        observed_source: &[u8],
        observed_mountpoint: &[u8],
    ) -> bool {
        held_source == observed_source && held_mountpoint == observed_mountpoint
    }

    fn same_directory(expected: &Metadata, actual: &Metadata) -> Result<()> {
        if !actual.is_dir() || (expected.dev(), expected.ino()) != (actual.dev(), actual.ino()) {
            bail!("the selected home folder changed while reading its identity");
        }
        Ok(())
    }

    #[cfg(test)]
    mod tests {
        use super::mount_names_match;

        #[test]
        fn an_observation_is_bound_to_both_names_of_the_open_mount() {
            assert!(mount_names_match(
                b"/dev/disk3s5",
                b"/Data",
                b"/dev/disk3s5",
                b"/Data"
            ));
            assert!(!mount_names_match(
                b"/dev/disk3s5",
                b"/Data",
                b"/dev/disk3s1",
                b"/Data"
            ));
            assert!(!mount_names_match(
                b"/dev/disk3s5",
                b"/Data",
                b"/dev/disk3s5",
                b"/replacement"
            ));
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const VOLUME: [u8; 16] = [0x51; 16];

    #[test]
    fn stable_identity_survives_device_renumbering_but_rejects_another_volume_or_inode() {
        let expected = identity_from_observation(14, 42, Some(VOLUME), None).unwrap();
        for device in [0, 14, 18, u64::MAX] {
            assert_eq!(
                identity_from_observation(device, 42, Some(VOLUME), Some(&expected)).unwrap(),
                expected
            );
        }
        assert_ne!(
            identity_from_observation(14, 43, Some(VOLUME), Some(&expected)).unwrap(),
            expected
        );
        assert_ne!(
            identity_from_observation(14, 42, Some([0x52; 16]), Some(&expected)).unwrap(),
            expected
        );
    }

    #[test]
    fn a_legacy_identity_keeps_its_exact_device_and_inode_check() {
        let expected = unix_identity(14, 42);
        assert_eq!(
            identity_from_observation(14, 42, Some(VOLUME), Some(&expected)).unwrap(),
            expected
        );
        assert_ne!(
            identity_from_observation(18, 42, Some(VOLUME), Some(&expected)).unwrap(),
            expected
        );
        assert_ne!(
            identity_from_observation(14, 43, Some(VOLUME), Some(&expected)).unwrap(),
            expected
        );
    }

    #[test]
    fn an_unavailable_persistent_identity_never_downgrades_a_stable_selection() {
        let expected = identity_from_observation(14, 42, Some(VOLUME), None).unwrap();
        assert!(identity_from_observation(14, 42, None, Some(&expected)).is_err());
        assert_eq!(
            identity_from_observation(14, 42, None, None).unwrap(),
            unix_identity(14, 42)
        );
    }

    #[test]
    fn malformed_recovery_identities_are_not_candidates_for_compatibility() {
        for identity in [
            "",
            "unix:0:0",
            "macos:not-a-volume:0000000000000001",
            "macos:00000000-0000-0000-0000-000000000000:0000000000000001",
        ] {
            assert!(!has_known_shape(identity), "{identity}");
        }
        assert!(has_known_shape(&unix_identity(14, 42)));
        assert!(has_known_shape(
            &identity_from_observation(14, 42, Some(VOLUME), None).unwrap()
        ));
    }

    #[cfg(target_os = "macos")]
    #[test]
    fn an_apfs_selection_binds_the_held_directory_and_legacy_identity() {
        use cap_std::ambient_authority;
        use std::fs;
        use std::os::unix::fs::MetadataExt;
        let temporary = tempfile::tempdir().unwrap();
        let path = temporary.path().join("selected");
        fs::create_dir(&path).unwrap();
        let root = Dir::open_ambient_dir(&path, ambient_authority()).unwrap();
        let metadata = root
            .try_clone()
            .unwrap()
            .into_std_file()
            .metadata()
            .unwrap();
        let legacy = unix_identity(metadata.dev(), metadata.ino());
        let stable = directory_identity(&root, None).unwrap();
        if whichdisk::resolve(&path).unwrap().fs_type() == "apfs" {
            assert!(
                stable.starts_with("macos:"),
                "APFS must report its persistent UUID"
            );
        }
        assert_eq!(directory_identity(&root, Some(&legacy)).unwrap(), legacy);
        assert!(crate::vault::root_identities_match(&root, &legacy, &stable).unwrap());
        assert!(
            !crate::vault::root_identities_match(
                &root,
                &unix_identity(metadata.dev() + 1, metadata.ino()),
                &stable
            )
            .unwrap()
        );
        fs::rename(&path, temporary.path().join("moved")).unwrap();
        fs::create_dir(&path).unwrap();
        assert_eq!(directory_identity(&root, Some(&stable)).unwrap(), stable);
        let failure = crate::vault::open_selected_root(&path, Some(&stable)).unwrap_err();
        assert!(
            failure
                .downcast_ref::<crate::vault::SelectedRootChanged>()
                .is_some()
        );
    }
}
