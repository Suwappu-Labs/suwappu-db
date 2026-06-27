/// suwappu-db canonical Coin module — S9.5g.
///
/// Holds the per-account balance + sequence number resource that the
/// suwappu-db dual-projection invariant tracks. Mirrors the `CoinStore`
/// BCS layout the Rust-side `BalanceViewResolver` and
/// `decode_coin_store` agree on:
///
///   struct CoinStore { value: u64, sequence: u64 }    // 16 bytes LE
///
/// Deliberately simpler than `aptos_framework::coin::CoinStore`
/// (which adds `frozen: bool` + two `EventHandle`s). Events surface
/// through `MoveOutcome::events` on the Rust side; we don't need
/// on-chain Move event handles.
///
/// The `transfer` entry function is the canonical balance-mutating
/// surface; `MockMoveExecutor` simulates the same semantics so the
/// cross-VM parity invariant holds whether real or mock backend is
/// in use.
module gsxdb_coin::coin {
    /// Insufficient balance abort code — matches
    /// `aptos_framework::coin::EINSUFFICIENT_BALANCE` so wire-level
    /// comparison stays consistent.
    const EINSUFFICIENT_BALANCE: u64 = 0x10006;

    /// The canonical balance + sequence resource.
    struct CoinStore has key {
        value: u64,
        sequence: u64,
    }

    /// Transfer `amount` from `from` to `to`, incrementing `from`'s
    /// sequence by 1. Aborts with `EINSUFFICIENT_BALANCE` if
    /// `from`'s balance < amount.
    ///
    /// suwappu-db semantics: the transaction-author identity is checked
    /// at the bridge/lane boundary (the Move `&signer` capability is
    /// never forged into a Move VM session). Inside the VM we accept
    /// the source address as an unsigned `address` argument — the
    /// caller's authorization is already validated.
    public entry fun transfer(from: address, to: address, amount: u64)
        acquires CoinStore
    {
        let from_store = borrow_global_mut<CoinStore>(from);
        assert!(from_store.value >= amount, EINSUFFICIENT_BALANCE);
        from_store.value = from_store.value - amount;
        from_store.sequence = from_store.sequence + 1;

        let to_store = borrow_global_mut<CoinStore>(to);
        to_store.value = to_store.value + amount;
    }

    #[view]
    public fun balance(addr: address): u64 acquires CoinStore {
        if (exists<CoinStore>(addr)) {
            borrow_global<CoinStore>(addr).value
        } else {
            0
        }
    }

    #[view]
    public fun sequence(addr: address): u64 acquires CoinStore {
        if (exists<CoinStore>(addr)) {
            borrow_global<CoinStore>(addr).sequence
        } else {
            0
        }
    }
}
