//! Multi-process harness for index concurrency tests (7vf).
//!
//! `spawn_children` re-executes this test binary once per child with
//! `--exact <mp_child> --nocapture --test-threads=1`, passing the child's
//! work through `BRAUSER_INDEX_MP_CHILD=<role>:<dir>:<seed>` on the child's
//! own `Command` environment. `mp_child` is a no-op without that variable,
//! so ordinary test runs skip it.
//!
//! Every child opens the same root and cache under `<dir>`, waits on the
//! barrier file so all start together, runs its role and writes one JSON
//! result into `<dir>/results`. Children never take `.config.lock`: they
//! call `ArtifactIndex` and `CaptureStore` directly, as a lock-free caller.

use super::*;
use std::cell::RefCell;
use std::process::{Child, Command, Stdio};

use crate::capture::{CapturePolicy, CaptureStore};

/// The child's work, set only on a spawned child's environment.
const CHILD_ENV: &str = "BRAUSER_INDEX_MP_CHILD";
/// Batches each deliverer sends, and passes each reader makes.
const BATCHES: u64 = 20;
/// Events in one delivered batch.
const BATCH_SIZE: u64 = 3;
/// Distinct artifacts the deliverers spread their events over, so
/// children contend on the same groups.
const ARTIFACTS: u64 = 4;
const BARRIER_TIMEOUT: Duration = Duration::from_secs(120);

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub(crate) enum Role {
    Deliver,
    Read,
    Regenerate,
    Reindex,
}

impl Role {
    fn name(self) -> &'static str {
        match self {
            Role::Deliver => "deliver",
            Role::Read => "read",
            Role::Regenerate => "regenerate",
            Role::Reindex => "reindex",
        }
    }

    fn parse(value: &str) -> Option<Self> {
        [Role::Deliver, Role::Read, Role::Regenerate, Role::Reindex]
            .into_iter()
            .find(|role| role.name() == value)
    }
}

/// What one child was asked to do. `dir` may itself contain `:` (a
/// Windows drive), so the role is split at the first colon and the seed
/// at the last.
#[derive(Debug, PartialEq, Eq)]
struct ChildSpec {
    role: Role,
    dir: PathBuf,
    seed: u64,
}

impl ChildSpec {
    fn encode(&self) -> String {
        format!(
            "{}:{}:{}",
            self.role.name(),
            self.dir.to_string_lossy(),
            self.seed
        )
    }

    fn parse(value: &str) -> Option<Self> {
        let (role, rest) = value.split_once(':')?;
        let (dir, seed) = rest.rsplit_once(':')?;
        if dir.is_empty() {
            return None;
        }
        Some(Self {
            role: Role::parse(role)?,
            dir: PathBuf::from(dir),
            seed: seed.parse().ok()?,
        })
    }
}

/// One `replace_artifact` call, as seen by the process that made it.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub(crate) struct RegenSpan {
    pub(crate) pid: u32,
    pub(crate) artifact: String,
    /// The dirty generation being cleared; `None` until the SQL store
    /// gives markers generations.
    pub(crate) generation: Option<u64>,
    pub(crate) start_ns: u64,
    pub(crate) end_ns: u64,
}

thread_local! {
    /// `Some` while this thread records regeneration spans. A child runs
    /// its role on the test thread, so a thread-local sees all of them
    /// and never another test's.
    static TRACE: RefCell<Option<Vec<RegenSpan>>> = const { RefCell::new(None) };
}

/// Wall-clock nanoseconds, comparable across processes on one host.
fn now_ns() -> u64 {
    SystemTime::now()
        .duration_since(SystemTime::UNIX_EPOCH)
        .map_or(0, |elapsed| {
            u64::try_from(elapsed.as_nanos()).unwrap_or(u64::MAX)
        })
}

/// Records the span of one artifact replacement when dropped, if this
/// thread is tracing. Held across `replace_artifact` (the cfg(test) hook).
pub(in crate::index) struct SpanGuard {
    artifact: String,
    generation: Option<u64>,
    start_ns: u64,
}

impl SpanGuard {
    pub(in crate::index) fn start(artifact: &str, generation: Option<u64>) -> Option<Self> {
        TRACE.with(|trace| trace.borrow().is_some()).then(|| Self {
            artifact: artifact.to_owned(),
            generation,
            start_ns: now_ns(),
        })
    }
}

impl Drop for SpanGuard {
    fn drop(&mut self) {
        let span = RegenSpan {
            pid: std::process::id(),
            artifact: std::mem::take(&mut self.artifact),
            generation: self.generation,
            start_ns: self.start_ns,
            end_ns: now_ns(),
        };
        TRACE.with(|trace| {
            if let Some(spans) = trace.borrow_mut().as_mut() {
                spans.push(span);
            }
        });
    }
}

/// Run `body` with this thread's regeneration trace on, returning the
/// spans it recorded.
fn traced<T>(body: impl FnOnce() -> T) -> (T, Vec<RegenSpan>) {
    TRACE.with(|trace| *trace.borrow_mut() = Some(Vec::new()));
    let output = body();
    let spans = TRACE.with(|trace| trace.borrow_mut().take().unwrap_or_default());
    (output, spans)
}

/// The JSON each child writes to `<dir>/results/<role>-<seed>.json`.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct ChildResult {
    pub(crate) role: Role,
    pub(crate) seed: u64,
    pub(crate) pid: u32,
    /// Operations the role attempted.
    pub(crate) ops: u64,
    /// The text of every failed operation.
    pub(crate) errors: Vec<String>,
    /// Event IDs this child made durable in the log (deliverers only).
    pub(crate) delivered: Vec<String>,
    pub(crate) trace: Vec<RegenSpan>,
}

/// The shared layout every child of one run opens.
pub(crate) struct Layout {
    pub(crate) root: PathBuf,
    pub(crate) cache: PathBuf,
    pub(crate) state: PathBuf,
    pub(crate) results: PathBuf,
    pub(crate) barrier: PathBuf,
}

impl Layout {
    pub(crate) fn new(dir: &Path) -> Self {
        Self {
            root: dir.join("root"),
            cache: dir.join("cache"),
            state: dir.join("state"),
            results: dir.join("results"),
            barrier: dir.join("barrier"),
        }
    }

    /// Create the directories a run needs; the barrier stays closed.
    pub(crate) fn prepare(dir: &Path) -> Self {
        let layout = Self::new(dir);
        for path in [&layout.root, &layout.cache, &layout.state, &layout.results] {
            fs::create_dir_all(path).unwrap();
        }
        layout
    }

    pub(crate) fn index(&self) -> ArtifactIndex {
        ArtifactIndex::open_in(&storage(&self.root), None, &self.cache).unwrap()
    }
}

/// The libtest name of `mp_child`, without the crate segment.
fn child_test_name() -> String {
    let module = module_path!();
    let module = module.split_once("::").map_or(module, |(_, rest)| rest);
    format!("{module}::mp_child")
}

/// Start `n` children of `role` against `dir`, seeded `seed..seed + n`.
/// They block on the barrier until `release(dir)`.
pub(crate) fn spawn_children(role: Role, n: u64, dir: &Path, seed: u64) -> Vec<Child> {
    let exe = std::env::current_exe().unwrap();
    (seed..seed + n)
        .map(|seed| {
            let spec = ChildSpec {
                role,
                dir: dir.to_path_buf(),
                seed,
            };
            Command::new(&exe)
                .args([
                    child_test_name().as_str(),
                    "--exact",
                    "--nocapture",
                    "--test-threads=1",
                ])
                .env(CHILD_ENV, spec.encode())
                .stdin(Stdio::null())
                .stdout(Stdio::piped())
                .stderr(Stdio::piped())
                .spawn()
                .unwrap()
        })
        .collect()
}

/// Open the barrier: every waiting child starts.
pub(crate) fn release(dir: &Path) {
    fs::write(Layout::new(dir).barrier, b"go").unwrap();
}

/// Wait for every child, require a clean exit, and read its result.
pub(crate) fn collect(children: Vec<Child>, dir: &Path) -> Vec<ChildResult> {
    let mut pids = Vec::new();
    for child in children {
        let pid = child.id();
        let output = child.wait_with_output().unwrap();
        assert!(
            output.status.success(),
            "child {pid} failed: {}\n{}",
            output.status,
            String::from_utf8_lossy(&output.stderr)
        );
        pids.push(pid);
    }
    let layout = Layout::new(dir);
    let mut results = fs::read_dir(&layout.results)
        .unwrap()
        .map(|entry| entry.unwrap().path())
        .filter(|path| path.extension().is_some_and(|ext| ext == "json"))
        .map(|path| serde_json::from_slice::<ChildResult>(&fs::read(path).unwrap()).unwrap())
        .collect::<Vec<_>>();
    results.sort_by_key(|result| (result.role.name(), result.seed));
    let mut seen = results.iter().map(|result| result.pid).collect::<Vec<_>>();
    seen.sort_unstable();
    pids.sort_unstable();
    assert_eq!(seen, pids, "every child writes exactly one result");
    results
}

/// The event ID a deliverer uses for event `n` of batch `batch`.
fn delivered_id(seed: u64, batch: u64, n: u64) -> String {
    let unique = (seed * BATCHES + batch) * BATCH_SIZE + n;
    format!("00000000-0000-4000-8000-{unique:012}")
}

fn deliverer_artifact(seed: u64, batch: u64, n: u64) -> String {
    format!("mp{}", (seed + batch + n) % ARTIFACTS)
}

fn wait_for_barrier(layout: &Layout) {
    let started = Instant::now();
    while !layout.barrier.exists() {
        assert!(
            started.elapsed() < BARRIER_TIMEOUT,
            "the barrier never opened"
        );
        std::thread::sleep(Duration::from_millis(5));
    }
}

fn run_child(spec: &ChildSpec) -> ChildResult {
    let layout = Layout::new(&spec.dir);
    let index = layout.index();
    let mut result = ChildResult {
        role: spec.role,
        seed: spec.seed,
        pid: std::process::id(),
        ops: 0,
        errors: Vec::new(),
        delivered: Vec::new(),
        trace: Vec::new(),
    };
    wait_for_barrier(&layout);
    let ((), trace) = traced(|| match spec.role {
        Role::Deliver => deliver(&layout, &index, spec.seed, &mut result),
        Role::Read => read(&index, &mut result),
        Role::Regenerate => {
            for _ in 0..BATCHES {
                result.ops += 1;
                if let Err(error) = index.regenerate_dirty() {
                    result.errors.push(format!("{error:#}"));
                }
            }
        }
        Role::Reindex => {
            result.ops += 1;
            if let Err(error) = index.reindex() {
                result.errors.push(format!("{error:#}"));
            }
        }
    });
    result.trace = trace;
    result
}

/// Append each batch to the log through `CaptureStore` (the capture lock,
/// never `.config.lock`), then index it, as the native host does.
fn deliver(layout: &Layout, index: &ArtifactIndex, seed: u64, result: &mut ChildResult) {
    let store = CaptureStore::open_in(&storage(&layout.root), None, &layout.state).unwrap();
    let origins = ["https://example.com".to_owned()];
    let strip_params = ["utm_source".to_owned()];
    let policy = CapturePolicy {
        enabled: true,
        origins: &origins,
        strip_params: &strip_params,
    };
    for batch in 0..BATCHES {
        let at = OffsetDateTime::now_utc()
            .replace_nanosecond(0)
            .unwrap()
            .format(&Rfc3339)
            .unwrap();
        let mut events = Vec::new();
        for n in 0..BATCH_SIZE {
            let id = delivered_id(seed, batch, n);
            let event = activity(&id, &deliverer_artifact(seed, batch, n), "Page", &at);
            match store.record_activity(policy, &event) {
                crate::capture::CaptureOutcome::Persisted { .. } => {
                    result.delivered.push(id);
                    events.push(event);
                }
                outcome => result.errors.push(format!("capture {id}: {outcome:?}")),
            }
        }
        result.ops += 1;
        if let Err(error) = index.record_activities(&events, &strip_params) {
            result.errors.push(format!("{error:#}"));
        }
    }
}

fn read(index: &ArtifactIndex, result: &mut ChildResult) {
    for batch in 0..BATCHES {
        result.ops += 1;
        if let Err(error) = index.recent(None, None, 200) {
            result.errors.push(format!("{error:#}"));
        }
        result.ops += 1;
        let id = format!("work:example.com/mp{}", batch % ARTIFACTS);
        if let Err(error) = index.artifact(&id) {
            result.errors.push(format!("{error:#}"));
        }
    }
}

fn write_result(spec: &ChildSpec, result: &ChildResult) {
    let layout = Layout::new(&spec.dir);
    let name = format!("{}-{}", spec.role.name(), spec.seed);
    let partial = layout.results.join(format!("{name}.partial"));
    fs::write(&partial, serde_json::to_vec_pretty(result).unwrap()).unwrap();
    fs::rename(&partial, layout.results.join(format!("{name}.json"))).unwrap();
}

/// The child entry point. A no-op unless a harness spawned this process.
#[test]
fn mp_child() {
    let Some(value) = std::env::var_os(CHILD_ENV) else {
        return;
    };
    let value = value.to_str().expect("child spec is UTF-8");
    let spec = ChildSpec::parse(value).unwrap_or_else(|| panic!("bad child spec {value:?}"));
    let result = run_child(&spec);
    write_result(&spec, &result);
}

#[test]
fn child_spec_round_trips_paths_with_colons() {
    for dir in [
        r"C:\Users\me\AppData\Local\Temp\.tmpX",
        "/tmp/a:b",
        "/tmp/plain",
    ] {
        for role in [Role::Deliver, Role::Read, Role::Regenerate, Role::Reindex] {
            let spec = ChildSpec {
                role,
                dir: PathBuf::from(dir),
                seed: 42,
            };
            assert_eq!(ChildSpec::parse(&spec.encode()), Some(spec));
        }
    }
    for bad in [
        "",
        "deliver",
        "deliver:/tmp",
        "write:/tmp:1",
        "read::1",
        "read:/tmp:x",
    ] {
        assert_eq!(ChildSpec::parse(bad), None, "{bad:?}");
    }
}

#[test]
fn child_test_name_is_the_libtest_path() {
    assert_eq!(child_test_name(), "index::tests::mp_harness::mp_child");
}

#[test]
fn replace_artifact_records_a_span_only_while_tracing() {
    let root = tempfile::tempdir().unwrap();
    let cache = tempfile::tempdir().unwrap();
    let index = index(&root, &cache);
    let event = activity(
        "11111111-1111-4111-8111-111111111111",
        "traced",
        "Traced",
        "2026-04-01T10:00:00Z",
    );
    write_log(root.path(), "2026-04-01.md", &[&event]);
    index.record_activity(&event, &[]).unwrap();

    let before = now_ns();
    let (report, spans) = traced(|| index.regenerate_dirty().unwrap());
    assert_eq!(report.written, 1);
    assert_eq!(spans.len(), 1, "{spans:?}");
    let span = &spans[0];
    assert_eq!(span.artifact, "work:example.com/traced");
    assert_eq!(span.pid, std::process::id());
    assert_eq!(span.generation, None);
    assert!(before <= span.start_ns && span.start_ns <= span.end_ns);

    // Off by default: an untraced pass records nothing anywhere.
    let second = activity(
        "22222222-2222-4222-8222-222222222222",
        "traced",
        "Traced",
        "2026-04-01T11:00:00Z",
    );
    write_log(root.path(), "2026-04-01.md", &[&event, &second]);
    index.record_activity(&second, &[]).unwrap();
    assert_eq!(index.regenerate_dirty().unwrap().written, 1);
    assert!(TRACE.with(|trace| trace.borrow().is_none()));
}

/// The skeleton run against today's index: two deliverers and two readers
/// share one root and cache with no config lock. 7vf (T20) removes the
/// `ignore` and adds the regenerate and reindex roles and its assertions.
#[test]
#[ignore = "multi-process skeleton; T20 turns it into the acceptance test"]
fn mp_deliver_and_read_share_one_cache() {
    let dir = tempfile::tempdir().unwrap();
    let layout = Layout::prepare(dir.path());
    let mut children = spawn_children(Role::Deliver, 2, dir.path(), 0);
    children.extend(spawn_children(Role::Read, 2, dir.path(), 100));
    release(dir.path());
    let results = collect(children, dir.path());

    assert_eq!(results.len(), 4);
    for result in &results {
        assert_eq!(
            result.ops,
            BATCHES * if result.role == Role::Read { 2 } else { 1 }
        );
        assert!(result.trace.is_empty(), "no role here regenerates");
    }
    let delivered = results
        .iter()
        .flat_map(|result| result.delivered.iter().cloned())
        .collect::<BTreeSet<_>>();
    assert_eq!(delivered.len() as u64, 2 * BATCHES * BATCH_SIZE);
    let errors = results
        .iter()
        .flat_map(|result| result.errors.iter())
        .collect::<Vec<_>>();
    eprintln!("errors against today's index: {}", errors.len());
    for error in errors.iter().take(10) {
        eprintln!("  {error}");
    }

    // Every logged event is indexed once the logs are reconciled.
    let index = layout.index();
    index.reconcile_from_logs().unwrap();
    let opens = (0..ARTIFACTS)
        .filter_map(|n| index.artifact(&format!("work:example.com/mp{n}")).unwrap())
        .map(|view| u64::from(view.opens))
        .sum::<u64>();
    assert_eq!(opens, 2 * BATCHES * BATCH_SIZE);
}
