use std::{
    ffi::OsStr,
    fs,
    path::{Path, PathBuf},
    process::Command,
    time::{SystemTime, UNIX_EPOCH},
};

use pathhydra_store::{Catalog, ConfirmedRecord};

#[test]
fn inspection_commands_are_read_only_and_aggregate_only() {
    let temporary = TestDirectory::new("inspection");
    let database = temporary.path().join("database");
    let catalog = Catalog::open(&database).unwrap();
    let candidate = catalog
        .insert_node_candidate_with_payload("SecretExactName", vec![0, 1, 2, 255])
        .unwrap();
    let ConfirmedRecord::Node(_) = catalog.confirm_validated_candidate(candidate).unwrap() else {
        panic!("expected confirmed node")
    };
    catalog.insert_node_candidate("PendingSecretName").unwrap();
    drop(catalog);
    let before = directory_state(&database);

    for arguments in [
        vec!["summary", "--database", database.to_str().unwrap()],
        vec!["candidate-counts", "--database", database.to_str().unwrap()],
        vec!["verify", "--database", database.to_str().unwrap()],
        vec!["active-pointer", "--database", database.to_str().unwrap()],
        vec!["metrics-snapshot", "--database", database.to_str().unwrap()],
    ] {
        let is_summary = arguments[0] == "summary";
        let output = admin(arguments);
        assert!(output.status.success(), "{}", stderr(&output));
        let stdout = String::from_utf8(output.stdout).unwrap();
        let escaped_database = json_escaped_path(&database);
        assert!(stdout.starts_with('{'));
        assert!(!stdout.contains("SecretExactName"));
        assert!(!stdout.contains("PendingSecretName"));
        assert_eq!(
            stdout.contains(&escaped_database),
            is_summary,
            "only the local operator summary exposes its resolved database path"
        );
    }
    assert_eq!(directory_state(&database), before);
}

#[test]
fn explicit_checkpoint_and_restore_validate_complete_state() {
    let temporary = TestDirectory::new("checkpoint-restore");
    let database = temporary.path().join("database");
    let catalog = Catalog::open(&database).unwrap();
    let confirmed = catalog.insert_node_candidate("confirmed").unwrap();
    catalog.confirm_validated_candidate(confirmed).unwrap();
    catalog.insert_node_candidate("provisional").unwrap();
    drop(catalog);

    let checkpoint = temporary.path().join("checkpoint");
    let output = admin_os([
        OsStr::new("checkpoint-create"),
        OsStr::new("--database"),
        database.as_os_str(),
        OsStr::new("--destination-root"),
        temporary.path().as_os_str(),
        OsStr::new("--destination"),
        checkpoint.as_os_str(),
        OsStr::new("--available-bytes"),
        OsStr::new("18446744073709551615"),
    ]);
    assert!(output.status.success(), "{}", stderr(&output));
    let source_before = directory_state(&checkpoint);

    let restore = temporary.path().join("restore");
    let routing = temporary.path().join("restore-routing");
    let output = admin_os([
        OsStr::new("restore-validate"),
        OsStr::new("--source-root"),
        temporary.path().as_os_str(),
        OsStr::new("--source"),
        checkpoint.as_os_str(),
        OsStr::new("--destination-root"),
        temporary.path().as_os_str(),
        OsStr::new("--destination"),
        restore.as_os_str(),
        OsStr::new("--routing-root"),
        routing.as_os_str(),
        OsStr::new("--available-bytes"),
        OsStr::new("18446744073709551615"),
    ]);
    assert!(output.status.success(), "{}", stderr(&output));
    let stdout = String::from_utf8(output.stdout).unwrap();
    assert!(stdout.contains("\"confirmed_nodes\":1"));
    assert!(stdout.contains("\"total\":1"));
    assert!(stdout.contains("\"catalog_smoke\":true"));
    assert!(stdout.contains("\"route_smoke\":true"));
    assert!(stdout.contains("\"hydration_smoke\":true"));
    assert!(stdout.contains("\"shutdown_complete\":true"));
    assert_eq!(directory_state(&checkpoint), source_before);

    let restored = Catalog::open_existing(&restore).unwrap();
    assert!(restored.lookup_node_exact("confirmed").unwrap().is_some());
    assert_eq!(restored.summary().unwrap().candidates.total(), 1);
}

#[test]
fn engine_health_is_aggregate_only_and_releases_the_catalog_handle() {
    let temporary = TestDirectory::new("engine-health");
    let database = temporary.path().join("database");
    drop(Catalog::open(&database).unwrap());
    let routing = temporary.path().join("routing");

    let output = admin_os([
        OsStr::new("engine-health"),
        OsStr::new("--database"),
        database.as_os_str(),
        OsStr::new("--routing-root"),
        routing.as_os_str(),
    ]);
    assert!(output.status.success(), "{}", stderr(&output));
    let stdout = String::from_utf8(output.stdout).unwrap();
    assert!(stdout.contains("\"command\":\"engine-health\""));
    assert!(stdout.contains("\"durable_catalog_available\":true"));
    assert!(stdout.contains("\"drained_routes\":0"));
    assert!(stdout.contains("\"active_checkpoints_before_shutdown\":0"));
    assert!(stdout.contains("\"shutdown_complete\":true"));
    assert!(!stdout.contains(&json_escaped_path(&database)));
    assert!(!stdout.contains(&json_escaped_path(&routing)));

    drop(Catalog::open_existing(&database).unwrap());
}

#[test]
fn dry_run_reconciliation_never_removes_recognized_or_unknown_entries() {
    let temporary = TestDirectory::new("reconcile");
    let database = temporary.path().join("database");
    let catalog = Catalog::open(&database).unwrap();
    drop(catalog);
    let routing = temporary.path().join("routing");
    fs::create_dir(&routing).unwrap();
    fs::create_dir(routing.join(".tmp-stale")).unwrap();
    fs::create_dir(routing.join("bundle-invalid")).unwrap();
    fs::create_dir(routing.join("operator-owned")).unwrap();
    fs::write(routing.join("operator-file"), b"keep").unwrap();
    let before = directory_state(&routing);
    let output = admin_os([
        OsStr::new("reconcile-routing-root-dry-run"),
        OsStr::new("--database"),
        database.as_os_str(),
        OsStr::new("--routing-root"),
        routing.as_os_str(),
    ]);
    assert!(output.status.success(), "{}", stderr(&output));
    let stdout = String::from_utf8(output.stdout).unwrap();
    assert!(stdout.contains("\"mutated\":false"));
    assert!(stdout.contains("\"eligible_for_cleanup\":2"));
    assert_eq!(directory_state(&routing), before);
}

#[test]
fn parser_rejects_missing_duplicate_and_unknown_options() {
    for arguments in [
        vec!["summary"],
        vec!["summary", "--database", "one", "--database", "two"],
        vec!["summary", "--unknown", "value"],
        vec!["not-a-command"],
    ] {
        let output = admin(arguments);
        assert_eq!(output.status.code(), Some(2));
    }
    assert!(admin(["help"]).status.success());
}

fn admin<I, S>(arguments: I) -> std::process::Output
where
    I: IntoIterator<Item = S>,
    S: AsRef<OsStr>,
{
    Command::new(env!("CARGO_BIN_EXE_pathhydra-admin"))
        .args(arguments)
        .output()
        .unwrap()
}

fn admin_os<I, S>(arguments: I) -> std::process::Output
where
    I: IntoIterator<Item = S>,
    S: AsRef<OsStr>,
{
    admin(arguments)
}

fn stderr(output: &std::process::Output) -> String {
    String::from_utf8_lossy(&output.stderr).into_owned()
}

fn json_escaped_path(path: &Path) -> String {
    path.to_string_lossy().replace('\\', "\\\\")
}

fn directory_state(path: &Path) -> Vec<(PathBuf, u64)> {
    let mut output = Vec::new();
    let mut pending = vec![path.to_path_buf()];
    while let Some(directory) = pending.pop() {
        for entry in fs::read_dir(&directory).unwrap() {
            let entry = entry.unwrap();
            let metadata = entry.metadata().unwrap();
            let relative = entry.path().strip_prefix(path).unwrap().to_path_buf();
            output.push((relative, metadata.len()));
            if metadata.is_dir() {
                pending.push(entry.path());
            }
        }
    }
    output.sort();
    output
}

struct TestDirectory(PathBuf);

impl TestDirectory {
    fn new(name: &str) -> Self {
        let nonce = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap()
            .as_nanos();
        let path = std::env::temp_dir().join(format!(
            "pathhydra-admin-{name}-{}-{nonce}",
            std::process::id()
        ));
        fs::create_dir(&path).unwrap();
        Self(path)
    }

    fn path(&self) -> &Path {
        &self.0
    }
}

impl Drop for TestDirectory {
    fn drop(&mut self) {
        let _ = fs::remove_dir_all(&self.0);
    }
}
