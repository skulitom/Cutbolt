use super::*;
use crate::model::Asset;
use std::{
    fs,
    path::PathBuf,
    process::Command,
    sync::atomic::{AtomicU64, Ordering},
};
static NEXT: AtomicU64 = AtomicU64::new(0);
struct Temp(PathBuf);
impl Temp {
    fn new() -> Self {
        let path = std::env::temp_dir().join(format!(
            "cutbolt-history-test-{}-{}",
            std::process::id(),
            NEXT.fetch_add(1, Ordering::SeqCst)
        ));
        fs::create_dir(&path).unwrap();
        Self(path)
    }
}
impl Drop for Temp {
    fn drop(&mut self) {
        assert!(self.0.starts_with(std::env::temp_dir()));
        let _ = fs::remove_dir_all(&self.0);
    }
}
fn sample() -> Project {
    let mut project = Project::new("history".into(), 32, 24, Time::new(25, 1).unwrap()).unwrap();
    project.assets.push(Asset {
        id: "a".into(),
        path: "media/source.mkv".into(),
        duration: Time::new(20, 1).unwrap(),
        metadata: Default::default(),
        identity: None,
        proxy: None,
    });
    project.clips.push(Clip {
        id: "c".into(),
        asset_id: Some("a".into()),
        gap: false,
        source_in: Time::ZERO,
        duration: Time::new(10, 1).unwrap(),
    });
    project
}
fn seed(legacy: bool) -> Temp {
    let root = Temp::new();
    create(&root.0, sample(), "create").unwrap();
    for index in 0..16 {
        mutate(
            &root.0,
            "history",
            &format!("edit-{index}"),
            index,
            Mutation::Apply {
                operations: vec![Operation::Metadata {
                    asset_id: "a".into(),
                    metadata: crate::registry::Metadata {
                        title: format!("Original title {index}"),
                        ..Default::default()
                    },
                }],
            },
        )
        .unwrap();
    }
    if legacy {
        let conn = connect(&root.0, false).unwrap();
        conn.execute_batch("DROP TABLE revision_integrity; PRAGMA user_version=1; VACUUM;")
            .unwrap();
    }
    root
}
fn rows(root: &Path) -> Vec<String> {
    let conn = connect(root, false).unwrap();
    let mut output = Vec::new();
    for table in ["projects", "revisions", "requests"] {
        let mut statement = conn
            .prepare(&format!("SELECT * FROM {table} ORDER BY 1,2"))
            .unwrap();
        let count = statement.column_count();
        let values = statement
            .query_map([], |row| {
                Ok((0..count)
                    .map(|i| format!("{:?}", row.get_ref(i).unwrap()))
                    .collect::<Vec<_>>()
                    .join("|"))
            })
            .unwrap();
        output.extend(values.map(|r| r.unwrap()));
    }
    output
}
fn child(root: &Path, kind: &str, point: &str) {
    let output = Command::new(std::env::current_exe().unwrap())
        .args([
            "--exact",
            "store::integrity::tests::crash_worker",
            "--nocapture",
        ])
        .env("CUTBOLT_HISTORY_TEST_ROOT", root)
        .env("CUTBOLT_HISTORY_TEST_KIND", kind)
        .env("VIDEO_ENGINE_TEST_POINT", point)
        .output()
        .unwrap();
    assert_eq!(output.status.code(), Some(86), "{output:?}");
}
#[test]
fn crash_worker() {
    let Some(root) = std::env::var_os("CUTBOLT_HISTORY_TEST_ROOT") else {
        return;
    };
    let root = PathBuf::from(root);
    match std::env::var("CUTBOLT_HISTORY_TEST_KIND").unwrap().as_str() {
        "migrate" => {
            migrate(&root).unwrap();
        }
        "backup" => {
            backup(&Backup {
                store_root: root.clone(),
                output_root: root.clone(),
                output: root.join("saved.sqlite3"),
            })
            .unwrap();
        }
        "recover" => {
            let source = root.join("saved.sqlite3");
            recover(&Recover {
                source: crate::scene::Identity {
                    path: source.clone(),
                    bytes: source.metadata().unwrap().len(),
                    sha256: crate::media::file_hash(&source).unwrap(),
                },
                input_root: root.clone(),
                store_root: root.join("recovered"),
            })
            .unwrap();
        }
        _ => panic!("Unknown original crash fixture"),
    };
    panic!("Crash injection did not fire");
}
#[test]
fn explicit_migration_crashes_preserve_history_and_atomic_schema() {
    for point in [
        "migration_after_seal",
        "migration_before_commit",
        "migration_after_commit",
    ] {
        let root = seed(true);
        let before = rows(&root.0);
        child(&root.0, "migrate", point);
        let conn = connect(&root.0, false).unwrap();
        assert_eq!(
            version(&conn).unwrap(),
            if point == "migration_after_commit" {
                2
            } else {
                1
            }
        );
        drop(conn);
        assert_eq!(rows(&root.0), before);
        assert!(check(&root.0).unwrap()["valid"].as_bool().unwrap());
        migrate(&root.0).unwrap();
        assert_eq!(rows(&root.0), before);
        mutate(
            &root.0,
            "history",
            "undo-after-migration",
            16,
            Mutation::Undo,
        )
        .unwrap();
        assert_eq!(
            get(&root.0, "history", None).unwrap().assets[0]
                .metadata
                .title,
            "Original title 14"
        );
    }
}
#[test]
fn full_database_migration_rolls_back_schema_and_all_original_rows() {
    let root = seed(true);
    let before = rows(&root.0);
    let mut conn = connect(&root.0, false).unwrap();
    let pages: i64 = conn
        .pragma_query_value(None, "page_count", |r| r.get(0))
        .unwrap();
    conn.pragma_update(None, "max_page_count", pages).unwrap();
    assert_eq!(migrate_connection(&mut conn).unwrap_err().code, "DISK_FULL");
    assert_eq!(version(&conn).unwrap(), 1);
    assert!(
        !conn
            .prepare("SELECT name FROM sqlite_schema WHERE name='revision_integrity'")
            .unwrap()
            .exists([])
            .unwrap()
    );
    drop(conn);
    assert_eq!(rows(&root.0), before);
    migrate(&root.0).unwrap();
    assert_eq!(rows(&root.0), before);
}
#[test]
fn revision_metadata_and_head_corruption_reject_saved_reads() {
    for sql in [
        "UPDATE revisions SET undo_target=0 WHERE revision=16",
        "UPDATE projects SET head=15",
        "UPDATE revisions SET action='restore' WHERE revision=16",
        "UPDATE requests SET receipt='{}' WHERE request_id='edit-15'",
        "UPDATE requests SET payload_hash='changed' WHERE request_id='edit-15'",
    ] {
        let root = seed(false);
        let conn = connect(&root.0, false).unwrap();
        conn.execute_batch(sql).unwrap();
        drop(conn);
        assert_eq!(
            get(&root.0, "history", None).unwrap_err().code,
            "STORE_CORRUPT"
        );
        assert_eq!(check(&root.0).unwrap_err().code, "STORE_CORRUPT");
    }
}

#[test]
fn invalid_legacy_history_rejects_without_partial_migration_or_repair() {
    for (mutation, expected) in [
        (
            "UPDATE revisions SET undo_target=0 WHERE revision=16",
            "STORE_CORRUPT",
        ),
        (
            "CREATE TRIGGER projects AFTER UPDATE ON projects BEGIN SELECT 1; END",
            "UNSUPPORTED_STORE",
        ),
    ] {
        let root = seed(true);
        let conn = connect(&root.0, false).unwrap();
        conn.execute_batch(mutation).unwrap();
        drop(conn);
        let before = rows(&root.0);
        assert_eq!(migrate(&root.0).unwrap_err().code, expected);
        let conn = connect(&root.0, false).unwrap();
        assert_eq!(version(&conn).unwrap(), 1);
        assert!(
            !conn
                .prepare("SELECT name FROM sqlite_schema WHERE name='revision_integrity'")
                .unwrap()
                .exists([])
                .unwrap()
        );
        drop(conn);
        assert_eq!(rows(&root.0), before);
    }
}
#[test]
fn backup_process_crash_publishes_only_a_complete_checked_history() {
    for point in ["backup_before_publish", "backup_after_publish"] {
        let root = seed(false);
        let before = rows(&root.0);
        child(&root.0, "backup", point);
        assert_eq!(rows(&root.0), before);
        let saved = root.0.join("saved.sqlite3");
        assert_eq!(saved.exists(), point == "backup_after_publish");
        if saved.exists() {
            let conn =
                Connection::open_with_flags(saved, OpenFlags::SQLITE_OPEN_READ_ONLY).unwrap();
            assert_eq!(validate(&conn).unwrap()["revisions"], 17);
        }
    }
}
#[test]
fn recovery_process_crash_preserves_backup_and_never_exposes_partial_database() {
    for point in ["recovery_before_publish", "recovery_after_publish"] {
        let root = seed(false);
        let saved = root.0.join("saved.sqlite3");
        backup(&Backup {
            store_root: root.0.clone(),
            output_root: root.0.clone(),
            output: saved.clone(),
        })
        .unwrap();
        let hash = crate::media::file_hash(&saved).unwrap();
        let recovered = root.0.join("recovered");
        fs::create_dir(&recovered).unwrap();
        child(&root.0, "recover", point);
        assert_eq!(crate::media::file_hash(&saved).unwrap(), hash);
        assert_eq!(
            recovered.join("projects.sqlite3").exists(),
            point == "recovery_after_publish"
        );
        if point == "recovery_after_publish" {
            assert_eq!(rows(&recovered), rows(&root.0));
            assert_eq!(check(&recovered).unwrap()["revisions"], 17);
        }
    }
}
