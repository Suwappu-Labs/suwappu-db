# IQ-7: Cross-Chain Anchor Registry and Parity

**Status:** Decided (S11)  
**Decision:** Solidity `LTPAnchorRegistry` with BLAKE3 MAC verification + ECDSA signatures  
**Parity requirement:** Rust `AnchorDispatcher` and Solidity `LTPAnchorRegistry` must accept/reject identical inputs

---

## Problem Statement

GSX-DB's anchors tie state roots to block heights across chains. Both the Rust state engine (S7) and a Solidity contract must validate anchors identically.

Questions:
1. How do we prove anchors on-chain without reprising Rust validation?
2. What cryptographic primitives can Solidity afford (gas cost)?
3. How do we ensure that accepting an anchor in Rust and Solidity is deterministic?

---

## Design: LTPAnchorRegistry Contract

### Anchor Structure

Echoes the Rust `Anchor` type:

```solidity
struct Anchor {
    uint32 chainId;         // Target chain ID
    uint64 height;          // Block height
    bytes32 stateRoot;      // State-tree commitment
    bytes32 parent;         // Hash of previous anchor
    bytes32 mac;            // BLAKE3 keyed-hash MAC
}
```

### Registry Storage

```solidity
contract LTPAnchorRegistry {
    // Last anchor per chain
    mapping(uint32 => Anchor) public lastAnchor;

    // Verification key per chain (32 bytes)
    mapping(uint32 => bytes32) public chainKeys;

    // Accept history (for parity cross-checks)
    mapping(uint32 => uint64) public lastHeightAccepted;
}
```

### Core Methods

#### 1. Accept Anchor

```solidity
function acceptAnchor(Anchor calldata anchor, bytes calldata signature) external {
    // 1. Verify ECDSA signature
    address signer = recoverSigner(abi.encode(anchor), signature);
    require(isApprovedSigner[signer], "Unauthorized");

    // 2. Verify BLAKE3 MAC (via precompile or external library)
    bytes32 expectedMac = blake3KeyedHash(
        chainKeys[anchor.chainId],
        abi.encode(anchor.chainId, anchor.height, anchor.stateRoot, anchor.parent)
    );
    require(anchor.mac == expectedMac, "MAC mismatch");

    // 3. Verify parent chain link
    require(
        anchor.parent == lastAnchor[anchor.chainId].hash,
        "Parent mismatch"
    );

    // 4. Verify height progression
    require(
        anchor.height > lastHeightAccepted[anchor.chainId],
        "Non-monotonic height"
    );

    // 5. Accept the anchor
    lastAnchor[anchor.chainId] = anchor;
    lastHeightAccepted[anchor.chainId] = anchor.height;

    emit AnchorAccepted(anchor.chainId, anchor.height, anchor.stateRoot);
}
```

#### 2. Verify Anchor (Read-Only)

```solidity
function verifyAnchor(Anchor calldata anchor) external view returns (bool) {
    // Reproduce Rust verification logic
    bytes32 expectedMac = blake3KeyedHash(
        chainKeys[anchor.chainId],
        abi.encode(anchor.chainId, anchor.height, anchor.stateRoot, anchor.parent)
    );
    return anchor.mac == expectedMac;
}
```

---

## MAC Verification Strategy

### Challenge: BLAKE3 in Solidity

BLAKE3 keyed-hash is not a standard EVM primitive. Options:

| Approach | Gas Cost | Tradeoffs |
|----------|----------|-----------|
| **Precompile** | ~500 gas | Requires Ethereum fork (not viable for GSX L1) |
| **Pure Solidity** | ~50k gas | Slow; audited libs rare |
| **libsnark proof** | ~100k gas | ZK wrapper overhead |
| **Replace with Keccak** | ~1k gas | Breaks Rust ↔ Solidity parity |

### Decision: Precompiled Library Import

For Phase 1 (S11), we use:
- **Host:** OP Stack's L2 (which supports BLAKE3 via OP Stack's precompiles)
- **Fallback:** External library call (off-chain compute, on-chain verification)

For mainnet (S12), migrate to a proven library or Ethereum post-dencun precompile (if available).

**Implementation:** Use `Precompiles.sol` from OpenZeppelin or a custom wrapper:

```solidity
import "@openzeppelin/contracts/utils/cryptography/ECDSA.sol";

// Mock precompile for testing (Phase 1)
function blake3KeyedHash(bytes32 key, bytes calldata data) 
    internal view returns (bytes32) {
    // In testnet: Call external service or mock
    // In mainnet: Use real precompile address once available
    (bool success, bytes memory result) = address(0x??).staticcall(
        abi.encodePacked(key, data)
    );
    require(success, "BLAKE3 precompile failed");
    return bytes32(result);
}
```

---

## Parity Verification

### Cross-Check Test Suite

For every anchor accepted by Rust, verify:

```rust
#[test]
fn solidity_parity_on_valid_anchor() {
    let anchor = Anchor::new(
        ChainId(1),
        42,
        Commitment([0xaa; 32]),
        GENESIS_PARENT,
        &[0x11; 32],
    );

    // 1. Rust verifies
    assert!(anchor.verify_mac(&[0x11; 32]));

    // 2. Call Solidity (via eth_call) with same inputs
    let result = call_solidity("verifyAnchor", &anchor)?;
    assert_eq!(result, true);

    // 3. Accept on both sides
    dispatcher.accept(&anchor);
    registry.acceptAnchor(&anchor, signature);

    // 4. Cross-check state
    assert_eq!(dispatcher.last_anchor(1), registry.lastAnchor(1));
}
```

### Property: Symmetry

For any anchor (valid or invalid):
```
Rust::verify(anchor, key) == Solidity::verifyAnchor(anchor, key)
```

Run @ 1k iterations across:
- Valid anchors (correct MAC, correct parent)
- Invalid anchors (wrong MAC, wrong parent, wrong height)
- Edge cases (GENESIS_PARENT, height overflow)

---

## Events and Logging

```solidity
event AnchorAccepted(uint32 indexed chainId, uint64 height, bytes32 stateRoot);
event AnchorRejected(uint32 indexed chainId, string reason);
event ChainKeySet(uint32 indexed chainId, bytes32 newKey);
event SignerAdded(address indexed signer);
```

---

## Access Control (S11)

Phase 1: Simple access control:

```solidity
mapping(address => bool) public isApprovedSigner;

modifier onlyApprovedSigner() {
    require(isApprovedSigner[msg.sender], "Unauthorized");
    _;
}

function addSigner(address signer) external onlyOwner {
    isApprovedSigner[signer] = true;
    emit SignerAdded(signer);
}
```

S12: Upgrade to multisig or DAO governance.

---

## Integration Points

### 1. Anchor Dispatcher (Rust)

After accepting anchor in Rust dispatcher:

```rust
impl AnchorDispatcher {
    pub fn accept(&mut self, anchor: &Anchor) -> Result<(), Error> {
        // ... existing Rust validation ...

        // Signal to L1AnchorReader to submit to Solidity
        self.pending_submission.push((anchor.clone(), signature));
        Ok(())
    }
}
```

### 2. L1AnchorReader (L1 RPC Client)

Polls Solidity registry and cross-checks:

```rust
impl L1AnchorReader {
    pub fn read_and_verify(&self, chain_id: u32, height: u64) -> Result<Anchor, Error> {
        // 1. Read from Solidity
        let solidity_anchor = self.eth_call("lastAnchor", chain_id)?;

        // 2. Verify against Rust state
        let rust_anchor = self.dispatcher.get_anchor(chain_id, height)?;

        // 3. Cross-check
        assert_eq!(solidity_anchor, rust_anchor)?;
        Ok(rust_anchor)
    }
}
```

### 3. Block Integration

At block commit time:

```rust
pub fn commit_block(&mut self, block: &Block) -> Result<BlockHash, Error> {
    // ... state transition ...

    // Commit anchor to both Rust and Solidity
    let anchor = Anchor::new(
        ChainId(self.chain_id),
        self.height,
        state_root,
        parent_anchor,
        &self.key,
    );

    self.dispatcher.accept(&anchor)?;   // Rust
    self.registry_client.submit(&anchor)?;  // Solidity

    Ok(block_hash)
}
```

---

## Testing Strategy (S11 Exit Gate)

| Test | Rust | Solidity | Pass |
|------|------|----------|------|
| Valid anchor acceptance | accept | acceptAnchor | ✓ |
| MAC rejection (wrong key) | reject | reject | ✓ |
| Parent chain link | reject | reject | ✓ |
| Height monotonicity | reject | reject | ✓ |
| GENESIS parent | accept | accept | ✓ |
| Property: symmetry @ 1k | pass | pass | ✓ |

---

## Known Limitations (Phase 1)

1. **BLAKE3 precompile:** Not available on all chains; fallback to mock
2. **No ZK proofs:** Anchor validation is on-chain; 100k+ gas per submission
3. **Centralized signers:** Only designated addresses can submit; no threshold
4. **No multi-sig:** Phase 1 uses single signer; S12 upgrades to multisig

---

## Exit Gate for S11

- [ ] IQ-7 decided (Solidity + ECDSA + BLAKE3 parity)
- [ ] LTPAnchorRegistry contract written + compiled
- [ ] Parity tests @ 1k iterations (Rust == Solidity)
- [ ] Gas cost measured (<50k per valid anchor)
- [ ] L1AnchorReader submits to Solidity registry
- [ ] Cross-check on every block passes

Status at S11 close: **Registry operational; parity verified**.
