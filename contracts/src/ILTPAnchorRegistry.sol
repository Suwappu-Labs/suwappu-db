// SPDX-License-Identifier: MIT
pragma solidity ^0.8.20;

/**
 * @title ILTPAnchorRegistry
 * @notice External-integrator-facing interface for LTPAnchorRegistry.
 *
 * S11.4 ships this interface so downstream Solidity consumers can
 * import this file directly and link against the registry without
 * pulling in the full implementation source.
 *
 * Layout is byte-for-byte parity with `LTPAnchorRegistry.sol`; the
 * full contract's storage and constructor are not exposed.
 */
interface ILTPAnchorRegistry {
    /// One per-chain anchor for one block. Field set MUST match the
    /// Rust `Anchor` struct in `suwappudb-bridge` (chainId, height,
    /// stateRoot, parent, mac).
    struct Anchor {
        uint32 chainId;
        uint64 height;
        bytes32 stateRoot;
        bytes32 parent;
        bytes32 mac;
    }

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
    function setChainKey(uint32 chainId, bytes32 key) external;
    function addSigner(address signer) external;
    function removeSigner(address signer) external;

    // ===== Core =====
    function acceptAnchor(Anchor calldata anchor, bytes calldata signature) external;

    // ===== Read =====
    function verifyAnchor(Anchor calldata anchor) external view returns (bool);
    function verifyMac(Anchor calldata anchor) external view returns (bool);
    function getLastAnchor(uint32 chainId) external view returns (Anchor memory);
    function getLastHeight(uint32 chainId) external view returns (uint64);
    function recoverSigner(Anchor calldata anchor, bytes calldata signature)
        external pure returns (address);
    function computeMac(Anchor calldata anchor, bytes32 key)
        external pure returns (bytes32);
    function hashAnchor(Anchor calldata anchor) external pure returns (bytes32);

    // ===== State accessors =====
    function owner() external view returns (address);
    function lastAnchor(uint32 chainId) external view returns (
        uint32, uint64, bytes32, bytes32, bytes32
    );
    function chainKey(uint32 chainId) external view returns (bytes32);
    function lastHeightAccepted(uint32 chainId) external view returns (uint64);
    function isApprovedSigner(address signer) external view returns (bool);
}
