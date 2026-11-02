//! Several real host processes against one `index.db` (plan T20(b),
//! brwr-7vf acceptance check 1). Each host is the built `brauser serve`,
//! spoken to over native-messaging framing (a 4-byte little-endian length,
//! then JSON), and all of them share one injected config, home folder and
//! cache. They interleave `record_batch` (with retried batches),
//! `list_artifacts` and `get_artifact`; every response must be a success
//! with no SQLite busy or locked wording, and once they have all closed
//! (each close runs its own regeneration pass under `index-regen.lock`) the
//! index holds every delivered event exactly once and every artifact has
//! its file. Only temporary folders are used: the config path comes from
//! `BRAUSER_TEST_CONFIG_PATH`, which only the scripted build honours, and
//! the hosts never start the service.
#![cfg(feature = "scripted-dialogs")]

use std::collections::BTreeMap;
use std::fs;
use std::path::{Path, PathBuf};
use std::process::{Child, ChildStdin, ChildStdout, Command, Stdio};
use std::sync::Barrier;

use brauser_host::frame::{read_frame, write_request};
use brauser_protocol::{PROTOCOL_VERSION, Request};
use serde_json::{Value, json};
use time::OffsetDateTime;
use time::format_description::well_known::Rfc3339;

const HOST: &str = env!("CARGO_BIN_EXE_brauser");
const HOSTS: usize = 3;
const ROUNDS: usize = 10;
/// Few enough that one `list_artifacts` page (at most 32) shows them all.
const ARTIFACTS: usize = 6;
const EVENTS_PER_BATCH: usize = 2;

fn folder_identity(home: &Path) -> String {
    #[cfg(unix)]
    {
        use std::os::unix::fs::MetadataExt;
        let meta = fs::metadata(home).unwrap();
        format!("unix:{:016x}:{:016x}", meta.dev(), meta.ino())
    }
    #[cfg(windows)]
    {
        let handle = cap_std::fs::Dir::open_ambient_dir(home, cap_std::ambient_authority())
            .unwrap()
            .into_std_file();
        let info = winapi_util::file::information(&handle).unwrap();
        format!(
            "windows:{:08x}:{:016x}",
            info.volume_serial_number(),
            info.file_index()
        )
    }
}

struct World {
    _temp: tempfile::TempDir,
    home: PathBuf,
    config: PathBuf,
    logs: PathBuf,
}

impl World {
    fn new() -> Self {
        let temp = tempfile::tempdir().unwrap();
        let home = temp.path().join("home");
        fs::create_dir(&home).unwrap();
        let logs = temp.path().join("host-logs");
        fs::create_dir(&logs).unwrap();
        let config = temp.path().join("config.toml");
        let raw = json!({
            "root_picker_confirmed": true,
            "root_identity": folder_identity(&home),
            "capture_enabled": true,
            "storage": {
                "root": home.to_str().unwrap(),
                "profile": "neutral",
                "log_dir": "logs",
                "pages_dir": "notes",
                "later_dir": "later",
                "artifacts_dir": "artifacts",
                "summaries_dir": "summaries",
            },
            "adapters": {"format": 1},
            "artifacts": [{
                "type": "work.item",
                "origin": "https://work.example",
                "match": [{"path": "/items/{id:int}"}],
                "id": "work:{host}/{id}",
            }],
        });
        fs::write(&config, toml::to_string(&raw).unwrap()).unwrap();
        Self {
            _temp: temp,
            home,
            config,
            logs,
        }
    }

    fn serve(&self) -> Host {
        let mut child = Command::new(HOST)
            .arg("serve")
            .env("BRAUSER_TEST_CONFIG_PATH", &self.config)
            // Test builds log only where this says (diag.rs).
            .env("BRAUSER_LOG_DIR", &self.logs)
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::null())
            .spawn()
            .unwrap();
        Host {
            input: Some(child.stdin.take().unwrap()),
            output: child.stdout.take().unwrap(),
            child,
            next_id: 0,
        }
    }

    /// Every file under the home folder's `artifacts/`.
    fn artifact_files(&self) -> Vec<PathBuf> {
        fn walk(dir: &Path, found: &mut Vec<PathBuf>) {
            let Ok(entries) = fs::read_dir(dir) else {
                return;
            };
            for entry in entries {
                let path = entry.unwrap().path();
                if path.is_dir() {
                    walk(&path, found);
                } else {
                    found.push(path);
                }
            }
        }
        let mut found = Vec::new();
        walk(&self.home.join("artifacts"), &mut found);
        found
    }
}

struct Host {
    child: Child,
    input: Option<ChildStdin>,
    output: ChildStdout,
    next_id: u32,
}

impl Host {
    /// Send one request and return its reply, which must be of `expected`
    /// type and carry no SQLite busy or locked wording.
    fn call(&mut self, mut request: Value, expected: &str) -> Value {
        self.next_id += 1;
        let id = format!("r{}", self.next_id);
        request["protocol_version"] = json!(PROTOCOL_VERSION);
        request["request_id"] = json!(id);
        let request: Request = serde_json::from_value(request).unwrap();
        write_request(self.input.as_mut().unwrap(), &request).unwrap();
        let body = read_frame(&mut self.output).unwrap().unwrap();
        let reply: Value = serde_json::from_slice(&body).unwrap();
        assert_eq!(reply["request_id"], json!(id), "{reply}");
        assert_eq!(reply["type"], expected, "{reply}");
        let text = reply.to_string().to_lowercase();
        for word in ["locked", "busy", "sqlite"] {
            assert!(!text.contains(word), "{word} reached a response: {reply}");
        }
        reply
    }

    fn list(&mut self) -> Vec<Value> {
        let reply = self.call(
            json!({"type": "list_artifacts", "artifact_type": null, "title_query": null, "limit": 32}),
            "artifact_list_result",
        );
        assert_eq!(reply["truncated"], false, "{reply}");
        reply["items"].as_array().unwrap().clone()
    }

    fn close(mut self) {
        drop(self.input.take());
        assert!(self.child.wait().unwrap().success());
    }
}

impl Drop for Host {
    /// A test that panics mid-call must not leave a host writing into a
    /// temp folder that is being removed.
    fn drop(&mut self) {
        drop(self.input.take());
        let _ = self.child.kill();
        let _ = self.child.wait();
    }
}

fn url(artifact: usize) -> String {
    format!("https://work.example/items/{}", artifact + 1)
}

fn artifact_id(artifact: usize) -> String {
    format!("work:work.example/{}", artifact + 1)
}

fn uuid(kind: u32, host: usize, round: usize, item: usize) -> String {
    format!("00000000-0000-4{kind:03x}-8000-{host:04x}{round:04x}{item:04x}")
}

/// The batch `host` sends in `round`: its artifacts and the request.
fn batch(host: usize, round: usize, occurred_at: &str) -> (Vec<usize>, Value) {
    let artifacts = (0..EVENTS_PER_BATCH)
        .map(|item| (host * 2 + round + item) % ARTIFACTS)
        .collect::<Vec<_>>();
    let events = artifacts
        .iter()
        .enumerate()
        .map(|(item, &artifact)| {
            json!({
                "kind": "activity",
                "visit": null,
                "activity": {
                    "event_id": uuid(1, host, round, item),
                    "artifact_id": artifact_id(artifact),
                    "artifact_type": "work.item",
                    "url": url(artifact),
                    "title": format!("Work item {}", artifact + 1),
                    "occurred_at": occurred_at,
                    "incognito": false,
                    "opens": 1,
                    "focused_seconds": 60,
                    "background_seconds": 0,
                    "modes": [],
                    "trails": [],
                    "refs": [],
                },
            })
        })
        .collect::<Vec<_>>();
    let request = json!({
        "type": "record_batch",
        "batch_id": uuid(2, host, round, 0),
        "events": events,
        "overflow": null,
    });
    (artifacts, request)
}

/// Every event in the batch has this terminal acknowledgement.
fn assert_outcome(reply: &Value, outcome: &str, reason: Option<&str>) {
    let results = reply["results"].as_array().unwrap();
    assert_eq!(results.len(), EVENTS_PER_BATCH, "{reply}");
    for result in results {
        assert_eq!(result["outcome"], outcome, "{reply}");
        assert_eq!(result["reason"].as_str(), reason, "{reply}");
    }
}

#[test]
fn several_hosts_deliver_and_read_one_index_at_once() {
    let world = World::new();
    let occurred_at = OffsetDateTime::now_utc()
        .replace_minute(0)
        .unwrap()
        .replace_second(0)
        .unwrap()
        .replace_nanosecond(0)
        .unwrap()
        .format(&Rfc3339)
        .unwrap();
    let barrier = Barrier::new(HOSTS);
    // Each host's tally of the events it delivered, by artifact.
    let delivered = std::thread::scope(|scope| {
        let workers = (0..HOSTS)
            .map(|host| {
                let (world, barrier, occurred_at) = (&world, &barrier, &occurred_at);
                scope.spawn(move || {
                    let mut tally = BTreeMap::<usize, u32>::new();
                    let mut server = world.serve();
                    barrier.wait();
                    for round in 0..ROUNDS {
                        let (artifacts, request) = batch(host, round, occurred_at);
                        let reply = server.call(request.clone(), "batch_recorded");
                        assert_outcome(&reply, "persisted", None);
                        for artifact in artifacts {
                            *tally.entry(artifact).or_default() += 1;
                        }
                        // A lost acknowledgement: the same batch again, which
                        // is acknowledged and never counted twice.
                        if round % 3 == 1 {
                            let reply = server.call(request, "batch_recorded");
                            assert_outcome(&reply, "suppressed", Some("duplicate_event"));
                        }
                        server.list();
                        let artifact = (host + round) % ARTIFACTS;
                        let reply = server.call(
                            json!({"type": "get_artifact", "url": url(artifact), "title": null}),
                            "artifact_result",
                        );
                        assert_eq!(reply["recognized"], true, "{reply}");
                    }
                    // The panel's close runs a pass, or skips while another
                    // host holds `index-regen.lock`.
                    server.close();
                    tally
                })
            })
            .collect::<Vec<_>>();
        workers
            .into_iter()
            .map(|worker| worker.join().unwrap())
            .collect::<Vec<_>>()
    });
    let mut expected = BTreeMap::<String, u32>::new();
    for tally in delivered {
        for (artifact, count) in tally {
            *expected.entry(artifact_id(artifact)).or_default() += count;
        }
    }
    assert_eq!(expected.len(), ARTIFACTS);

    // Every delivered event is counted once, retries included, and the
    // final host's close regenerates whatever the skipped passes left.
    let mut last = world.serve();
    let listed = last
        .list()
        .into_iter()
        .map(|item| {
            (
                item["artifact_id"].as_str().unwrap().to_owned(),
                u32::try_from(item["opens"].as_u64().unwrap()).unwrap(),
            )
        })
        .collect::<BTreeMap<_, _>>();
    assert_eq!(listed, expected);
    for artifact in 0..ARTIFACTS {
        let reply = last.call(
            json!({"type": "get_artifact", "url": url(artifact), "title": null}),
            "artifact_result",
        );
        let view = &reply["artifact"];
        assert_eq!(view["artifact_id"], artifact_id(artifact), "{reply}");
        assert_eq!(view["opens"], expected[&artifact_id(artifact)], "{reply}");
        assert_eq!(
            view["focused_seconds"],
            60 * expected[&artifact_id(artifact)],
            "{reply}"
        );
    }
    last.close();
    assert_eq!(world.artifact_files().len(), ARTIFACTS);
    // No host logged a swallowed index failure (a delivery's index update,
    // a close pass) either.
    for entry in fs::read_dir(&world.logs).unwrap() {
        let log = fs::read_to_string(entry.unwrap().path()).unwrap();
        for line in log.lines() {
            let record: Value = serde_json::from_str(line).unwrap();
            assert!(
                record["level"] == "INFO" || record["level"] == "DEBUG",
                "{line}"
            );
        }
    }
}
