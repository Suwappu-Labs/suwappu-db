//! End-to-end coverage for the `suwappudb-snapshot` operator CLI
//! (gap item G5): export from a redb store, inspect, verify, import
//! into a fresh store, and reject a tampered snapshot.
//!
//! Drives the real binary via `CARGO_BIN_EXE_suwappudb-snapshot`, the
//! same way an operator would.

use std::path::Path;
use std::process::{Command, Output};

use suwappudb_state::snapshot::StateSnapshot;
use suwappudb_state::{
    Address, Balance, BridgeToken, RedbBalanceStore, State, StateChange, StateTree,
};

const BIN: &str = env!("CARGO_BIN_EXE_suwappudb-snapshot");

fn addr(i: u8) -> Address {
    Address([i; 20])
}

/// Build a redb-backed state with a few balances and one bytes-column
/// entry, mirroring what a live node's store contains.
fn seed_db(path: &Path) {
    let store = RedbBalanceStore::open(path).expect("open redb");
    let mut state = State::with_store(Box::new(store));
    let token = BridgeToken::__for_bridge_only();
    for i in 1..=5u8 {
        state.apply(
            &token,
            &StateChange::SetBalance {
                addr: addr(i),
                to: Balance(u128::from(i) * 1_000),
            },
        );
    }
    state.apply(
        &token,
        &StateChange::SetBytes {
            addr: addr(9),
            bytes: b"registry-blob".to_vec(),
        },
    );
}

fn run(args: &[&str]) -> Output {
    Command::new(BIN).args(args).output().expect("spawn CLI")
}

fn assert_success(out: &Output, context: &str) {
    assert!(
        out.status.success(),
        "{context} failed\nstdout: {}\nstderr: {}",
        String::from_utf8_lossy(&out.stdout),
        String::from_utf8_lossy(&out.stderr),
    );
}

#[test]
fn export_verify_import_roundtrip() {
    let dir = tempfile::tempdir().expect("tempdir");
    let src_db = dir.path().join("src.redb");
    let dst_db = dir.path().join("dst.redb");
    let snap_path = dir.path().join("snap.json");
    seed_db(&src_db);

    // export — embeds the recomputed state-tree root.
    let out = run(&[
        "export",
        "--db",
        src_db.to_str().unwrap(),
        "--height",
        "42",
        "--out",
        snap_path.to_str().unwrap(),
    ]);
    assert_success(&out, "export");

    // inspect — parses and prints metadata.
    let out = run(&["inspect", snap_path.to_str().unwrap()]);
    assert_success(&out, "inspect");
    let stdout = String::from_utf8_lossy(&out.stdout);
    assert!(
        stdout.contains("\"height\": 42"),
        "inspect output: {stdout}"
    );
    assert!(
        stdout.contains("balance entries: 5") && stdout.contains("bytes entries: 1"),
        "inspect output: {stdout}"
    );

    // verify — structural check + root recomputation must pass.
    let out = run(&["verify", snap_path.to_str().unwrap()]);
    assert_success(&out, "verify");
    assert!(
        String::from_utf8_lossy(&out.stdout).contains("verify: PASS"),
        "verify output missing PASS"
    );

    // import into a fresh store, then confirm the stores agree.
    let out = run(&[
        "import",
        "--snapshot",
        snap_path.to_str().unwrap(),
        "--db",
        dst_db.to_str().unwrap(),
    ]);
    assert_success(&out, "import");

    let restored = State::with_store(Box::new(RedbBalanceStore::open(&dst_db).expect("open dst")));
    for i in 1..=5u8 {
        assert_eq!(
            restored.balance_of(&addr(i)),
            Balance(u128::from(i) * 1_000)
        );
    }
    assert_eq!(restored.bytes_of(&addr(9)), Some(b"registry-blob".to_vec()));

    // Roots agree between the snapshot and the imported store.
    let snap = StateSnapshot::read_from_file(&snap_path).expect("read snapshot");
    assert_eq!(StateTree::from_state(&restored).root(), snap.state_root);
}

#[test]
fn verify_rejects_tampered_balance() {
    let dir = tempfile::tempdir().expect("tempdir");
    let src_db = dir.path().join("src.redb");
    let snap_path = dir.path().join("snap.json");
    seed_db(&src_db);

    let out = run(&[
        "export",
        "--db",
        src_db.to_str().unwrap(),
        "--height",
        "42",
        "--out",
        snap_path.to_str().unwrap(),
    ]);
    assert_success(&out, "export");

    // Flip one balance inside the encoded body: addr(1) 1000 -> 2000.
    // The JSON envelope stays well-formed, so only the root check can
    // catch it — exactly the attack `verify` exists to stop.
    let mut snap = StateSnapshot::read_from_file(&snap_path).expect("read snapshot");
    let needle = 1_000u128.to_le_bytes();
    let pos = snap
        .encoded_state
        .windows(needle.len())
        .position(|w| w == needle)
        .expect("seeded balance present in body");
    snap.encoded_state[pos..pos + needle.len()].copy_from_slice(&2_000u128.to_le_bytes());
    snap.write_to_file(&snap_path).expect("rewrite tampered");

    let out = run(&["verify", snap_path.to_str().unwrap()]);
    assert!(!out.status.success(), "verify must fail on tampered body");
    assert!(
        String::from_utf8_lossy(&out.stderr).contains("state-root mismatch"),
        "stderr: {}",
        String::from_utf8_lossy(&out.stderr)
    );
}

#[test]
fn verify_checks_anchor_hash() {
    let dir = tempfile::tempdir().expect("tempdir");
    let src_db = dir.path().join("src.redb");
    let snap_path = dir.path().join("snap.json");
    seed_db(&src_db);

    let anchor_hex = "11".repeat(32);
    let out = run(&[
        "export",
        "--db",
        src_db.to_str().unwrap(),
        "--height",
        "7",
        "--out",
        snap_path.to_str().unwrap(),
        "--anchor",
        &anchor_hex,
    ]);
    assert_success(&out, "export with anchor");

    let out = run(&[
        "verify",
        snap_path.to_str().unwrap(),
        "--anchor",
        &anchor_hex,
    ]);
    assert_success(&out, "verify matching anchor");

    let wrong = "22".repeat(32);
    let out = run(&["verify", snap_path.to_str().unwrap(), "--anchor", &wrong]);
    assert!(!out.status.success(), "verify must fail on anchor mismatch");
}
