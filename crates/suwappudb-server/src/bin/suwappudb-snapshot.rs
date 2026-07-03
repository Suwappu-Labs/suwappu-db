//! `suwappudb-snapshot` — operator CLI for state snapshots (gap item
//! G5 in `docs/research/chain-gap-analysis-2026-07.md`).
//!
//! Packages the existing `StateSnapshot` machinery (S12.2) into the
//! bootstrap flow described in `docs/architecture/node-bootstrap.md`:
//!
//! ```text
//! suwappudb-snapshot export  --db <state.redb> --height <n> --out <snap.json> [--anchor <64-hex>]
//! suwappudb-snapshot import  --snapshot <snap.json> --db <state.redb>
//! suwappudb-snapshot inspect <snap.json>
//! suwappudb-snapshot verify  <snap.json> [--anchor <64-hex>]
//! ```
//!
//! `export` embeds the recomputed state-tree root; `verify` restores
//! into a scratch in-memory state and recomputes the root so a
//! downloaded snapshot can be checked before it touches a node's
//! store. Exit code is non-zero on any failure or root mismatch.
//!
//! Commitment-scheme note: roots are computed with the build's default
//! scheme (BLAKE3 unless `production-verkle` is enabled). Verify with
//! a binary built with the same features as the exporter.

use suwappudb_state::snapshot::StateSnapshot;
use suwappudb_state::{BridgeToken, Commitment, RedbBalanceStore, State, StateTree};

const USAGE: &str = "suwappudb-snapshot — export / import / inspect / verify state snapshots

USAGE:
    suwappudb-snapshot export  --db <state.redb> --height <n> --out <snap.json> [--anchor <64-hex>]
    suwappudb-snapshot import  --snapshot <snap.json> --db <state.redb>
    suwappudb-snapshot inspect <snap.json>
    suwappudb-snapshot verify  <snap.json> [--anchor <64-hex>]

Snapshots are the JSON envelope written by StateSnapshot::write_to_file
(hex-encoded V2 body: balances + bytes column, sorted by address).
See docs/architecture/node-bootstrap.md for the full operator flow.";

fn main() {
    let args: Vec<String> = std::env::args().skip(1).collect();
    let result = match args.first().map(String::as_str) {
        Some("export") => cmd_export(&args[1..]),
        Some("import") => cmd_import(&args[1..]),
        Some("inspect") => cmd_inspect(&args[1..]),
        Some("verify") => cmd_verify(&args[1..]),
        Some("--help" | "-h" | "help") | None => {
            println!("{USAGE}");
            Ok(())
        }
        Some(other) => Err(format!("unknown subcommand `{other}`\n\n{USAGE}")),
    };
    if let Err(msg) = result {
        eprintln!("error: {msg}");
        std::process::exit(1);
    }
}

/// Pull the value following `--flag` out of `args`.
fn flag_value(args: &[String], flag: &str) -> Option<String> {
    args.iter()
        .position(|a| a == flag)
        .and_then(|i| args.get(i + 1))
        .cloned()
}

fn required(args: &[String], flag: &str, cmd: &str) -> Result<String, String> {
    flag_value(args, flag).ok_or_else(|| format!("`{cmd}` requires {flag} <value>\n\n{USAGE}"))
}

fn parse_anchor(args: &[String]) -> Result<Option<[u8; 32]>, String> {
    let Some(hex_str) = flag_value(args, "--anchor") else {
        return Ok(None);
    };
    let raw = hex::decode(hex_str.trim_start_matches("0x"))
        .map_err(|e| format!("--anchor is not valid hex: {e}"))?;
    let bytes: [u8; 32] = raw
        .try_into()
        .map_err(|_| "--anchor must be exactly 32 bytes (64 hex chars)".to_string())?;
    Ok(Some(bytes))
}

fn open_state(db_path: &str) -> Result<State, String> {
    let store =
        RedbBalanceStore::open(db_path).map_err(|e| format!("open redb store `{db_path}`: {e}"))?;
    Ok(State::with_store(Box::new(store)))
}

fn print_metadata(snap: &StateSnapshot) {
    println!(
        "{}",
        serde_json::to_string_pretty(&snap.to_metadata_json()).expect("metadata is valid JSON")
    );
    println!(
        "balance entries: {}  bytes entries: {}",
        snap.entry_count()
            .map_or_else(|| "?".into(), |n| n.to_string()),
        snap.bytes_entry_count()
            .map_or_else(|| "?".into(), |n| n.to_string()),
    );
}

fn cmd_export(args: &[String]) -> Result<(), String> {
    let db = required(args, "--db", "export")?;
    let out = required(args, "--out", "export")?;
    let height: u64 = required(args, "--height", "export")?
        .parse()
        .map_err(|e| format!("--height must be a u64: {e}"))?;
    let anchor = parse_anchor(args)?;

    let state = open_state(&db)?;
    let root = StateTree::from_state(&state).root();
    let snap = StateSnapshot::from_state(&state, height, anchor).with_state_root(root);
    snap.write_to_file(&out)?;

    println!("exported `{db}` -> `{out}`");
    print_metadata(&snap);
    Ok(())
}

fn cmd_import(args: &[String]) -> Result<(), String> {
    let snapshot_path = required(args, "--snapshot", "import")?;
    let db = required(args, "--db", "import")?;

    let snap = StateSnapshot::read_from_file(&snapshot_path)?;
    let mut state = open_state(&db)?;
    if !state.entries().is_empty() {
        eprintln!(
            "warning: `{db}` is not empty — snapshot addresses overwrite, \
             other entries are left in place (see restore_into_state docs)"
        );
    }

    let token = BridgeToken::__for_bridge_only();
    let applied = snap.restore_into_state(&mut state, &token)?;
    println!("applied {applied} state changes from `{snapshot_path}` into `{db}`");

    check_root(&snap, &state)
}

fn cmd_inspect(args: &[String]) -> Result<(), String> {
    let path = args
        .first()
        .ok_or_else(|| format!("`inspect` requires a snapshot path\n\n{USAGE}"))?;
    // read_from_file validates the encoded body structurally.
    let snap = StateSnapshot::read_from_file(path)?;
    print_metadata(&snap);
    Ok(())
}

fn cmd_verify(args: &[String]) -> Result<(), String> {
    let path = args
        .first()
        .ok_or_else(|| format!("`verify` requires a snapshot path\n\n{USAGE}"))?;
    let snap = StateSnapshot::read_from_file(path)?;

    if let Some(expected) = parse_anchor(args)? {
        if !snap.verify_anchor(&expected) {
            return Err(format!(
                "anchor mismatch: snapshot carries {:?}",
                snap.anchor_hash.map(hex::encode)
            ));
        }
        println!("anchor hash: match");
    }

    // Restore into a scratch in-memory state and recompute the root —
    // proves the body round-trips and the embedded root is honest.
    let mut scratch = State::default();
    let token = BridgeToken::__for_bridge_only();
    let applied = snap.restore_into_state(&mut scratch, &token)?;
    println!("structure: ok ({applied} state changes)");
    check_root(&snap, &scratch)?;
    println!("verify: PASS");
    Ok(())
}

/// Recompute the state-tree root over `state` and compare with the
/// snapshot's embedded root. A zero root means the exporter didn't
/// embed one (pre-CLI snapshots) — reported but not fatal.
fn check_root(snap: &StateSnapshot, state: &State) -> Result<(), String> {
    if snap.state_root == Commitment([0; 32]) {
        println!("state root: snapshot has no embedded root (skipped)");
        return Ok(());
    }
    let recomputed = StateTree::from_state(state).root();
    if recomputed == snap.state_root {
        println!("state root: match (0x{})", hex::encode(recomputed.0));
        Ok(())
    } else {
        Err(format!(
            "state-root mismatch: snapshot 0x{}, recomputed 0x{}",
            hex::encode(snap.state_root.0),
            hex::encode(recomputed.0)
        ))
    }
}
