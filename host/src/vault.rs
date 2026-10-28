//! Capability-scoped vault access. The extension never supplies a filesystem
//! path. Page notes are replaced whole under a caller-checked version and
//! ownership (§4.4); callers own that check, this module trusts it.

use std::fs;
use std::io::{self, Read, Write};
use std::path::{Component, Path, PathBuf};
use std::time::{Duration, SystemTime};

use crate::brand::NAMESPACE;
use crate::markdown::{self, LinkStyle};
use anyhow::{Context, Result, bail};
use brauser_protocol::{RecordKey, RecordKeyKind, StorageConfig};
use cap_std::ambient_authority;
use cap_std::fs::{Dir, OpenOptions};
use sha2::{Digest, Sha256};
use url::Url;
use uuid::Uuid;

const MAX_NOTE_BYTES: usize = 4 * 1024 * 1024;
/// The largest existing Brauser-owned file `replace_owned_file` will read to
/// check ownership. Anything larger is a conflict, never adopted.
const MAX_OWNED_FILE_BYTES: usize = 4 * 1024 * 1024;
const MAX_NOTE_SCAN_BYTES: usize = 16 * 1024 * 1024;
const MAX_NOTE_SCAN_FILES: usize = 4_096;
/// A live save renames its temporary within one request, so one this old was
/// left by a host that was killed mid-save. The margin keeps a startup sweep
/// from racing another host process's save in progress.
pub(crate) const STALE_TEMPORARY_AGE: Duration = Duration::from_secs(60 * 60);

pub struct Vault {
    root: Dir,
    pages_dir: PathBuf,
    /// Where Brauser-owned summary files live (§5.2); absent only for an
    /// older config that never named one.
    summaries_dir: Option<PathBuf>,
}

/// A page that was published under its final name. Warnings describe work that
/// failed after publication; callers must not retry the create as if it failed.
#[derive(Debug)]
pub struct CreatedPage {
    pub relative_path: PathBuf,
    pub warnings: Vec<PostPublishWarning>,
}

#[derive(Debug)]
pub enum PostPublishWarning {
    DirectorySync(io::Error),
    /// The published file's temporary name could not be removed; the startup
    /// sweep removes it later.
    TemporaryCleanup(io::Error),
}

impl Vault {
    pub fn open(storage: &StorageConfig) -> Result<Self> {
        Self::open_checked(storage, None)
    }

    /// The identity comes from the native picker and is persisted with config.
    /// Check the opened directory handle, not just a canonicalized pathname:
    /// a folder can be replaced between those two operations.
    pub fn open_checked(storage: &StorageConfig, expected_identity: Option<&str>) -> Result<Self> {
        let chosen = Path::new(&storage.root);
        let pages_dir = checked_relative_dir(&storage.pages_dir)?;
        checked_relative_dir(&storage.log_dir)?;
        checked_relative_dir(&storage.later_dir)?;
        checked_relative_dir(&storage.artifacts_dir)?;
        let summaries_dir = storage
            .summaries_dir
            .as_deref()
            .map(checked_relative_dir)
            .transpose()?;
        let (root, _) = open_selected_root(chosen, expected_identity)?;
        Ok(Self {
            root,
            pages_dir,
            summaries_dir,
        })
    }

    /// Atomically replace (or create) the whole note. The filename is derived
    /// from the URL, never from an extension path. Callers must already have
    /// checked the caller's expected version and Brauser ownership of any
    /// existing file (§4.4); this call trusts the markdown it is given and
    /// replaces whatever currently occupies the generated name.
    pub fn replace_page(&self, page_url: &str, markdown: &str) -> Result<CreatedPage> {
        self.replace_note_with_post_publish_ops(
            &url_record_key(page_url)?,
            page_url,
            markdown,
            sync_directory,
        )
    }

    pub fn replace_note(
        &self,
        key: &RecordKey,
        canonical_url: &str,
        markdown: &str,
    ) -> Result<CreatedPage> {
        self.replace_note_with_post_publish_ops(key, canonical_url, markdown, sync_directory)
    }

    pub fn page_relative_path(&self, page_url: &str) -> Result<PathBuf> {
        Ok(self.pages_dir.join(page_filename(page_url)?))
    }

    pub fn note_relative_path(&self, key: &RecordKey, canonical_url: &str) -> Result<PathBuf> {
        Ok(self.pages_dir.join(note_filename(key, canonical_url)?))
    }

    /// The record's summary file, `<summaries_dir>/<note stem>.summary.md`.
    /// The name is a slug and a digest, and since the layout move (§6.1) the
    /// folder is the fixed `summaries`, so the path passes
    /// `summary::validate_page_summary`'s path check. A vault opened from an
    /// older config can still name another folder, which
    /// `checked_relative_dir` does not screen for link-breaking characters,
    /// so the whole path is checked here and a failing one is an error.
    pub(crate) fn summary_relative_path(
        &self,
        key: &RecordKey,
        canonical_url: &str,
    ) -> Result<PathBuf> {
        let path = self
            .summaries_dir()?
            .join(crate::summary::summary_filename(key, canonical_url)?);
        let linked = slash_path(&path).context("summary path is not UTF-8")?;
        if let Err(code) = crate::summary::check_summary_path(&linked) {
            bail!("{code}: the summaries folder cannot be linked from a summary event");
        }
        Ok(path)
    }

    /// A link from the record's summary file to its page note, in `style`:
    /// a percent-encoded relative Markdown link from the summaries folder, or
    /// a wikilink to the note's path without `.md` (`notes/<stem>`). The note
    /// need not exist yet.
    ///
    /// The wikilink names the folder (§5.2: "the page note's computed path"):
    /// Obsidian resolves a bare `[[stem]]` to whichever file of that name it
    /// finds first when several share it, and the home folder may sit inside
    /// a larger vault holding a copied or archived note of the same stem. A
    /// path-qualified link only matches a file whose path ends in
    /// `notes/<stem>.md`, and it still resolves when the home folder is a
    /// subfolder of the vault, which a vault-root path would not.
    #[cfg_attr(not(any(test, unix)), allow(dead_code))]
    pub(crate) fn note_link(
        &self,
        key: &RecordKey,
        canonical_url: &str,
        style: LinkStyle,
    ) -> Result<String> {
        let summaries_dir = self.summaries_dir()?;
        let note = note_filename(key, canonical_url)?;
        Ok(match style {
            LinkStyle::Relative => format!(
                "[Page note]({})",
                markdown::relative_link(summaries_dir, &self.pages_dir.join(&note))
            ),
            LinkStyle::Wikilink => {
                let stem = note.strip_suffix(".md").unwrap_or(&note);
                let notes = slash_path(&self.pages_dir).context("notes folder is not UTF-8")?;
                markdown::wikilink(&format!("{notes}/{stem}"), Some("Page note"))
            }
        })
    }

    /// Replace the record's summary file whole (§4.4): only an absent name or
    /// a file `summary::owned_summary` accepts for `key` is written; anything
    /// else is `OwnedWrite::Conflict` and is left untouched.
    #[cfg_attr(not(any(test, unix)), allow(dead_code))]
    pub(crate) fn write_summary(
        &self,
        key: &RecordKey,
        canonical_url: &str,
        contents: &str,
    ) -> Result<OwnedWrite> {
        let path = self.summary_relative_path(key, canonical_url)?;
        replace_owned_file(&self.root, &path, contents, "summary", &|bytes| {
            crate::summary::owned_summary(bytes, key)
        })
    }

    /// What occupies the record's summary path, read the way
    /// `write_summary` reads an occupant: only a regular file of at most
    /// 4 MiB whose bytes `summary::owned_summary` accepts for `key` is
    /// `Owned`. A vault with no summaries folder has no summary.
    pub(crate) fn find_summary(
        &self,
        key: &RecordKey,
        canonical_url: &str,
    ) -> Result<FoundSummary> {
        if self.summaries_dir.is_none() {
            return Ok(FoundSummary::Absent);
        }
        let path = self.summary_relative_path(key, canonical_url)?;
        let linked = slash_path(&path).context("summary path is not UTF-8")?;
        let parent = path.parent().context("summary path has no parent")?;
        let filename = path.file_name().context("summary path has no filename")?;
        let dir = match self.root.open_dir(parent) {
            Ok(dir) => dir,
            Err(error) if error.kind() == io::ErrorKind::NotFound => {
                return Ok(FoundSummary::Absent);
            }
            Err(error) => return Err(error).context("opening summaries directory"),
        };
        let inspected = match dir.symlink_metadata(filename) {
            Ok(inspected) => inspected,
            Err(error) if error.kind() == io::ErrorKind::NotFound => {
                return Ok(FoundSummary::Absent);
            }
            Err(error) => return Err(error).context("checking summary file"),
        };
        if inspected.file_type().is_symlink()
            || !inspected.is_file()
            || inspected.len() > MAX_OWNED_FILE_BYTES as u64
        {
            return Ok(FoundSummary::Unowned);
        }
        let occupant =
            read_inspected_occupant(&dir, filename, &inspected).context("reading summary file")?;
        Ok(match occupant {
            Some(bytes) if crate::summary::owned_summary(&bytes, key) => FoundSummary::Owned {
                relative: path,
                linked,
                summarized_at: crate::summary::summary_summarized_at(&bytes),
                file_id: file_id(&inspected),
            },
            _ => FoundSummary::Unowned,
        })
    }

    /// The owned summary at home-relative `linked` (`/` separators, as a
    /// `SummaryView` or run names it), whatever record the page's URL
    /// resolves to now. The file must be a summary of the record its own
    /// front matter names, at exactly that record's summary path, so no
    /// other file is ever found this way: anything else there is
    /// `Unowned`, and nothing there is `Absent`. `linked` must already pass
    /// `summary::check_summary_path`.
    pub(crate) fn find_summary_at(&self, linked: &str) -> Result<FoundSummary> {
        if self.summaries_dir.is_none() {
            return Ok(FoundSummary::Absent);
        }
        let path: PathBuf = linked.split('/').collect();
        let (Some(parent), Some(filename)) = (path.parent(), path.file_name()) else {
            return Ok(FoundSummary::Unowned);
        };
        let dir = match self.root.open_dir(parent) {
            Ok(dir) => dir,
            Err(error) if error.kind() == io::ErrorKind::NotFound => {
                return Ok(FoundSummary::Absent);
            }
            // Outside the home folder, or not a folder: not a summary.
            Err(_) => return Ok(FoundSummary::Unowned),
        };
        let inspected = match dir.symlink_metadata(filename) {
            Ok(inspected) => inspected,
            Err(error) if error.kind() == io::ErrorKind::NotFound => {
                return Ok(FoundSummary::Absent);
            }
            Err(error) => return Err(error).context("checking summary file"),
        };
        if inspected.file_type().is_symlink()
            || !inspected.is_file()
            || inspected.len() > MAX_OWNED_FILE_BYTES as u64
        {
            return Ok(FoundSummary::Unowned);
        }
        let Some(bytes) =
            read_inspected_occupant(&dir, filename, &inspected).context("reading summary file")?
        else {
            return Ok(FoundSummary::Unowned);
        };
        let Some((key, canonical)) = crate::summary::summary_identity(&bytes) else {
            return Ok(FoundSummary::Unowned);
        };
        let expected = self.summary_relative_path(&key, &canonical).ok();
        if expected.as_deref().and_then(slash_path).as_deref() != Some(linked) {
            return Ok(FoundSummary::Unowned);
        }
        // Read again by its record, with the ownership check.
        self.find_summary(&key, &canonical)
    }

    fn summaries_dir(&self) -> Result<&Path> {
        self.summaries_dir
            .as_deref()
            .context("no summaries folder is configured")
    }

    /// Read a page generated from the same canonical URL. Callers must check
    /// ownership and identity themselves before treating an existing file as
    /// an owned note (§6.4).
    pub fn read_page(&self, page_url: &str) -> Result<Option<String>> {
        let filename = page_filename(page_url)?;
        self.read_named(&filename)
    }

    pub fn read_note(&self, key: &RecordKey, canonical_url: &str) -> Result<Option<String>> {
        let filename = note_filename(key, canonical_url)?;
        self.read_named(&filename)
    }

    /// Bounded candidates for reconciling M1 URL-keyed notes with current
    /// adapter keys. Notes are flat in `pages_dir`; symlinks and large files
    /// are never followed or read. Older-note lists may omit candidates, so
    /// unreadable or oversized files and anything past the budget are skipped.
    pub fn note_candidates(&self) -> Result<Vec<String>> {
        let pages = match self.root.open_dir(&self.pages_dir) {
            Ok(dir) => dir,
            Err(error) if error.kind() == io::ErrorKind::NotFound => return Ok(Vec::new()),
            Err(error) => return Err(error).context("opening page notes directory"),
        };
        let mut names = Vec::new();
        for entry in pages.entries().context("listing page notes directory")? {
            let Ok(entry) = entry else {
                continue;
            };
            let Ok(name) = entry.file_name().into_string() else {
                continue;
            };
            if name.ends_with(".md") {
                names.push(name);
            }
        }
        names.sort();
        names.truncate(MAX_NOTE_SCAN_FILES);
        let mut total = 0usize;
        let mut contents = Vec::new();
        for name in names {
            let Ok(metadata) = pages.symlink_metadata(&name) else {
                continue;
            };
            if metadata.file_type().is_symlink() || !metadata.is_file() {
                continue;
            }
            let length = usize::try_from(metadata.len()).unwrap_or(usize::MAX);
            if length > MAX_NOTE_BYTES || total.saturating_add(length) > MAX_NOTE_SCAN_BYTES {
                continue;
            }
            // Skip unreadable candidates so one bad file cannot hide the
            // primary note.
            let Ok(Some(text)) = self.read_named(&name) else {
                continue;
            };
            // A file may grow between metadata and read; enforce the budget
            // against the bytes actually read as well.
            if total.saturating_add(text.len()) > MAX_NOTE_SCAN_BYTES {
                continue;
            }
            total += text.len();
            contents.push(text);
        }
        Ok(contents)
    }

    /// Publish a re-keyed note without replacing either side. The caller has
    /// already validated the source contents and destination ownership. A
    /// concurrent source edit aborts and removes the newly published copy.
    pub fn move_note_no_clobber(
        &self,
        source_key: &RecordKey,
        source_url: &str,
        destination_key: &RecordKey,
        destination_url: &str,
        source_contents: &str,
        destination_contents: &str,
    ) -> Result<CreatedPage> {
        self.move_note_no_clobber_with_sync(
            source_key,
            source_url,
            destination_key,
            destination_url,
            source_contents,
            destination_contents,
            sync_directory,
        )
    }

    #[allow(clippy::too_many_arguments)]
    fn move_note_no_clobber_with_sync<G>(
        &self,
        source_key: &RecordKey,
        source_url: &str,
        destination_key: &RecordKey,
        destination_url: &str,
        source_contents: &str,
        destination_contents: &str,
        sync_after_move: G,
    ) -> Result<CreatedPage>
    where
        G: FnOnce(&Dir) -> io::Result<()>,
    {
        if destination_contents.len() > MAX_NOTE_BYTES {
            bail!("page note exceeds the 4 MiB write limit");
        }
        let source = note_filename(source_key, source_url)?;
        let destination = note_filename(destination_key, destination_url)?;
        self.root.create_dir_all(&self.pages_dir)?;
        sync_directory_chain(&self.root, &self.pages_dir)?;
        let pages = self.root.open_dir(&self.pages_dir)?;
        if pages.symlink_metadata(&destination).is_ok() {
            bail!("artifact note already exists");
        }
        let temporary = format!(".{NAMESPACE}-{}.tmp", Uuid::new_v4());
        let mut file =
            pages.open_with(&temporary, OpenOptions::new().write(true).create_new(true))?;
        let cleanup = TempFileCleanup::new(&pages, &temporary);
        file.write_all(destination_contents.as_bytes())?;
        file.sync_all()?;
        drop(file);
        pages
            .hard_link(&temporary, &pages, &destination)
            .context("publishing artifact-keyed note")?;
        // The hard link is the publication point. Temporary cleanup is
        // best-effort after that; a cleanup error must not turn a published
        // destination into a reported failure that cannot be retried.
        drop(cleanup);
        let current = self.read_named(&source)?;
        if current.as_deref() != Some(source_contents) {
            if self.read_named(&destination)?.as_deref() == Some(destination_contents) {
                let _ = pages.remove_file(&destination);
            }
            bail!("older note changed while it was being re-keyed");
        }
        if let Err(error) = pages.remove_file(&source) {
            if self.read_named(&destination)?.as_deref() == Some(destination_contents) {
                let _ = pages.remove_file(&destination);
            }
            return Err(error).context("removing URL-keyed note after re-keying");
        }
        // The move is complete once the source is gone. A sync failure now
        // is a warning, as for every other post-publication step; reporting
        // it as a failure would hide a finished move behind an error.
        let mut warnings = Vec::new();
        if let Err(error) = sync_after_move(&pages) {
            warnings.push(PostPublishWarning::DirectorySync(error));
        }
        Ok(CreatedPage {
            relative_path: self.pages_dir.join(destination),
            warnings,
        })
    }

    fn read_named(&self, filename: &str) -> Result<Option<String>> {
        let pages = match self.root.open_dir(&self.pages_dir) {
            Ok(dir) => dir,
            Err(error) if error.kind() == io::ErrorKind::NotFound => return Ok(None),
            Err(error) => return Err(error).context("opening page notes directory"),
        };
        let metadata = match pages.symlink_metadata(filename) {
            Ok(metadata) => metadata,
            Err(error) if error.kind() == io::ErrorKind::NotFound => return Ok(None),
            Err(error) => return Err(error).context("checking page note"),
        };
        if metadata.file_type().is_symlink() || !metadata.is_file() {
            bail!("page-note name is occupied by a non-regular file");
        }
        if metadata.len() > MAX_NOTE_BYTES as u64 {
            bail!("existing page note exceeds the 4 MiB read limit");
        }
        let mut file = pages.open(filename).context("opening page note")?;
        let opened = file.metadata().context("checking opened page note")?;
        if !opened.is_file() {
            bail!("opened page note is not a regular file");
        }
        if opened.len() > MAX_NOTE_BYTES as u64 {
            bail!("opened page note exceeds the 4 MiB read limit");
        }
        let mut bytes = Vec::new();
        Read::by_ref(&mut file)
            .take((MAX_NOTE_BYTES + 1) as u64)
            .read_to_end(&mut bytes)
            .context("reading page note")?;
        if bytes.len() > MAX_NOTE_BYTES {
            bail!("existing page note exceeds the 4 MiB read limit");
        }
        Ok(Some(
            String::from_utf8(bytes).context("existing page note is not UTF-8")?,
        ))
    }

    /// Remove page-note and summary temporaries left by a host killed between
    /// creating and publishing them. Only regular files whose whole name is
    /// one this module generates in that folder (`.{NAMESPACE}-<uuid>.tmp` for
    /// notes, `.{NAMESPACE}-summary-<uuid>.tmp` for summaries), and that are
    /// older than `STALE_TEMPORARY_AGE`, are removed. Returns how many were
    /// removed.
    ///
    /// Each folder is swept independently and best effort: a folder that
    /// cannot be opened or listed is reported on stderr and skipped, and the
    /// other folder's sweep and count still stand.
    pub fn sweep_stale_temporaries(&self) -> Result<usize> {
        let mut folders = vec![(&self.pages_dir, format!(".{NAMESPACE}-"))];
        if let Some(summaries_dir) = &self.summaries_dir {
            folders.push((summaries_dir, format!(".{NAMESPACE}-summary-")));
        }
        let mut removed = 0;
        for (folder, prefix) in folders {
            match self.sweep_temporaries_in(folder, &prefix) {
                Ok(count) => removed += count,
                Err(error) => tracing::warn!(
                    folder = %folder.file_name().unwrap_or_default().to_string_lossy(),
                    error = format!("{error:#}"),
                    "could not sweep temporaries"
                ),
            }
        }
        Ok(removed)
    }

    fn sweep_temporaries_in(&self, relative: &Path, prefix: &str) -> Result<usize> {
        let dir = match self.root.open_dir(relative) {
            Ok(dir) => dir,
            Err(error) if error.kind() == io::ErrorKind::NotFound => return Ok(0),
            Err(error) => {
                return Err(error).with_context(|| format!("opening {}", relative.display()));
            }
        };
        let now = SystemTime::now();
        let mut removed = 0;
        for entry in dir
            .entries()
            .with_context(|| format!("listing {}", relative.display()))?
        {
            let entry = entry.with_context(|| format!("listing {}", relative.display()))?;
            let name = entry.file_name();
            let Some(name) = name.to_str() else { continue };
            if !is_generated_temporary(name, prefix) {
                continue;
            }
            // Not following symlinks: a link is not ours, whatever its name.
            let Ok(metadata) = dir.symlink_metadata(name) else {
                continue;
            };
            let stale = metadata.is_file()
                && metadata
                    .modified()
                    .ok()
                    .and_then(|modified| now.duration_since(modified.into_std()).ok())
                    .is_some_and(|elapsed| elapsed >= STALE_TEMPORARY_AGE);
            if stale && dir.remove_file(name).is_ok() {
                removed += 1;
            }
        }
        Ok(removed)
    }

    /// Write a temp file, fsync it, then atomically rename it over the note's
    /// generated name, replacing whatever is currently there (§4.4). Unlike
    /// the old no-clobber hard-link publish, a rename is itself the atomic
    /// cleanup step: on success no temporary name is left to remove.
    #[cfg(test)]
    fn replace_page_with_post_publish_ops<G>(
        &self,
        page_url: &str,
        markdown: &str,
        sync_parent: G,
    ) -> Result<CreatedPage>
    where
        G: FnOnce(&Dir) -> io::Result<()>,
    {
        self.replace_note_with_post_publish_ops(
            &url_record_key(page_url)?,
            page_url,
            markdown,
            sync_parent,
        )
    }

    fn replace_note_with_post_publish_ops<G>(
        &self,
        key: &RecordKey,
        canonical_url: &str,
        markdown: &str,
        sync_parent: G,
    ) -> Result<CreatedPage>
    where
        G: FnOnce(&Dir) -> io::Result<()>,
    {
        if markdown.len() > MAX_NOTE_BYTES {
            bail!("page note exceeds the 4 MiB write limit");
        }
        let filename = note_filename(key, canonical_url)?;
        self.root
            .create_dir_all(&self.pages_dir)
            .context("creating page notes directory")?;
        sync_directory_chain(&self.root, &self.pages_dir)
            .context("syncing page notes parent directories")?;
        let pages = self
            .root
            .open_dir(&self.pages_dir)
            .context("opening page notes directory")?;

        let temporary = format!(".{NAMESPACE}-{}.tmp", Uuid::new_v4());
        let mut file = pages
            .open_with(&temporary, OpenOptions::new().write(true).create_new(true))
            .context("creating page note temporary file")?;
        let mut cleanup = TempFileCleanup::new(&pages, &temporary);
        let write_result = file
            .write_all(markdown.as_bytes())
            .and_then(|_| file.sync_all());
        drop(file);
        write_result?;

        pages
            .rename(&temporary, &pages, &filename)
            .with_context(|| format!("replacing page note: {filename}"))?;
        cleanup.disarm();
        drop(cleanup);

        let directory_sync = sync_parent(&pages);
        let mut warnings = Vec::new();
        if let Err(error) = directory_sync {
            warnings.push(PostPublishWarning::DirectorySync(error));
        }
        Ok(CreatedPage {
            relative_path: self.pages_dir.join(filename),
            warnings,
        })
    }
}

/// What `Vault::find_summary` found at a record's summary path.
#[derive(Debug, PartialEq, Eq)]
pub(crate) enum FoundSummary {
    /// Nothing is there yet.
    Absent,
    /// The record's own summary: its home-folder relative path, the same
    /// path with `/` separators as summary events write it, and the time
    /// its front matter says it was summarized (`None` if unreadable).
    ///
    /// `file_id` is the (device, inode) of the file whose bytes were read
    /// (Unix only), so a caller handing the path to another program can
    /// check the name still holds that file ([`same_file`]).
    Owned {
        relative: PathBuf,
        linked: String,
        summarized_at: Option<String>,
        file_id: Option<(u64, u64)>,
    },
    /// Something else holds the name: another file, a directory, a symlink,
    /// or another record's summary.
    Unowned,
}

#[cfg(unix)]
fn file_id(metadata: &cap_std::fs::Metadata) -> Option<(u64, u64)> {
    use cap_std::fs::MetadataExt;
    Some((metadata.dev(), metadata.ino()))
}

#[cfg(not(unix))]
fn file_id(_metadata: &cap_std::fs::Metadata) -> Option<(u64, u64)> {
    None
}

/// Whether the ambient `path` names, without following a symlink at its
/// leaf, the regular file `file_id` identifies. Any intermediate folder
/// swapped or renamed since the check (the home folder included) makes the
/// path resolve to another inode, so it fails too. Without a file ID (not
/// Unix) only the regular, non-symlink check applies.
pub(crate) fn same_file(path: &Path, file_id: Option<(u64, u64)>) -> bool {
    let Ok(metadata) = fs::symlink_metadata(path) else {
        return false;
    };
    if !metadata.file_type().is_file() {
        return false;
    }
    #[cfg(unix)]
    {
        use std::os::unix::fs::MetadataExt;
        file_id == Some((metadata.dev(), metadata.ino()))
    }
    #[cfg(not(unix))]
    {
        let _ = file_id;
        true
    }
}

/// The outcome of `replace_owned_file`.
pub(crate) enum OwnedWrite {
    /// The new content is published under the name. Warnings describe work
    /// that failed after publication; the write must not be retried as if it
    /// failed.
    Written(Vec<PostPublishWarning>),
    Unchanged,
    Conflict,
}

/// The filesystem steps `replace_owned_file` takes around publication, so
/// tests can fail or race them.
struct OwnedFileOps<'a> {
    /// Runs between inspecting the name and opening its occupant.
    after_inspect: &'a dyn Fn(&Dir),
    remove_temporary: &'a dyn Fn(&Dir, &str) -> io::Result<()>,
    sync_directory: &'a dyn Fn(&Dir) -> io::Result<()>,
}

impl OwnedFileOps<'static> {
    const REAL: Self = Self {
        after_inspect: &|_| {},
        remove_temporary: &|dir, name| dir.remove_file(name),
        sync_directory: &sync_directory,
    };
}

/// Replace a Brauser-owned file whole, under `root`, at `relative`.
///
/// `contents` over 4 MiB is refused before anything is touched: a file that
/// large could never be read back for the ownership check, so every later
/// write would be a `Conflict`. The parent chain is created and synced first.
/// When the name is absent, the new content is written and fsynced to
/// `.{NAMESPACE}-{kind}-{uuid}.tmp` and published with a no-clobber hard
/// link; losing that race to another writer is a `Conflict`. An existing
/// name is renamed over only when it is a regular file (not a symlink) of at
/// most 4 MiB whose bytes `owned` accepts, and is left alone as `Unchanged`
/// when it already holds `contents`. Anything else is a `Conflict`, and the
/// occupant is never touched. Removing the published temporary and syncing
/// the directory happen after publication, so their failures are warnings on
/// `Written`, as for page notes; the startup sweep removes a leftover
/// temporary.
///
/// The occupant is read through a handle that must be the regular file the
/// name was inspected as (on Unix, the same device and inode), so a name
/// swapped for a symlink or another file between the two is a `Conflict`.
/// A window remains between that read and the rename: a writer that replaces
/// the occupant after the ownership check has its file replaced. Brauser's
/// own writers serialize under the host's locks, so the window is open only
/// to another program writing the same generated name.
pub(crate) fn replace_owned_file(
    root: &Dir,
    relative: &Path,
    contents: &str,
    kind: &str,
    owned: &dyn Fn(&[u8]) -> bool,
) -> Result<OwnedWrite> {
    replace_owned_file_with(root, relative, contents, kind, owned, &OwnedFileOps::REAL)
}

fn replace_owned_file_with(
    root: &Dir,
    relative: &Path,
    contents: &str,
    kind: &str,
    owned: &dyn Fn(&[u8]) -> bool,
    ops: &OwnedFileOps<'_>,
) -> Result<OwnedWrite> {
    if contents.len() > MAX_OWNED_FILE_BYTES {
        bail!("{kind} file exceeds the 4 MiB write limit");
    }
    let parent = relative
        .parent()
        .with_context(|| format!("{kind} path has no parent"))?;
    let filename = relative
        .file_name()
        .with_context(|| format!("{kind} path has no filename"))?;
    root.create_dir_all(parent)
        .with_context(|| format!("creating {kind} directory"))?;
    sync_directory_chain(root, parent)
        .with_context(|| format!("syncing {kind} directory chain"))?;
    let dir = root
        .open_dir(parent)
        .with_context(|| format!("opening {kind} directory"))?;
    let existed = match dir.symlink_metadata(filename) {
        Ok(inspected) => {
            if inspected.file_type().is_symlink()
                || !inspected.is_file()
                || inspected.len() > MAX_OWNED_FILE_BYTES as u64
            {
                return Ok(OwnedWrite::Conflict);
            }
            (ops.after_inspect)(&dir);
            let Some(existing) = read_inspected_occupant(&dir, filename, &inspected)
                .with_context(|| format!("reading existing {kind}"))?
            else {
                return Ok(OwnedWrite::Conflict);
            };
            if !owned(&existing) {
                return Ok(OwnedWrite::Conflict);
            }
            if existing == contents.as_bytes() {
                return Ok(OwnedWrite::Unchanged);
            }
            true
        }
        Err(error) if error.kind() == io::ErrorKind::NotFound => false,
        Err(error) => return Err(error).with_context(|| format!("checking {kind} file")),
    };

    let temporary = format!(".{NAMESPACE}-{kind}-{}.tmp", Uuid::new_v4());
    let mut file = dir
        .open_with(&temporary, OpenOptions::new().write(true).create_new(true))
        .with_context(|| format!("creating {kind} temporary"))?;
    let mut cleanup = TempFileCleanup::new(&dir, &temporary);
    file.write_all(contents.as_bytes())
        .and_then(|()| file.sync_all())?;
    drop(file);
    let mut warnings = Vec::new();
    if existed {
        dir.rename(&temporary, &dir, filename)
            .with_context(|| format!("replacing {kind} file"))?;
        cleanup.disarm();
    } else {
        match dir.hard_link(&temporary, &dir, filename) {
            Ok(()) => {
                // The hard link is the publication point.
                cleanup.disarm();
                if let Err(error) = (ops.remove_temporary)(&dir, &temporary) {
                    warnings.push(PostPublishWarning::TemporaryCleanup(error));
                }
            }
            Err(error) if error.kind() == io::ErrorKind::AlreadyExists => {
                return Ok(OwnedWrite::Conflict);
            }
            Err(error) => return Err(error).with_context(|| format!("publishing {kind} file")),
        }
    }
    drop(cleanup);
    if let Err(error) = (ops.sync_directory)(&dir) {
        warnings.push(PostPublishWarning::DirectorySync(error));
    }
    Ok(OwnedWrite::Written(warnings))
}

/// The bytes of the regular file at `name`, read only when the opened handle
/// is the file `inspected` (taken without following symlinks) described:
/// `None` when the name now holds something else or more than 4 MiB.
fn read_inspected_occupant(
    dir: &Dir,
    name: &std::ffi::OsStr,
    inspected: &cap_std::fs::Metadata,
) -> io::Result<Option<Vec<u8>>> {
    let mut file = dir.open(name)?;
    let opened = file.metadata()?;
    if !opened.is_file() || opened.len() > MAX_OWNED_FILE_BYTES as u64 {
        return Ok(None);
    }
    #[cfg(unix)]
    {
        use cap_std::fs::MetadataExt;
        if (opened.dev(), opened.ino()) != (inspected.dev(), inspected.ino()) {
            return Ok(None);
        }
    }
    // Windows has no by-handle file ID in cap-std's metadata; there, the
    // handle is only checked to be a regular file within the size cap.
    #[cfg(not(unix))]
    let _ = inspected;
    let mut existing = Vec::new();
    Read::by_ref(&mut file)
        .take(MAX_OWNED_FILE_BYTES as u64 + 1)
        .read_to_end(&mut existing)?;
    if existing.len() > MAX_OWNED_FILE_BYTES {
        return Ok(None);
    }
    Ok(Some(existing))
}

/// True only when `bytes` open with a `---` frontmatter holding exactly one
/// top-level `brauser:` block, and that block has exactly one `kind:` line
/// equal to `kind`, exactly one line for each `ids` key whose JSON string
/// value equals the expected value, and no line for any `forbidden` key. Only
/// the leading frontmatter is read, so body text cannot change a file's
/// identity.
pub(crate) fn owned_frontmatter(
    bytes: &[u8],
    kind: &str,
    ids: &[(&str, &str)],
    forbidden: &[&str],
) -> bool {
    let Ok(text) = std::str::from_utf8(bytes) else {
        return false;
    };
    let Some(rest) = text.strip_prefix("---\n") else {
        return false;
    };
    let Some(frontmatter) = rest.split_once("\n---\n").map(|value| value.0) else {
        return false;
    };
    let mut blocks = 0;
    let mut in_brauser = false;
    let mut kinds = 0;
    let mut kind_matches = false;
    let mut id_counts = vec![0usize; ids.len()];
    let mut id_matches = vec![false; ids.len()];
    let mut forbidden_seen = false;
    for line in frontmatter.lines() {
        if !line.starts_with(' ') {
            in_brauser = line == "brauser:";
            if in_brauser {
                blocks += 1;
            }
            continue;
        }
        if !in_brauser {
            continue;
        }
        if let Some(value) = line.strip_prefix("  kind: ") {
            kinds += 1;
            kind_matches = value == kind;
            continue;
        }
        for (index, (key, expected)) in ids.iter().enumerate() {
            if let Some(value) = block_value(line, key) {
                id_counts[index] += 1;
                id_matches[index] =
                    serde_json::from_str::<String>(value).ok().as_deref() == Some(*expected);
            }
        }
        if forbidden.iter().any(|key| {
            line.strip_prefix("  ")
                .and_then(|rest| rest.strip_prefix(key))
                .is_some_and(|rest| rest.starts_with(':'))
        }) {
            forbidden_seen = true;
        }
    }
    blocks == 1
        && kinds == 1
        && kind_matches
        && id_counts.iter().all(|count| *count == 1)
        && id_matches.iter().all(|matches| *matches)
        && !forbidden_seen
}

/// The value of a `  key: value` line directly inside a frontmatter block.
fn block_value<'a>(line: &'a str, key: &str) -> Option<&'a str> {
    line.strip_prefix("  ")?
        .strip_prefix(key)?
        .strip_prefix(": ")
}

/// Open the selected directory as a capability, then identify that handle.
/// Reopening a pathname must never silently authorize a replacement folder.
pub(crate) fn open_selected_root(
    chosen: &Path,
    expected_identity: Option<&str>,
) -> Result<(Dir, String)> {
    if !chosen.is_absolute() {
        bail!("home folder must be an absolute path");
    }
    let canonical = fs::canonicalize(chosen)
        .with_context(|| format!("resolving home folder {}", chosen.display()))?;
    if !canonical.is_dir() {
        bail!("home folder is not a directory: {}", canonical.display());
    }
    // This is the sole ambient filesystem entry point for vault contents.
    // cap-std confines subsequent paths to the opened directory.
    let root = Dir::open_ambient_dir(&canonical, ambient_authority())
        .with_context(|| format!("opening home folder {}", canonical.display()))?;
    let identity = directory_identity(&root)?;
    if expected_identity.is_some_and(|expected| expected != identity) {
        bail!("selected home folder changed; choose it again");
    }
    Ok((root, identity))
}

pub(crate) fn selected_root_identity(chosen: &Path) -> Result<String> {
    let (_, identity) = open_selected_root(chosen, None)?;
    Ok(identity)
}

fn directory_identity(dir: &Dir) -> Result<String> {
    let metadata = dir
        .try_clone()
        .context("cloning home folder handle")?
        .into_std_file()
        .metadata()
        .context("identifying home folder")?;
    if !metadata.is_dir() {
        bail!("selected home folder is not a directory");
    }
    #[cfg(unix)]
    {
        use std::os::unix::fs::MetadataExt;
        Ok(format!(
            "unix:{:016x}:{:016x}",
            metadata.dev(),
            metadata.ino()
        ))
    }
    #[cfg(windows)]
    {
        // std's by-handle metadata accessors are unstable; winapi-util wraps
        // GetFileInformationByHandle for the same volume and file IDs.
        let handle = dir
            .try_clone()
            .context("cloning home folder handle")?
            .into_std_file();
        let info = winapi_util::file::information(&handle)
            .context("identifying home folder on Windows")?;
        Ok(format!(
            "windows:{:08x}:{:016x}",
            info.volume_serial_number(),
            info.file_index()
        ))
    }
}

/// True only for `<prefix><hyphenated UUID>.tmp`, the exact shape this host
/// generates, so a user's own file is never mistaken for a leftover.
pub(crate) fn is_generated_temporary(name: &str, prefix: &str) -> bool {
    name.strip_prefix(prefix)
        .and_then(|rest| rest.strip_suffix(".tmp"))
        .is_some_and(|id| Uuid::try_parse(id).is_ok_and(|uuid| uuid.hyphenated().to_string() == id))
}

/// `path`'s components joined with `/`, as links and summary events write
/// vault-relative paths on every platform; `None` when one is not UTF-8.
fn slash_path(path: &Path) -> Option<String> {
    let parts = path
        .components()
        .map(|part| part.as_os_str().to_str())
        .collect::<Option<Vec<_>>>()?;
    Some(parts.join("/"))
}

pub(crate) fn checked_relative_dir(value: &str) -> Result<PathBuf> {
    let path = Path::new(value);
    if path.as_os_str().is_empty()
        || !path
            .components()
            .all(|part| matches!(part, Component::Normal(_)))
    {
        bail!("content location must be a nonempty relative directory");
    }
    Ok(path.to_path_buf())
}

pub fn page_filename(page_url: &str) -> Result<String> {
    let mut parsed = Url::parse(page_url).context("page URL is invalid")?;
    if !matches!(parsed.scheme(), "http" | "https") || parsed.host_str().is_none() {
        bail!("page URL must be HTTP or HTTPS with a host");
    }
    if !parsed.username().is_empty() || parsed.password().is_some() {
        bail!("page URL must not contain credentials");
    }
    parsed.set_fragment(None);
    let canonical = parsed.as_str();
    let digest = Sha256::digest(canonical.as_bytes());
    let stem = format!("{}{}", parsed.host_str().unwrap_or_default(), parsed.path());
    let slug = slug(&stem);
    Ok(format!("{slug}-{}.md", hex::encode(digest)))
}

pub fn url_record_key(page_url: &str) -> Result<RecordKey> {
    let mut parsed = Url::parse(page_url).context("page URL is invalid")?;
    if !matches!(parsed.scheme(), "http" | "https") || parsed.host_str().is_none() {
        bail!("page URL must be HTTP or HTTPS with a host");
    }
    if !parsed.username().is_empty() || parsed.password().is_some() {
        bail!("page URL must not contain credentials");
    }
    parsed.set_fragment(None);
    Ok(RecordKey {
        kind: RecordKeyKind::Url,
        value: hex::encode(Sha256::digest(parsed.as_str().as_bytes())),
    })
}

pub fn note_filename(key: &RecordKey, canonical_url: &str) -> Result<String> {
    match key.kind {
        RecordKeyKind::Url => {
            let expected = url_record_key(canonical_url)?;
            if &expected != key {
                bail!("URL record key does not match its canonical URL");
            }
            page_filename(canonical_url)
        }
        RecordKeyKind::Artifact => {
            if key.value.is_empty() || key.value.len() > 512 {
                bail!("artifact record key is invalid");
            }
            let digest = hex::encode(Sha256::digest(key.value.as_bytes()));
            Ok(format!("artifact-{}-{}.md", slug(&key.value), digest))
        }
    }
}

fn slug(value: &str) -> String {
    let mut result = String::new();
    let mut last_dash = false;
    for ch in value.chars() {
        if result.len() >= 64 {
            break;
        }
        if ch.is_ascii_alphanumeric() {
            result.push(ch.to_ascii_lowercase());
            last_dash = false;
        } else if !last_dash && !result.is_empty() {
            result.push('-');
            last_dash = true;
        }
    }
    while result.ends_with('-') {
        result.pop();
    }
    if result.is_empty() {
        "page".to_owned()
    } else {
        result
    }
}

/// Removes a temporary file on early return. Disarm it once the final name is
/// published: another writer could reuse the temporary name after that.
pub(crate) struct TempFileCleanup<'a> {
    dir: &'a Dir,
    name: &'a str,
    armed: bool,
}

impl<'a> TempFileCleanup<'a> {
    pub(crate) fn new(dir: &'a Dir, name: &'a str) -> Self {
        Self {
            dir,
            name,
            armed: true,
        }
    }

    pub(crate) fn disarm(&mut self) {
        self.armed = false;
    }
}

impl Drop for TempFileCleanup<'_> {
    fn drop(&mut self) {
        if self.armed {
            let _ = self.dir.remove_file(self.name);
        }
    }
}

#[cfg(not(windows))]
pub(crate) fn sync_directory(dir: &Dir) -> io::Result<()> {
    // cap-std may represent a directory capability with an O_PATH handle on
    // Linux. That handle is suitable for relative lookup but fsync(2) rejects
    // it with EBADF, so open the capability itself as a readable directory
    // before asking the filesystem to persist its entries.
    dir.open(".")?.sync_all()?;
    Ok(())
}

#[cfg(windows)]
pub(crate) fn sync_directory(_dir: &Dir) -> io::Result<()> {
    // std does not expose a portable way to fsync a Windows directory handle.
    Ok(())
}

pub(crate) fn sync_directory_chain(root: &Dir, relative: &Path) -> io::Result<()> {
    let mut current = root.try_clone()?;
    sync_directory(&current)?;
    for part in relative.components() {
        current = current.open_dir(part.as_os_str())?;
        sync_directory(&current)?;
    }
    Ok(())
}

#[cfg(test)]
pub(crate) mod tests {
    use super::*;

    fn storage(root: &Path, pages_dir: &str) -> StorageConfig {
        StorageConfig {
            root: root.to_string_lossy().into_owned(),
            profile: "neutral".into(),
            log_dir: "log".into(),
            pages_dir: pages_dir.into(),
            later_dir: "later".into(),
            artifacts_dir: "artifacts".into(),
            summaries_dir: None,
            knowledge_base_dir: None,
        }
    }

    #[test]
    fn rejects_traversal_in_configured_directory() {
        let root = tempfile::tempdir().unwrap();
        assert!(Vault::open(&storage(root.path(), "../escape")).is_err());
        assert!(Vault::open(&storage(root.path(), "/tmp/escape")).is_err());
    }

    #[test]
    fn replacing_a_page_creates_it_when_absent() {
        let root = tempfile::tempdir().unwrap();
        let vault = Vault::open(&storage(root.path(), "pages")).unwrap();
        let created = vault
            .replace_page("https://example.com/article#part-1", "first")
            .unwrap();
        assert!(created.warnings.is_empty());
        assert_eq!(
            fs::read_to_string(root.path().join(&created.relative_path)).unwrap(),
            "first"
        );
        // The fragment is not part of the generated identity.
        assert_eq!(
            created.relative_path,
            vault
                .page_relative_path("https://example.com/article#part-2")
                .unwrap()
        );
    }

    #[test]
    fn replacing_a_page_overwrites_existing_content() {
        let root = tempfile::tempdir().unwrap();
        let vault = Vault::open(&storage(root.path(), "pages")).unwrap();
        let first = vault
            .replace_page("https://example.com/article", "first")
            .unwrap();
        let second = vault
            .replace_page("https://example.com/article", "second")
            .unwrap();
        assert_eq!(first.relative_path, second.relative_path);
        assert_eq!(
            fs::read_to_string(root.path().join(second.relative_path)).unwrap(),
            "second"
        );
    }

    #[test]
    fn post_publish_directory_sync_failure_reports_a_warning_but_still_publishes() {
        let root = tempfile::tempdir().unwrap();
        let vault = Vault::open(&storage(root.path(), "pages")).unwrap();
        let created = vault
            .replace_page_with_post_publish_ops(
                "https://example.com/article",
                "saved content",
                |_| Err(io::Error::other("simulated directory sync failure")),
            )
            .unwrap();

        assert_eq!(
            fs::read_to_string(root.path().join(&created.relative_path)).unwrap(),
            "saved content"
        );
        assert!(matches!(
            created.warnings.as_slice(),
            [PostPublishWarning::DirectorySync(_)]
        ));
    }

    #[test]
    fn replace_page_overwrites_whatever_occupies_the_generated_name() {
        // vault::replace_page trusts its caller. The note module (§4.4) is
        // responsible for checking the caller's version and ownership of an
        // existing file before calling this; this low-level test documents
        // that this layer itself does not refuse an unrelated occupant.
        let root = tempfile::tempdir().unwrap();
        let pages = root.path().join("pages");
        fs::create_dir(&pages).unwrap();
        let name = page_filename("https://example.com/user-note").unwrap();
        let user_file = pages.join(name);
        fs::write(&user_file, "user content").unwrap();

        let vault = Vault::open(&storage(root.path(), "pages")).unwrap();
        vault
            .replace_page("https://example.com/user-note", "generated")
            .unwrap();
        assert_eq!(fs::read_to_string(user_file).unwrap(), "generated");
    }

    #[test]
    fn a_sync_failure_after_a_note_move_reports_the_finished_move_with_a_warning() {
        let root = tempfile::tempdir().unwrap();
        let vault = Vault::open(&storage(root.path(), "pages")).unwrap();
        let source_url = "https://example.com/older";
        let source_key = url_record_key(source_url).unwrap();
        vault.replace_page(source_url, "older note").unwrap();
        let destination_key = RecordKey {
            kind: RecordKeyKind::Artifact,
            value: "work.item:example.com/1".into(),
        };
        let destination_url = "https://example.com/item/1";
        let moved = vault
            .move_note_no_clobber_with_sync(
                &source_key,
                source_url,
                &destination_key,
                destination_url,
                "older note",
                "artifact note",
                |_| Err(io::Error::other("simulated directory sync failure")),
            )
            .unwrap();

        assert!(matches!(
            moved.warnings.as_slice(),
            [PostPublishWarning::DirectorySync(_)]
        ));
        assert_eq!(
            fs::read_to_string(root.path().join(&moved.relative_path)).unwrap(),
            "artifact note"
        );
        assert_eq!(vault.read_page(source_url).unwrap(), None);
    }

    fn owned_root(path: &Path) -> Dir {
        Dir::open_ambient_dir(path, ambient_authority()).unwrap()
    }

    fn temporaries_in(dir: &Path) -> Vec<String> {
        fs::read_dir(dir)
            .unwrap()
            .map(|entry| entry.unwrap().file_name().to_string_lossy().into_owned())
            .filter(|name| name.ends_with(".tmp"))
            .collect()
    }

    #[test]
    fn owned_replace_creates_replaces_and_skips_unchanged() {
        let root = tempfile::tempdir().unwrap();
        let dir = owned_root(root.path());
        let relative = Path::new(crate::layout::ARTIFACTS_DIR).join("site/a.md");
        let path = root.path().join(&relative);
        let ours = |bytes: &[u8]| bytes.starts_with(b"ours");

        // Absent: the parent chain is created and the file is published.
        assert!(matches!(
            replace_owned_file(&dir, &relative, "ours 1", "thing", &ours).unwrap(),
            OwnedWrite::Written(_)
        ));
        assert_eq!(fs::read_to_string(&path).unwrap(), "ours 1");
        // Byte-identical content is not rewritten.
        let before = fs::metadata(&path).unwrap().modified().unwrap();
        assert!(matches!(
            replace_owned_file(&dir, &relative, "ours 1", "thing", &ours).unwrap(),
            OwnedWrite::Unchanged
        ));
        assert_eq!(fs::metadata(&path).unwrap().modified().unwrap(), before);
        // An owned file is replaced whole.
        assert!(matches!(
            replace_owned_file(&dir, &relative, "ours 2", "thing", &ours).unwrap(),
            OwnedWrite::Written(_)
        ));
        assert_eq!(fs::read_to_string(&path).unwrap(), "ours 2");
        // A file the predicate rejects is a conflict and stays as it was.
        fs::write(&path, "the user's own text").unwrap();
        assert!(matches!(
            replace_owned_file(&dir, &relative, "ours 3", "thing", &ours).unwrap(),
            OwnedWrite::Conflict
        ));
        assert_eq!(fs::read_to_string(&path).unwrap(), "the user's own text");
        assert!(temporaries_in(path.parent().unwrap()).is_empty());
    }

    #[test]
    fn unowned_symlink_or_directory_is_a_conflict_and_untouched() {
        let root = tempfile::tempdir().unwrap();
        let dir = owned_root(root.path());
        let site = root.path().join(crate::layout::ARTIFACTS_DIR).join("site");
        fs::create_dir_all(&site).unwrap();
        let anything = |_: &[u8]| true;
        let relative = |name: &str| {
            Path::new(crate::layout::ARTIFACTS_DIR)
                .join("site")
                .join(name)
        };

        // A directory at the name is never replaced.
        fs::create_dir(site.join("dir.md")).unwrap();
        fs::write(site.join("dir.md").join("inside"), "kept").unwrap();
        assert!(matches!(
            replace_owned_file(&dir, &relative("dir.md"), "ours", "thing", &anything).unwrap(),
            OwnedWrite::Conflict
        ));
        assert_eq!(
            fs::read_to_string(site.join("dir.md").join("inside")).unwrap(),
            "kept"
        );

        // A file over 4 MiB is never read whole or replaced, even when the
        // predicate would accept it.
        let large = vec![b'x'; MAX_OWNED_FILE_BYTES + 1];
        fs::write(site.join("large.md"), &large).unwrap();
        assert!(matches!(
            replace_owned_file(&dir, &relative("large.md"), "ours", "thing", &anything).unwrap(),
            OwnedWrite::Conflict
        ));
        assert_eq!(fs::read(site.join("large.md")).unwrap(), large);

        #[cfg(unix)]
        {
            // A symlink is never followed or replaced, whatever it points at.
            let outside = tempfile::tempdir().unwrap();
            let target = outside.path().join("target.md");
            fs::write(&target, "outside").unwrap();
            std::os::unix::fs::symlink(&target, site.join("link.md")).unwrap();
            assert!(matches!(
                replace_owned_file(&dir, &relative("link.md"), "ours", "thing", &anything).unwrap(),
                OwnedWrite::Conflict
            ));
            assert!(
                fs::symlink_metadata(site.join("link.md"))
                    .unwrap()
                    .file_type()
                    .is_symlink()
            );
            assert_eq!(fs::read_to_string(&target).unwrap(), "outside");
        }
        assert!(temporaries_in(&site).is_empty());
    }

    #[test]
    fn owned_replace_refuses_content_over_the_cap_and_publishes_nothing() {
        // A file this large would fail the size guard on every later write
        // and so be a conflict forever; it is refused up front instead.
        let root = tempfile::tempdir().unwrap();
        let dir = owned_root(root.path());
        let relative = Path::new(crate::layout::ARTIFACTS_DIR).join("site/a.md");
        let anything = |_: &[u8]| true;
        let large = "x".repeat(MAX_OWNED_FILE_BYTES + 1);
        let error = replace_owned_file(&dir, &relative, &large, "thing", &anything)
            .err()
            .expect("oversized content is refused");
        assert!(format!("{error:#}").contains("4 MiB"), "{error:#}");
        assert!(!root.path().join(crate::layout::ARTIFACTS_DIR).exists());
        // Content exactly at the cap is still written.
        let at_cap = "x".repeat(MAX_OWNED_FILE_BYTES);
        assert!(matches!(
            replace_owned_file(&dir, &relative, &at_cap, "thing", &anything).unwrap(),
            OwnedWrite::Written(_)
        ));
    }

    #[test]
    fn failures_after_an_owned_publish_are_warnings_on_a_written_result() {
        let root = tempfile::tempdir().unwrap();
        let dir = owned_root(root.path());
        let relative = Path::new(crate::layout::ARTIFACTS_DIR).join("site/a.md");
        let path = root.path().join(&relative);
        let ours = |bytes: &[u8]| bytes.starts_with(b"ours");
        let failing = OwnedFileOps {
            after_inspect: &|_| {},
            remove_temporary: &|_, _| Err(io::Error::other("simulated cleanup failure")),
            sync_directory: &|_| Err(io::Error::other("simulated directory sync failure")),
        };

        // A new name is published by hard link; the temporary that could not
        // be removed is left for the startup sweep.
        let OwnedWrite::Written(warnings) =
            replace_owned_file_with(&dir, &relative, "ours 1", "thing", &ours, &failing).unwrap()
        else {
            panic!("the published file was not reported as written");
        };
        assert!(matches!(
            warnings.as_slice(),
            [
                PostPublishWarning::TemporaryCleanup(_),
                PostPublishWarning::DirectorySync(_)
            ]
        ));
        assert_eq!(fs::read_to_string(&path).unwrap(), "ours 1");
        let left = temporaries_in(path.parent().unwrap());
        assert_eq!(left.len(), 1);
        assert!(is_generated_temporary(
            &left[0],
            &format!(".{NAMESPACE}-thing-")
        ));
        fs::remove_file(path.parent().unwrap().join(&left[0])).unwrap();

        // An owned name is replaced by rename, which leaves no temporary; only
        // the directory sync can fail after it.
        let OwnedWrite::Written(warnings) =
            replace_owned_file_with(&dir, &relative, "ours 2", "thing", &ours, &failing).unwrap()
        else {
            panic!("the replaced file was not reported as written");
        };
        assert!(matches!(
            warnings.as_slice(),
            [PostPublishWarning::DirectorySync(_)]
        ));
        assert_eq!(fs::read_to_string(&path).unwrap(), "ours 2");
        assert!(temporaries_in(path.parent().unwrap()).is_empty());

        // Without failures there are no warnings.
        let OwnedWrite::Written(warnings) =
            replace_owned_file(&dir, &relative, "ours 3", "thing", &ours).unwrap()
        else {
            panic!("the replaced file was not reported as written");
        };
        assert!(warnings.is_empty());
    }

    // Uses Unix symlink creation; Windows symlinks require separate privileges.
    #[cfg(unix)]
    #[test]
    fn an_occupant_swapped_after_inspection_is_a_conflict() {
        let root = tempfile::tempdir().unwrap();
        let dir = owned_root(root.path());
        let site = root.path().join(crate::layout::ARTIFACTS_DIR).join("site");
        fs::create_dir_all(&site).unwrap();
        let relative = Path::new(crate::layout::ARTIFACTS_DIR).join("site/a.md");
        let path = site.join("a.md");
        let owned_elsewhere = site.join("elsewhere.md");
        let ours = |bytes: &[u8]| bytes.starts_with(b"ours");

        // Swapped for a symlink to a file the predicate would accept: the
        // link is never followed into an ownership decision.
        fs::write(&path, "user text").unwrap();
        fs::write(&owned_elsewhere, "ours elsewhere").unwrap();
        let to_symlink = OwnedFileOps {
            after_inspect: &|_| {
                fs::remove_file(&path).unwrap();
                std::os::unix::fs::symlink("elsewhere.md", &path).unwrap();
            },
            ..OwnedFileOps::REAL
        };
        assert!(matches!(
            replace_owned_file_with(&dir, &relative, "ours new", "thing", &ours, &to_symlink)
                .unwrap(),
            OwnedWrite::Conflict
        ));
        assert!(
            fs::symlink_metadata(&path)
                .unwrap()
                .file_type()
                .is_symlink()
        );
        assert_eq!(
            fs::read_to_string(&owned_elsewhere).unwrap(),
            "ours elsewhere"
        );

        // Swapped for a different regular file: not the inspected occupant.
        fs::remove_file(&path).unwrap();
        fs::write(&path, "user text").unwrap();
        let replacement = site.join("replacement.md");
        fs::write(&replacement, "ours replacement").unwrap();
        let to_other_file = OwnedFileOps {
            after_inspect: &|_| fs::rename(&replacement, &path).unwrap(),
            ..OwnedFileOps::REAL
        };
        assert!(matches!(
            replace_owned_file_with(&dir, &relative, "ours new", "thing", &ours, &to_other_file)
                .unwrap(),
            OwnedWrite::Conflict
        ));
        assert_eq!(fs::read_to_string(&path).unwrap(), "ours replacement");
        assert!(temporaries_in(&site).is_empty());
    }

    #[test]
    fn ownership_needs_exactly_one_block_kind_and_id() {
        let ids: &[(&str, &str)] = &[("canonical_url", "https://e.example/a"), ("url_id", "u1")];
        let owned = "---\ntitle: \"A\"\nbrauser:\n  kind: summary\n  canonical_url: \"https://e.example/a\"\n  url_id: \"u1\"\n---\nbody\n";
        assert!(owned_frontmatter(
            owned.as_bytes(),
            "summary",
            ids,
            &["artifact_id"]
        ));

        for (case, text) in [
            ("no frontmatter", "brauser:\n  kind: summary\n".to_owned()),
            (
                "unterminated",
                "---\nbrauser:\n  kind: summary\n".to_owned(),
            ),
            ("another block", owned.replace("brauser:", "other:")),
            (
                "two blocks",
                owned.replace("---\nbody", "brauser:\n  kind: summary\n---\nbody"),
            ),
            (
                "wrong kind",
                owned.replace("kind: summary", "kind: artifact"),
            ),
            (
                "two kinds",
                owned.replace("  kind: summary\n", "  kind: summary\n  kind: summary\n"),
            ),
            ("no kind", owned.replace("  kind: summary\n", "")),
            ("wrong id", owned.replace("\"u1\"", "\"u2\"")),
            ("unquoted id", owned.replace("\"u1\"", "u1")),
            (
                "two ids",
                owned.replace("  url_id: \"u1\"\n", "  url_id: \"u1\"\n  url_id: \"u1\"\n"),
            ),
            ("missing id", owned.replace("  url_id: \"u1\"\n", "")),
            (
                "forbidden key",
                owned.replace("  url_id:", "  artifact_id: \"x\"\n  url_id:"),
            ),
            (
                "id in the body",
                owned.replace("  url_id: \"u1\"\n---", "---\n  url_id: \"u1\""),
            ),
            (
                "id under a sibling top-level key",
                owned.replace("  url_id: \"u1\"\n---", "other:\n  url_id: \"u1\"\n---"),
            ),
            (
                "body only",
                format!("---\ntitle: \"A\"\n---\n{}", &owned[4..]),
            ),
        ] {
            assert!(
                !owned_frontmatter(text.as_bytes(), "summary", ids, &["artifact_id"]),
                "{case} was accepted"
            );
        }
        assert!(!owned_frontmatter(&[0xff, 0xfe], "summary", ids, &[]));
        // Indented deeper than the block's own keys is not the block's key.
        let nested = owned.replace("  url_id: \"u1\"\n", "  extra:\n    url_id: \"u1\"\n");
        assert!(!owned_frontmatter(nested.as_bytes(), "summary", ids, &[]));
    }

    pub(crate) fn age_file(path: &Path, age: Duration) {
        fs::File::options()
            .write(true)
            .open(path)
            .unwrap()
            .set_modified(SystemTime::now() - age)
            .unwrap();
    }

    #[test]
    fn startup_sweep_removes_only_stale_page_note_temporaries() {
        let root = tempfile::tempdir().unwrap();
        let pages = root.path().join("pages");
        fs::create_dir(&pages).unwrap();
        let old = STALE_TEMPORARY_AGE + Duration::from_secs(60);
        let stale = pages.join(format!(".{NAMESPACE}-{}.tmp", Uuid::new_v4()));
        let fresh = pages.join(format!(".{NAMESPACE}-{}.tmp", Uuid::new_v4()));
        // Other writers' temporaries and user files are never touched.
        let log_temp = pages.join(format!(".{NAMESPACE}-log-{}.tmp", Uuid::new_v4()));
        let user_file = pages.join(format!(".{NAMESPACE}-notes.tmp"));
        let note = pages.join("note.md");
        for path in [&stale, &fresh, &log_temp, &user_file, &note] {
            fs::write(path, "content").unwrap();
        }
        for path in [&stale, &log_temp, &user_file, &note] {
            age_file(path, old);
        }

        let vault = Vault::open(&storage(root.path(), "pages")).unwrap();
        assert_eq!(vault.sweep_stale_temporaries().unwrap(), 1);
        assert!(!stale.exists());
        for path in [&fresh, &log_temp, &user_file, &note] {
            assert!(path.exists(), "{} was removed", path.display());
        }
    }

    #[test]
    fn startup_sweep_tolerates_a_missing_pages_directory() {
        let root = tempfile::tempdir().unwrap();
        let vault = Vault::open(&storage(root.path(), "pages")).unwrap();
        assert_eq!(vault.sweep_stale_temporaries().unwrap(), 0);
    }

    // Uses Unix symlink creation; Windows symlinks require separate privileges.
    #[cfg(unix)]
    #[test]
    fn startup_sweep_does_not_follow_symlinks() {
        use std::os::unix::fs::symlink;

        let root = tempfile::tempdir().unwrap();
        let outside = tempfile::tempdir().unwrap();
        let target = outside.path().join("target");
        fs::write(&target, "outside").unwrap();
        let pages = root.path().join("pages");
        fs::create_dir(&pages).unwrap();
        let link = pages.join(format!(".{NAMESPACE}-{}.tmp", Uuid::new_v4()));
        symlink(&target, &link).unwrap();
        age_file(&target, STALE_TEMPORARY_AGE + Duration::from_secs(60));

        let vault = Vault::open(&storage(root.path(), "pages")).unwrap();
        assert_eq!(vault.sweep_stale_temporaries().unwrap(), 0);
        assert!(link.symlink_metadata().is_ok());
        assert_eq!(fs::read_to_string(target).unwrap(), "outside");
    }

    // Uses Unix symlink creation; Windows symlinks require separate privileges.
    #[cfg(unix)]
    #[test]
    fn configured_symlink_cannot_escape_the_vault() {
        use std::os::unix::fs::symlink;

        let root = tempfile::tempdir().unwrap();
        let outside = tempfile::tempdir().unwrap();
        symlink(outside.path(), root.path().join("pages")).unwrap();
        let vault = Vault::open(&storage(root.path(), "pages")).unwrap();
        assert!(
            vault
                .replace_page("https://example.com", "content")
                .is_err()
        );
        assert_eq!(fs::read_dir(outside.path()).unwrap().count(), 0);
    }

    // Checks Unix device/inode identity and symlinks, unavailable on Windows.
    #[cfg(unix)]
    #[test]
    fn same_file_refuses_a_swapped_or_symlinked_name() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("a.summary.md");
        fs::write(&path, "owned\n").unwrap();
        let id = {
            use std::os::unix::fs::MetadataExt;
            let metadata = fs::metadata(&path).unwrap();
            Some((metadata.dev(), metadata.ino()))
        };
        assert!(same_file(&path, id));
        // Another file under the name.
        let other = dir.path().join("other.command");
        fs::write(&other, "#!/bin/sh\n").unwrap();
        fs::rename(&path, dir.path().join("kept")).unwrap();
        fs::copy(&other, &path).unwrap();
        assert!(!same_file(&path, id));
        // A symlink to the very file that was checked is still refused.
        fs::remove_file(&path).unwrap();
        std::os::unix::fs::symlink(dir.path().join("kept"), &path).unwrap();
        assert!(!same_file(&path, id));
        // Nothing at the name, or no file ID to match.
        fs::remove_file(&path).unwrap();
        assert!(!same_file(&path, id));
        assert!(!same_file(&dir.path().join("kept"), None));
    }
}
