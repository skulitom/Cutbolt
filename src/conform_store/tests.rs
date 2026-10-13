use super::*;

struct Fixture(PathBuf);
impl Fixture {
    fn new() -> Self {
        let stamp = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_nanos();
        let root = std::env::temp_dir().join(format!(
            "cutbolt-conform-store-test-{}-{stamp}",
            std::process::id()
        ));
        fs::create_dir(&root).unwrap();
        Self(root)
    }
}
impl Drop for Fixture {
    fn drop(&mut self) {
        let _ = fs::remove_dir_all(&self.0);
    }
}
fn put(store: &Store, key: &str, bytes: &[u8], policy: &Policy) {
    let out = store.begin(key, "mkv", policy).unwrap();
    store.reserve(key, bytes.len() as u64, policy).unwrap();
    fs::write(&out, bytes).unwrap();
    store
        .finish(
            key,
            "mkv",
            &out,
            &Identity {
                bytes: bytes.len() as u64,
                sha256: digest(bytes),
            },
            &json!({"original":true}),
        )
        .unwrap();
}
#[test]
fn reservation_lru_integrity_and_zero_prune() {
    let root = Fixture::new();
    let mut store = Store::open(&root.0, true, true).unwrap();
    let policy = Policy {
        max_bytes: 100,
        max_entries: 3,
    };
    let (a, b, c) = (digest(b"a"), digest(b"b"), digest(b"c"));
    put(&store, &a, &[1; 35], &policy);
    put(&store, &b, &[2; 35], &policy);
    assert!(store.read(&a).unwrap().is_some());
    let out = store.begin(&c, "mkv", &policy).unwrap();
    store.reserve(&c, 40, &policy).unwrap();
    assert!(store.read(&b).unwrap().is_none());
    assert!(store.read(&a).unwrap().is_some());
    assert_eq!(store.inspect().unwrap()["payload_bytes"], 75);
    fs::write(&out, [3; 40]).unwrap();
    store
        .finish(
            &c,
            "mkv",
            &out,
            &Identity {
                bytes: 40,
                sha256: digest(&[3; 40]),
            },
            &json!({"original":true}),
        )
        .unwrap();
    let first = store.read(&a).unwrap().unwrap();
    fs::write(first.path, [9; 35]).unwrap();
    assert_eq!(store.read(&a).err().unwrap().code, "CACHE_CORRUPT");
    store.discard(&a).unwrap();
    store
        .connection
        .execute("UPDATE entries SET metadata='{}' WHERE key=?1", [&c])
        .unwrap();
    assert_eq!(store.read(&c).err().unwrap().code, "CACHE_CORRUPT");
    store
        .prune(&Policy {
            max_bytes: 0,
            max_entries: 0,
        })
        .unwrap();
    assert_eq!(store.inspect().unwrap()["payload_bytes"], 0);
}
#[test]
fn interrupted_production_reclaims_only_indexed_staging() {
    let root = Fixture::new();
    let keep = root.0.join("user-original.txt");
    fs::write(&keep, b"preserve").unwrap();
    let key = digest(b"partial");
    let policy = Policy {
        max_bytes: 100,
        max_entries: 3,
    };
    {
        let store = Store::open(&root.0, true, true).unwrap();
        let out = store.begin(&key, "mkv", &policy).unwrap();
        store.reserve(&key, 70, &policy).unwrap();
        fs::write(&out, [4; 70]).unwrap();
        fs::hard_link(&out, store.path(&key, "mkv").unwrap()).unwrap(); // crash after publication, before ready transaction
    }
    let store = Store::open(&root.0, true, false).unwrap();
    assert_eq!(store.inspect().unwrap()["payload_bytes"], 0);
    assert!(!store.stage(&key).unwrap().exists());
    assert!(!store.path(&key, "mkv").unwrap().exists());
    assert_eq!(fs::read(keep).unwrap(), b"preserve");
}
#[test]
fn foreign_files_and_unindexed_destinations_are_preserved() {
    let root = Fixture::new();
    let lock = root.0.join(LOCK);
    fs::write(&lock, b"foreign").unwrap();
    assert_eq!(
        Store::open(&root.0, true, true).err().unwrap().code,
        "INVALID_CACHE"
    );
    assert_eq!(fs::read(&lock).unwrap(), b"foreign");
    fs::remove_file(lock).unwrap();
    let db = root.0.join(INDEX);
    fs::write(&db, b"foreign database").unwrap();
    assert_eq!(
        Store::open(&root.0, true, true).err().unwrap().code,
        "INVALID_CACHE"
    );
    assert_eq!(fs::read(&db).unwrap(), b"foreign database");
    fs::remove_file(db).unwrap();
    let store = Store::open(&root.0, true, true).unwrap();
    let key = digest(b"collision");
    let output = store.path(&key, "mkv").unwrap();
    fs::write(&output, b"unindexed").unwrap();
    assert_eq!(
        store
            .begin(
                &key,
                "mkv",
                &Policy {
                    max_bytes: 100,
                    max_entries: 2
                }
            )
            .unwrap_err()
            .code,
        "INVALID_CACHE"
    );
    assert_eq!(fs::read(output).unwrap(), b"unindexed");
}
#[test]
fn reader_lease_blocks_cross_process_eviction() {
    let root = Fixture::new();
    let key = digest(b"leased");
    {
        let store = Store::open(&root.0, true, true).unwrap();
        put(
            &store,
            &key,
            b"derived",
            &Policy {
                max_bytes: 100,
                max_entries: 2,
            },
        );
    }
    let mut read = Store::open(&root.0, false, false).unwrap();
    let entry = read.read(&key).unwrap().unwrap();
    let status = std::process::Command::new(std::env::current_exe().unwrap())
        .args([
            "--exact",
            "conform_store::tests::lease_child",
            "--nocapture",
        ])
        .env("CUTBOLT_CONFORM_LOCK_TEST_ROOT", &root.0)
        .status()
        .unwrap();
    assert!(status.success());
    assert!(entry.path.exists());
    drop(read);
    let store = Store::open(&root.0, true, false).unwrap();
    store
        .prune(&Policy {
            max_bytes: 0,
            max_entries: 0,
        })
        .unwrap();
    assert!(!entry.path.exists());
}
#[test]
fn lease_child() {
    let Some(root) = std::env::var_os("CUTBOLT_CONFORM_LOCK_TEST_ROOT") else {
        return;
    };
    let file = OpenOptions::new()
        .read(true)
        .write(true)
        .open(PathBuf::from(root).join(LOCK))
        .unwrap();
    assert!(matches!(
        file.try_lock(),
        Err(std::fs::TryLockError::WouldBlock)
    ));
    file.try_lock_shared().unwrap();
}
#[test]
fn hard_reservation_refuses_oversize_and_large_policy_is_separate() {
    let root = Fixture::new();
    let store = Store::open(&root.0, true, true).unwrap();
    let key = digest(b"budget");
    let policy = Policy {
        max_bytes: 17,
        max_entries: 1,
    };
    let output = store.begin(&key, "wav", &policy).unwrap();
    assert_eq!(
        store.reserve(&key, 18, &policy).unwrap_err().code,
        "LIMIT_EXCEEDED"
    );
    assert!(!output.exists());
    validate(&Policy {
        max_bytes: 50_000_000_000,
        max_entries: 4096,
    })
    .unwrap();
    assert!(
        Policy {
            max_bytes: 50_000_000_000,
            max_entries: 4096
        }
        .validate()
        .is_err()
    );
}

#[test]
fn working_set_is_not_evicted_while_another_member_is_produced() {
    let root = Fixture::new();
    let mut store = Store::open(&root.0, true, true).unwrap();
    let policy = Policy {
        max_bytes: 100,
        max_entries: 3,
    };
    let a = digest(b"needed-a");
    let b = digest(b"unrelated-b");
    let c = digest(b"needed-c");
    put(&store, &a, &[1; 40], &policy);
    put(&store, &b, &[2; 40], &policy);
    store.protect([a.clone(), c.clone()]).unwrap();
    let output = store.begin(&c, "mkv", &policy).unwrap();
    store.reserve(&c, 50, &policy).unwrap();
    assert!(store.read(&a).unwrap().is_some());
    assert!(store.read(&b).unwrap().is_none());
    assert_eq!(
        store.reserve(&c, 70, &policy).unwrap_err().code,
        "LIMIT_EXCEEDED"
    );
    assert!(store.read(&a).unwrap().is_some());
    assert!(!output.exists());
    store.discard(&c).unwrap();
    assert_eq!(store.inspect().unwrap()["payload_bytes"], 40);
}
