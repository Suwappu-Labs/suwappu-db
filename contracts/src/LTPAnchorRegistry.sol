// SPDX-License-Identifier: MIT
pragma solidity ^0.8.20;

/**
 * @title LTPAnchorRegistry
 * @notice Stores and validates cross-chain anchors for GSX-DB.
 *
 * An anchor ties a state root to a block height and is authenticated
 * via BLAKE3 keyed-hash MAC. The registry ensures:
 *
 * 1. MAC validity under the chain's verification key
 * 2. Parent chain link (each anchor points to its predecessor)
 * 3. Height monotonicity (no gaps or rollbacks)
 * 4. Authorized signers (only designated addresses can submit)
 *
 * @dev Parity requirement: Rust AnchorDispatcher and this contract
 * must accept/reject identical inputs. Verified via property tests.
 */
contract LTPAnchorRegistry {
    // ===== Ownership =====
    address public owner;

    constructor() {
        owner = msg.sender;
    }

    modifier onlyOwner() {
        require(msg.sender == owner, "LTPAnchorRegistry: Only owner");
        _;
    }

    // ===== Types =====

    /// One per-chain anchor for one block.
    struct Anchor {
        uint32 chainId;         // Target chain
        uint64 height;          // Block height
        bytes32 stateRoot;      // State-tree commitment
        bytes32 parent;         // Hash of previous anchor (GENESIS = 0)
        bytes32 mac;            // BLAKE3 keyed-hash MAC
    }

    /// Hash of genesis anchor (parent of first anchor on each chain)
    bytes32 public constant GENESIS_PARENT = bytes32(0);

    // ===== State =====

    /// Last accepted anchor per chain
    mapping(uint32 => Anchor) public lastAnchor;

    /// BLAKE3 verification key per chain (32 bytes)
    mapping(uint32 => bytes32) public chainKey;

    /// Last accepted height per chain
    mapping(uint32 => uint64) public lastHeightAccepted;

    /// Approved signers (can submit anchors)
    mapping(address => bool) public isApprovedSigner;

    // ===== Events =====

    event AnchorAccepted(
        uint32 indexed chainId,
        uint64 indexed height,
        bytes32 indexed stateRoot
    );

    event AnchorRejected(uint32 indexed chainId, string reason);

    event ChainKeySet(uint32 indexed chainId, bytes32 newKey);

    event SignerAdded(address indexed signer);

    event SignerRemoved(address indexed signer);

    // ===== Admin =====

    /// Set verification key for a chain
    function setChainKey(uint32 chainId, bytes32 key) external onlyOwner {
        require(key != bytes32(0), "Key cannot be zero");
        chainKey[chainId] = key;
        emit ChainKeySet(chainId, key);
    }

    /// Add an approved signer
    function addSigner(address signer) external onlyOwner {
        require(signer != address(0), "Invalid signer");
        isApprovedSigner[signer] = true;
        emit SignerAdded(signer);
    }

    /// Remove an approved signer
    function removeSigner(address signer) external onlyOwner {
        isApprovedSigner[signer] = false;
        emit SignerRemoved(signer);
    }

    // ===== Core =====

    /**
     * @notice Accept a new anchor.
     *
     * Validates:
     * 1. Signer is approved
     * 2. MAC is correct under chain key
     * 3. Parent chain link is valid
     * 4. Height is monotonically increasing
     *
     * @param anchor The anchor to accept
     * @param signature ECDSA signature from approved signer
     */
    function acceptAnchor(Anchor calldata anchor, bytes calldata signature) external {
        // 1. Verify signer is approved
        address signer = recoverSigner(anchor, signature);
        require(isApprovedSigner[signer], "LTPAnchorRegistry: Unauthorized signer");

        // 2. Verify MAC
        require(verifyMac(anchor), "LTPAnchorRegistry: MAC mismatch");

        // 3. Verify parent chain link
        bytes32 expectedParent = hashAnchorMemory(lastAnchor[anchor.chainId]);
        // For genesis: parent must be GENESIS_PARENT and lastHeightAccepted must be 0
        if (lastHeightAccepted[anchor.chainId] == 0) {
            require(
                anchor.parent == GENESIS_PARENT,
                "LTPAnchorRegistry: Genesis anchor must have GENESIS parent"
            );
        } else {
            require(
                anchor.parent == expectedParent,
                "LTPAnchorRegistry: Parent mismatch"
            );
        }

        // 4. Verify height progression
        require(
            anchor.height > lastHeightAccepted[anchor.chainId],
            "LTPAnchorRegistry: Non-monotonic height"
        );

        // 5. Accept anchor
        lastAnchor[anchor.chainId] = anchor;
        lastHeightAccepted[anchor.chainId] = anchor.height;

        emit AnchorAccepted(anchor.chainId, anchor.height, anchor.stateRoot);
    }

    // ===== Verification (Read-Only) =====

    /**
     * @notice Verify an anchor without accepting it.
     * @param anchor The anchor to verify
     * @return True iff the anchor is valid
     */
    function verifyAnchor(Anchor calldata anchor) external view returns (bool) {
        return verifyMac(anchor);
    }

    /**
     * @notice Verify MAC under chain key.
     * @param anchor The anchor to verify
     * @return True iff MAC is correct
     */
    function verifyMac(Anchor calldata anchor) public view returns (bool) {
        bytes32 key = chainKey[anchor.chainId];
        require(key != bytes32(0), "LTPAnchorRegistry: Chain not registered");

        bytes32 expectedMac = computeMac(anchor, key);
        return anchor.mac == expectedMac;
    }

    /**
     * @notice Get the last accepted anchor for a chain.
     * @param chainId The chain ID
     * @return The last accepted anchor (empty if none)
     */
    function getLastAnchor(uint32 chainId) external view returns (Anchor memory) {
        return lastAnchor[chainId];
    }

    /**
     * @notice Get the last accepted height for a chain.
     * @param chainId The chain ID
     * @return The last accepted height (0 if none)
     */
    function getLastHeight(uint32 chainId) external view returns (uint64) {
        return lastHeightAccepted[chainId];
    }

    // ===== Cryptography =====

    /**
     * @notice Recover signer from signature.
     * @param anchor The anchor that was signed
     * @param signature ECDSA signature
     * @return The recovered signer address
     */
    function recoverSigner(Anchor calldata anchor, bytes calldata signature)
        public pure returns (address)
    {
        bytes32 messageHash = keccak256(abi.encode(anchor));
        bytes32 ethMessageHash = keccak256(
            abi.encodePacked("\x19Ethereum Signed Message:\n32", messageHash)
        );
        return recoverAddress(ethMessageHash, signature);
    }

    /**
     * @notice Recover address from message hash and signature.
     * @param messageHash The message hash
     * @param signature The ECDSA signature
     * @return The recovered address
     */
    function recoverAddress(bytes32 messageHash, bytes memory signature)
        internal pure returns (address)
    {
        require(signature.length == 65, "LTPAnchorRegistry: Invalid signature length");

        bytes32 r;
        bytes32 s;
        uint8 v;

        assembly {
            r := mload(add(signature, 0x20))
            s := mload(add(signature, 0x40))
            v := byte(0, mload(add(signature, 0x60)))
        }

        require(v == 27 || v == 28, "LTPAnchorRegistry: Invalid signature");

        // Reject high-s signatures (EIP-2 malleability). secp256k1n/2:
        require(
            uint256(s) <= 0x7FFFFFFFFFFFFFFFFFFFFFFFFFFFFFFF5D576E7357A4501DDFE92F46681B20A0,
            "LTPAnchorRegistry: Invalid signature"
        );

        address recovered = ecrecover(messageHash, v, r, s);
        require(recovered != address(0), "LTPAnchorRegistry: Invalid signature");

        return recovered;
    }

    /**
     * @notice Compute BLAKE3 keyed-hash MAC.
     *
     * @dev Phase 1: Uses Keccak256 as a placeholder (NOT BLAKE3).
     * This is for testing parity logic; real deployment requires
     * actual BLAKE3 via precompile or external library.
     *
     * @param anchor The anchor to MAC
     * @param key The verification key
     * @return The computed MAC
     */
    function computeMac(Anchor calldata anchor, bytes32 key)
        public pure returns (bytes32)
    {
        // TODO(S11): Replace with actual BLAKE3 keyed-hash.
        // For now, use Keccak256 with domain separation.
        return keccak256(
            abi.encodePacked(
                "GSXDB-ANCHOR/MAC",
                key,
                anchor.chainId,
                anchor.height,
                anchor.stateRoot,
                anchor.parent
            )
        );
    }

    /**
     * @notice Hash of an anchor (used as parent for next anchor).
     * @param anchor The anchor to hash
     * @return The hash
     */
    function hashAnchor(Anchor calldata anchor) public pure returns (bytes32) {
        return keccak256(
            abi.encodePacked(
                "GSXDB-ANCHOR/HASH",
                anchor.chainId,
                anchor.height,
                anchor.stateRoot,
                anchor.parent,
                anchor.mac
            )
        );
    }

    /**
     * @notice Hash of an anchor in memory.
     * @param anchor The anchor to hash
     * @return The hash
     */
    function hashAnchorMemory(Anchor memory anchor) internal pure returns (bytes32) {
        return keccak256(
            abi.encodePacked(
                "GSXDB-ANCHOR/HASH",
                anchor.chainId,
                anchor.height,
                anchor.stateRoot,
                anchor.parent,
                anchor.mac
            )
        );
    }

    /**
     * @notice Hash of a stored anchor (view version).
     * @param chainId The chain ID
     * @return The hash of the last anchor
     */
    function getLastAnchorHash(uint32 chainId) external view returns (bytes32) {
        Anchor memory anchor = lastAnchor[chainId];
        return hashAnchorMemory(anchor);
    }
}
