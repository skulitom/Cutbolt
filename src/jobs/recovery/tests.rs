use super::*;

struct Fixture(PathBuf);
impl Fixture {
    fn new() -> Self {
        let stamp = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_nanos();
        let root = std::env::temp_dir().join(format!(
            "cutbolt-publication-{}-{stamp}",
            std::process::id()
        ));
        std::fs::create_dir(&root).unwrap();
        Self(root.canonicalize().unwrap())
    }
}
impl Drop for Fixture {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.0);
    }
}

fn seed(root: &Path, maximum: i64) -> (Connection, Value) {
    seed_at(root, maximum, false)
}
fn seed_at(root: &Path, maximum: i64, fractional: bool) -> (Connection, Value) {
    let connection = connect(root, true).unwrap();
    let output = root.join("final.mkv");
    let (rate, duration, frames, samples) = if fractional {
        (
            json!({"num":30000,"den":1001}),
            json!({"num":1001,"den":6000}),
            5,
            8008,
        )
    } else {
        (
            json!({"num":25,"den":1}),
            json!({"num":1,"den":25}),
            1,
            1920,
        )
    };
    let project:Project=serde_json::from_value(json!({"schema_version":1,"id":"original-test","revision":0,"width":16,"height":16,"frame_rate":rate,"assets":[],"clips":[{"id":"gap","gap":true,"source_in":{"num":0,"den":1},"duration":duration}]})).unwrap();
    let saved = SavedRequest {
        render: RenderRequest {
            project,
            input_root: root.into(),
            output_root: root.into(),
            output: output.clone(),
            retry: None,
        },
        ffmpeg: "not-used".into(),
        ffprobe: "not-used".into(),
        ffmpeg_sha256: None,
        ffprobe_sha256: None,
    };
    let encoded = serde_json::to_string(&saved).unwrap();
    connection.execute("INSERT INTO jobs(id,request_id,payload_hash,request,request_hash,ticket,output,status,phase,total_frames,attempt,max_attempts)
        VALUES('one','one','original',?1,?2,'{}',?3,'running','verifying',?5,1,?4)",params![encoded,digest(encoded.as_bytes()),output.to_string_lossy(),maximum,frames]).unwrap();
    begin_attempt(&connection, "one").unwrap();
    let bytes = b"original publication fixture content";
    std::fs::write(root.join(partial_name("one", 1)), bytes).unwrap();
    let receipt = json!({"output":output,"sha256":digest(bytes),"frames":frames,"samples":samples,"project_revision":0});
    (connection, receipt)
}

fn publish_fixture(root: &Path, connection: &Connection, receipt: &Value) {
    let control = JobControl {
        connection,
        id: "one",
        checked: Cell::new(Instant::now()),
        ffmpeg: "not-used",
        ffprobe: "not-used",
    };
    control
        .publish(
            &root.join(partial_name("one", 1)),
            &root.join("final.mkv"),
            receipt,
        )
        .unwrap();
}

#[test]
fn crash_child() {
    let Ok(directory) = std::env::var("CUTBOLT_TEST_PUBLICATION_ROOT") else {
        return;
    };
    let root = PathBuf::from(directory);
    let (connection, receipt) = seed_at(
        &root,
        2,
        std::env::var("CUTBOLT_TEST_NATIVE_RATE").as_deref() == Ok("fractional"),
    );
    publish_fixture(&root, &connection, &receipt);
    panic!("Requested interruption was not reached");
}

fn crash(root: &Path, stage: &str) {
    crash_at(root, stage, false);
}
fn crash_at(root: &Path, stage: &str, fractional: bool) {
    let status = Command::new(std::env::current_exe().unwrap())
        .args([
            "--exact",
            "jobs::recovery::tests::crash_child",
            "--nocapture",
        ])
        .env("CUTBOLT_TEST_PUBLICATION_ROOT", root)
        .env("CUTBOLT_TEST_PUBLICATION_CRASH", stage)
        .env(
            "CUTBOLT_TEST_NATIVE_RATE",
            if fractional { "fractional" } else { "original" },
        )
        .status()
        .unwrap();
    assert_eq!(status.code(), Some(89));
}

#[test]
fn fractional_publication_crash_recovers_exact_frame_and_audio_clocks() {
    let fixture = Fixture::new();
    crash_at(&fixture.0, "published", true);
    let output = fixture.0.join("final.mkv");
    let before = std::fs::read(&output).unwrap();
    let connection = connect(&fixture.0, false).unwrap();
    reconcile(&connection).unwrap();
    let saved = state(&connection, "one").unwrap();
    assert_eq!(saved["status"], "completed");
    assert_eq!(saved["attempts"]["current"], 1);
    assert_eq!(saved["result"]["frames"], 5);
    assert_eq!(saved["result"]["samples"], 8008);
    assert_eq!(std::fs::read(output).unwrap(), before);
}

#[test]
fn published_crash_reconciles_without_reencoding_or_overwrite() {
    let fixture = Fixture::new();
    crash(&fixture.0, "published");
    let output = fixture.0.join("final.mkv");
    let original = std::fs::read(&output).unwrap();
    let connection = connect(&fixture.0, false).unwrap();
    assert_eq!(state(&connection, "one").unwrap()["status"], "running");
    reconcile(&connection).unwrap();
    let state = state(&connection, "one").unwrap();
    assert_eq!(state["status"], "completed");
    assert_eq!(state["attempts"]["current"], 1);
    assert_eq!(state["result"]["sha256"], digest(&original));
    reconcile(&connection).unwrap();
    assert_eq!(std::fs::read(output).unwrap(), original);
    assert!(fixture.0.join(partial_name("one", 1)).exists());
}

#[test]
fn intent_crash_requeues_only_opted_in_and_cancellation_wins() {
    for cancelled in [false, true] {
        let fixture = Fixture::new();
        crash(&fixture.0, "intent");
        let connection = connect(&fixture.0, false).unwrap();
        if cancelled {
            connection
                .execute("UPDATE jobs SET cancel_requested=1", [])
                .unwrap();
        }
        reconcile(&connection).unwrap();
        let value = state(&connection, "one").unwrap();
        assert_eq!(
            value["status"],
            if cancelled { "cancelled" } else { "queued" }
        );
        assert_eq!(
            value["attempts"]["history"][0]["status"],
            if cancelled {
                "cancelled"
            } else {
                "interrupted"
            }
        );
        assert!(!fixture.0.join("final.mkv").exists());
        assert!(fixture.0.join(partial_name("one", 1)).exists());
        assert_ne!(partial_name("one", 1), partial_name("one", 2));
    }
    let fixture = Fixture::new();
    let (connection, _) = seed(&fixture.0, 1);
    reconcile(&connection).unwrap();
    assert_eq!(state(&connection, "one").unwrap()["status"], "interrupted");
}

#[test]
fn publication_conflicts_and_corrupt_receipts_are_preserved_and_reject() {
    for corrupt_receipt in [false, true] {
        let fixture = Fixture::new();
        crash(&fixture.0, "published");
        let connection = connect(&fixture.0, false).unwrap();
        if corrupt_receipt {
            connection
                .execute("UPDATE job_publications SET receipt_hash='changed'", [])
                .unwrap();
        } else {
            std::fs::write(fixture.0.join("final.mkv"), b"unrelated content").unwrap();
        }
        let before = std::fs::read(fixture.0.join("final.mkv")).unwrap();
        reconcile(&connection).unwrap();
        let value = state(&connection, "one").unwrap();
        assert_eq!(value["status"], "failed");
        assert_eq!(
            value["error"]["code"],
            if corrupt_receipt {
                "STORE_CORRUPT"
            } else {
                "PUBLICATION_CONFLICT"
            }
        );
        assert_eq!(std::fs::read(fixture.0.join("final.mkv")).unwrap(), before);
    }
}

#[test]
fn bounded_attempts_pin_sources_and_keep_failure_history() {
    let fixture = Fixture::new();
    let (connection, _) = seed(&fixture.0, 3);
    let mut sources = vec![render::Source {
        path: fixture.0.join("source.mkv"),
        sha256: digest(b"original"),
        frames: 1,
        samples: 1920,
    }];
    pin_sources(&connection, "one", &sources).unwrap();
    pin_sources(&connection, "one", &sources).unwrap();
    sources[0].sha256 = digest(b"changed");
    assert_eq!(
        pin_sources(&connection, "one", &sources).unwrap_err().code,
        "MEDIA_CHANGED"
    );
    for attempt in 1..=3 {
        failed(
            &connection,
            "one",
            &error("TOOL_FAILED", "Original injected transient failure"),
        )
        .unwrap();
        let value = state(&connection, "one").unwrap();
        assert_eq!(
            value["attempts"]["history"].as_array().unwrap().len(),
            attempt
        );
        assert_eq!(
            value["status"],
            if attempt < 3 { "queued" } else { "failed" }
        );
        if attempt < 3 {
            connection
                .execute("UPDATE jobs SET attempt=attempt+1,status='running'", [])
                .unwrap();
            begin_attempt(&connection, "one").unwrap();
        }
    }
}

fn downgrade_fixture(connection: &Connection) {
    // Recreate the exact earlier queue column surface, retaining its original row.
    connection.execute_batch("DROP TABLE job_attempts; DROP TABLE job_publications;
        ALTER TABLE jobs DROP COLUMN attempt; ALTER TABLE jobs DROP COLUMN max_attempts;
        ALTER TABLE jobs DROP COLUMN source_manifest; ALTER TABLE jobs DROP COLUMN source_manifest_hash;
        PRAGMA user_version=1;").unwrap();
}

#[test]
fn version_one_queue_migration_preserves_tickets_requests_and_default_retry_policy() {
    for status in ["queued", "completed", "running"] {
        let fixture = Fixture::new();
        let (connection, receipt) = seed(&fixture.0, 1);
        connection
            .execute(
                "UPDATE jobs SET status=?1,phase=?1,result=?2",
                params![status, receipt.to_string()],
            )
            .unwrap();
        let before: (String, String, String, String) = connection
            .query_row(
                "SELECT request,request_hash,payload_hash,ticket FROM jobs",
                [],
                |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?, r.get(3)?)),
            )
            .unwrap();
        downgrade_fixture(&connection);
        drop(connection);
        let connection = connect(&fixture.0, false).unwrap();
        assert_eq!(
            connection
                .pragma_query_value(None, "user_version", |r| r.get::<_, i64>(0))
                .unwrap(),
            2
        );
        let after: (String, String, String, String) = connection
            .query_row(
                "SELECT request,request_hash,payload_hash,ticket FROM jobs",
                [],
                |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?, r.get(3)?)),
            )
            .unwrap();
        assert_eq!(before, after);
        let value = state(&connection, "one").unwrap();
        assert_eq!(value["status"], status);
        assert_eq!(value["result"], receipt);
        assert_eq!(value["attempts"]["maximum"], 1);
        assert_eq!(
            value["attempts"]["current"],
            if status == "queued" { 0 } else { 1 }
        );
        reconcile(&connection).unwrap();
        assert_eq!(
            state(&connection, "one").unwrap()["status"],
            if status == "running" {
                "interrupted"
            } else {
                status
            }
        );
    }
}

#[test]
fn failed_queue_migration_rolls_back_schema_and_preserves_rows() {
    let fixture = Fixture::new();
    let (connection, _) = seed(&fixture.0, 1);
    downgrade_fixture(&connection);
    connection.execute_batch("CREATE TABLE job_attempts(original_fixture TEXT); INSERT INTO job_attempts VALUES('retain');").unwrap();
    drop(connection);
    assert!(connect(&fixture.0, false).is_err());
    let connection = Connection::open(fixture.0.join("jobs.sqlite3")).unwrap();
    assert_eq!(
        connection
            .pragma_query_value(None, "user_version", |r| r.get::<_, i64>(0))
            .unwrap(),
        1
    );
    assert_eq!(
        connection
            .query_row("SELECT original_fixture FROM job_attempts", [], |r| r
                .get::<_, String>(0))
            .unwrap(),
        "retain"
    );
    assert_eq!(
        connection
            .query_row(
                "SELECT count(*) FROM pragma_table_info('jobs') WHERE name='attempt'",
                [],
                |r| r.get::<_, i64>(0)
            )
            .unwrap(),
        0
    );
    assert_eq!(
        connection
            .query_row("SELECT count(*) FROM jobs", [], |r| r.get::<_, i64>(0))
            .unwrap(),
        1
    );
}
