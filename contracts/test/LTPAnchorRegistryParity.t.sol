// SPDX-License-Identifier: MIT
pragma solidity ^0.8.20;

import {Test} from "forge-std/Test.sol";
import {LTPAnchorRegistry} from "../src/LTPAnchorRegistry.sol";

/**
 * @title LTPAnchorRegistryParityTest
 * @notice S11.5 cross-impl differential test.
 *
 * Reads parity_vectors.json (produced by Rust `cargo run --example
 * gen_parity_vectors`) and asserts every vector's signature recovers
 * to the embedded `expectedSigner` address via the contract's own
 * `recoverSigner` path.
 *
 * Vector schema (per entry):
 *
 *   { chainId:        uint32
 *   , height:         uint64
 *   , stateRoot:      bytes32
 *   , parent:         bytes32
 *   , mac:            bytes32  (zero for ECDSA anchors)
 *   , signature:      bytes65
 *   , expectedSigner: address
 *   }
 *
 * The fixture path is fixed: `test/fixtures/parity_vectors.json`
 * relative to the foundry root. Regenerate with:
 *
 *   cargo run --example gen_parity_vectors --release \
 *     -- contracts/test/fixtures/parity_vectors.json
 */
contract LTPAnchorRegistryParityTest is Test {
    LTPAnchorRegistry public registry;

    function setUp() public {
        registry = new LTPAnchorRegistry();
    }

    function testCrossImplParityVectors() public {
        string memory path = "test/fixtures/parity_vectors.json";
        string memory json = vm.readFile(path);

        // Parse the top-level signer address as a sanity check on the
        // envelope structure. parseJsonAddress paths use JSONPath.
        bytes memory signerBytes = vm.parseJson(json, ".signer_address");
        address embeddedSigner = abi.decode(signerBytes, (address));
        emit log_address(embeddedSigner);

        // The vectors array is variable-length; we iterate until
        // parseJson reverts (a clean off-the-end signal).
        for (uint256 i = 0; i < 64; i++) {
            string memory idx = vm.toString(i);
            string memory base = string.concat(".vectors[", idx, "]");

            // Probe the existence of vector i — if it doesn't exist,
            // parseJson reverts. try/catch is unavailable on
            // vm.parseJson (cheatcode), so we use vm.keyExists.
            string memory probeKey = string.concat(base, ".chainId");
            if (!vm.keyExists(json, probeKey)) {
                break;
            }

            LTPAnchorRegistry.Anchor memory anchor;
            anchor.chainId = uint32(
                abi.decode(vm.parseJson(json, string.concat(base, ".chainId")), (uint256))
            );
            anchor.height = uint64(
                abi.decode(vm.parseJson(json, string.concat(base, ".height")), (uint256))
            );
            anchor.stateRoot = abi.decode(
                vm.parseJson(json, string.concat(base, ".stateRoot")),
                (bytes32)
            );
            anchor.parent = abi.decode(
                vm.parseJson(json, string.concat(base, ".parent")),
                (bytes32)
            );
            anchor.mac = abi.decode(
                vm.parseJson(json, string.concat(base, ".mac")),
                (bytes32)
            );

            bytes memory signature = abi.decode(
                vm.parseJson(json, string.concat(base, ".signature")),
                (bytes)
            );
            address expectedSigner = abi.decode(
                vm.parseJson(json, string.concat(base, ".expectedSigner")),
                (address)
            );

            address recovered = registry.recoverSigner(anchor, signature);
            assertEq(
                recovered,
                expectedSigner,
                string.concat(
                    "Vector ",
                    idx,
                    ": recovered signer != expected - Rust ECDSA payload diverged from Solidity recoverSigner"
                )
            );
            assertEq(
                recovered,
                embeddedSigner,
                "Per-vector signer must match the envelope signer_address"
            );
        }
    }
}
