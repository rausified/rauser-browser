//! Host-owned, picker-confirmed task destinations. These never enter a
//! writable ConfigSnapshot; ordinary settings saves carry them forward.
use super::*;

pub(crate) const MAX_TASK_FOLDERS: usize = 16;

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct TaskFolder {
    pub id: String,
    pub path: String,
    pub identity: String,
}

pub(super) fn validate(folders: &[TaskFolder]) -> Result<()> {
    if folders.len() > MAX_TASK_FOLDERS {
        bail!("at most 16 task folders can be configured");
    }
    let mut ids = std::collections::HashSet::new();
    let mut paths = std::collections::HashSet::new();
    for folder in folders {
        if uuid::Uuid::parse_str(&folder.id).is_err()
            || !ids.insert(&folder.id)
            || !paths.insert(&folder.path)
            || folder.path.len() > MAX_ROOT_PATH_BYTES
            || !Path::new(&folder.path).is_absolute()
            || has_unsafe_display_chars(&folder.path)
            || folder.identity.is_empty()
        {
            bail!("invalid configured task folder; choose the folder again in settings");
        }
    }
    Ok(())
}

impl ConfigStore {
    pub(crate) fn task_folder_choices(&self) -> Vec<brauser_protocol::TaskFolderChoice> {
        self.task_folders
            .iter()
            .map(|folder| brauser_protocol::TaskFolderChoice {
                id: folder.id.clone(),
                path: folder.path.clone(),
            })
            .collect()
    }

    /// Call after refreshing under the config lock. Missing/replaced folders
    /// fail only the task that selects them, not unrelated configuration saves.
    #[cfg(any(unix, test))]
    pub(crate) fn task_folder(&self, id: &str) -> Result<PathBuf> {
        let folder = self
            .task_folders
            .iter()
            .find(|folder| folder.id == id)
            .context(
                "This task folder was removed. Choose a working folder before running again.",
            )?;
        let path = Path::new(&folder.path);
        let actual = selected_root_identity(path).context(
            "This task folder is unavailable. Reconnect it or choose it again in settings.",
        )?;
        if actual != folder.identity || fs::canonicalize(path)? != path {
            bail!("This task folder changed. Remove it and choose it again in settings.");
        }
        Ok(path.to_path_buf())
    }

    pub(crate) fn add_task_folder(
        &mut self,
        path: String,
        identity: String,
        expected: &str,
    ) -> Result<bool> {
        self.change_task_folders(expected, move |folders| {
            // Recheck under the write lock, after checking the picker grant.
            if selected_root_identity(Path::new(&path))? != identity {
                bail!("selected task folder changed; choose it again");
            }
            if folders.iter().any(|folder| folder.path == path) {
                bail!("this task folder is already configured");
            }
            folders.push(TaskFolder {
                id: uuid::Uuid::new_v4().to_string(),
                path,
                identity,
            });
            Ok(())
        })
    }

    pub(crate) fn remove_task_folder(&mut self, id: &str, expected: &str) -> Result<bool> {
        self.change_task_folders(expected, |folders| {
            let index = folders
                .iter()
                .position(|folder| folder.id == id)
                .context("task folder is no longer configured")?;
            folders.remove(index);
            Ok(())
        })
    }

    fn change_task_folders(
        &mut self,
        expected: &str,
        change: impl FnOnce(&mut Vec<TaskFolder>) -> Result<()>,
    ) -> Result<bool> {
        let _lock = self.lock_current()?;
        let current = read_disk_state(&self.path)?;
        if expected != current.revision {
            self.adopt(current);
            return Ok(false);
        }
        if current.issue.is_some() || current.needs_backup || current.config.storage.is_none() {
            bail!("repair the home folder configuration before changing task folders");
        }
        let mut folders = current.task_folders.clone();
        change(&mut folders)?;
        validate(&folders)?;
        let serialized = toml::to_string_pretty(&StoredConfig::from_snapshot(
            &current.config,
            current.root_identity.clone(),
            current.knowledge_base_identity.clone(),
            &current.agent,
            current.commands.clone(),
            current.retired_adapter_contracts.clone(),
            current.adapter_extensions.clone(),
            folders,
        ))?;
        if serialized.len() > MAX_CONFIG_BYTES {
            return Err(config_size_refusal(0).into());
        }
        // A local editor can ignore the lock. Never replace unreviewed bytes.
        if read_disk_state(&self.path)?.revision != expected {
            self.refresh()?;
            return Ok(false);
        }
        publish_config(
            self.path.parent().context("config path has no parent")?,
            &self.path,
            &serialized,
        )?;
        self.adopt(read_disk_state(&self.path)?);
        Ok(true)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn configured(dir: &Path) -> ConfigStore {
        let root = dir.join("home");
        fs::create_dir(&root).unwrap();
        let mut store = ConfigStore::for_test(dir.join("config.toml"));
        let next = ConfigSnapshot {
            storage: Some(brauser_protocol::StorageConfig {
                root: root.display().to_string(),
                profile: "neutral".into(),
                log_dir: "logs".into(),
                pages_dir: "notes".into(),
                later_dir: "later".into(),
                artifacts_dir: "artifacts".into(),
                summaries_dir: Some("summaries".into()),
                knowledge_base_dir: None,
            }),
            ..empty_config()
        };
        store
            .update_with_root_identity(
                next,
                "missing",
                Some(&selected_root_identity(&root).unwrap()),
            )
            .unwrap()
            .unwrap();
        store
    }

    #[test]
    fn task_folders_survive_ordinary_saves_and_only_expose_public_choices() {
        let dir = tempfile::tempdir().unwrap();
        let mut store = configured(dir.path());
        let project = dir.path().join("project");
        fs::create_dir(&project).unwrap();
        let project = fs::canonicalize(project).unwrap();
        let rev = store.revision().to_owned();
        assert!(
            store
                .add_task_folder(
                    project.display().to_string(),
                    selected_root_identity(&project).unwrap(),
                    &rev
                )
                .unwrap()
        );
        let choices = store.task_folder_choices();
        assert_eq!(choices.len(), 1);
        let wire = serde_json::to_value(&choices).unwrap();
        assert_eq!(wire[0].as_object().unwrap().len(), 2);
        assert_eq!(store.task_folder(&choices[0].id).unwrap(), project);
        let mut next = store.snapshot().clone();
        next.auto_summary.enabled = false;
        let rev = store.revision().to_owned();
        store.update(next, &rev).unwrap().unwrap();
        let mut loaded = store.reopen();
        loaded.refresh().unwrap();
        assert_eq!(loaded.task_folder_choices(), choices);
        assert_eq!(loaded.task_folder(&choices[0].id).unwrap(), project);
        assert!(
            !serde_json::to_string(store.snapshot())
                .unwrap()
                .contains("task_folders")
        );
    }

    #[test]
    fn task_folder_changes_are_revision_bound_and_readding_never_revives_an_id() {
        let dir = tempfile::tempdir().unwrap();
        let mut store = configured(dir.path());
        let project = dir.path().join("project");
        fs::create_dir(&project).unwrap();
        let old = store.revision().to_owned();
        let path = project.display().to_string();
        let identity = selected_root_identity(&project).unwrap();
        assert!(
            store
                .add_task_folder(path.clone(), identity.clone(), &old)
                .unwrap()
        );
        let id = store.task_folder_choices()[0].id.clone();
        let before = fs::read(&store.path).unwrap();
        assert!(!store.remove_task_folder(&id, &old).unwrap());
        assert_eq!(before, fs::read(&store.path).unwrap());
        let rev = store.revision().to_owned();
        assert!(
            store
                .add_task_folder(path.clone(), identity.clone(), &rev)
                .is_err()
        );
        assert!(store.remove_task_folder(&id, &rev).unwrap());
        assert!(project.is_dir());
        let rev = store.revision().to_owned();
        assert!(store.add_task_folder(path, identity, &rev).unwrap());
        assert_ne!(store.task_folder_choices()[0].id, id);
        assert!(store.task_folder(&id).is_err());
    }

    #[test]
    fn unavailable_or_replaced_task_folders_never_block_unrelated_settings() {
        let dir = tempfile::tempdir().unwrap();
        let mut store = configured(dir.path());
        let project = dir.path().join("project");
        fs::create_dir(&project).unwrap();
        let project = fs::canonicalize(project).unwrap();
        let rev = store.revision().to_owned();
        store
            .add_task_folder(
                project.display().to_string(),
                selected_root_identity(&project).unwrap(),
                &rev,
            )
            .unwrap();
        let id = store.task_folder_choices()[0].id.clone();
        // Retain the original inode so the replacement cannot reuse it.
        fs::rename(&project, dir.path().join("original")).unwrap();
        assert!(store.task_folder(&id).is_err());
        fs::create_dir(&project).unwrap();
        assert!(store.task_folder(&id).is_err());
        let next = store.snapshot().clone();
        let rev = store.revision().to_owned();
        store.update(next, &rev).unwrap().unwrap();
        assert_eq!(store.task_folder_choices()[0].id, id);
        assert!(store.task_folder("/arbitrary/path").is_err());
    }

    #[test]
    fn invalid_task_folder_entries_can_be_backed_up_and_repaired() {
        for entry in [
            "id = 'not-a-uuid'\npath = '/project'\nidentity = 'picked'",
            "id = '00000000-0000-4000-8000-000000000001'\npath = 'relative'\nidentity = 'picked'",
            "id = '00000000-0000-4000-8000-000000000001'\npath = '/project'\nidentity = ''",
        ] {
            let dir = tempfile::tempdir().unwrap();
            let mut store = configured(dir.path());
            let next = store.snapshot().clone();
            let root = Path::new(&next.storage.as_ref().unwrap().root);
            let original = format!(
                "{}\n[[task_folders]]\n{entry}\n",
                fs::read_to_string(&store.path).unwrap()
            );
            fs::write(&store.path, &original).unwrap();

            // A readable invalid entry must keep a real revision, so the
            // native picker and ordinary revision-checked repair still work.
            store.refresh().unwrap();
            assert!(store.config_issue().is_some());
            assert_ne!(store.revision(), UNREADABLE_REVISION);
            assert!(store.task_folder_choices().is_empty());
            let revision = store.revision().to_owned();
            store
                .update_with_root_identity(
                    next.clone(),
                    &revision,
                    Some(&selected_root_identity(root).unwrap()),
                )
                .unwrap()
                .unwrap();
            assert!(store.config_issue().is_none());
            assert_eq!(store.snapshot(), &next);
            let backups: Vec<_> = fs::read_dir(dir.path())
                .unwrap()
                .map(|entry| entry.unwrap().path())
                .filter(|path| {
                    path.file_name()
                        .unwrap()
                        .to_string_lossy()
                        .starts_with("config-invalid-")
                })
                .collect();
            assert_eq!(backups.len(), 1);
            assert_eq!(fs::read_to_string(&backups[0]).unwrap(), original);
        }
    }

    #[test]
    fn task_folder_bound_refuses_the_seventeenth_without_writing() {
        let dir = tempfile::tempdir().unwrap();
        let mut store = configured(dir.path());
        for n in 0..=MAX_TASK_FOLDERS {
            let project = dir.path().join(format!("p{n}"));
            fs::create_dir(&project).unwrap();
            let rev = store.revision().to_owned();
            let before = fs::read(&store.path).unwrap();
            let result = store.add_task_folder(
                project.display().to_string(),
                selected_root_identity(&project).unwrap(),
                &rev,
            );
            if n == MAX_TASK_FOLDERS {
                assert!(result.is_err());
                assert_eq!(before, fs::read(&store.path).unwrap());
            } else {
                assert!(result.unwrap());
            }
        }
    }
}
