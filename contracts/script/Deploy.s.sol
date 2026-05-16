// SPDX-License-Identifier: MIT
pragma solidity ^0.8.20;

import {Script, console} from "forge-std/Script.sol";
import {LTPAnchorRegistry} from "../src/LTPAnchorRegistry.sol";

/**
 * @title Deploy
 * @notice S11.4 — deploy LTPAnchorRegistry to a target chain.
 *
 * Reads configuration from env:
 *
 *   PRIVATE_KEY           — deployer key (must hold gas).
 *   INITIAL_SIGNER        — first approved signer; the address whose
 *                           ECDSA key gsxdb-bridge's EcdsaSecp256k1Signer
 *                           will use. Optional; if unset, only the
 *                           deployer becomes owner and signer set is
 *                           empty (admin can `addSigner` later).
 *   INITIAL_CHAIN_ID      — uint32. Optional; if set together with
 *                           INITIAL_CHAIN_KEY, registers the chain.
 *   INITIAL_CHAIN_KEY     — bytes32. Companion to INITIAL_CHAIN_ID.
 *
 * Usage:
 *
 *   anvil (local):
 *     forge script script/Deploy.s.sol --rpc-url http://127.0.0.1:8545 \
 *       --broadcast --private-key 0xac0974… --legacy
 *
 *   Sepolia:
 *     forge script script/Deploy.s.sol --rpc-url $SEPOLIA_RPC_URL \
 *       --broadcast --verify --etherscan-api-key $ETHERSCAN_API_KEY \
 *       --private-key $PRIVATE_KEY
 *
 * The deployed address is logged so external integrators can pick it
 * up from CI artifacts.
 */
contract Deploy is Script {
    function run() external returns (LTPAnchorRegistry registry) {
        uint256 deployerKey = vm.envUint("PRIVATE_KEY");

        vm.startBroadcast(deployerKey);

        registry = new LTPAnchorRegistry();
        console.log("LTPAnchorRegistry deployed at:", address(registry));
        console.log("Owner:", registry.owner());

        // Optional initial signer.
        address initialSigner = vm.envOr("INITIAL_SIGNER", address(0));
        if (initialSigner != address(0)) {
            registry.addSigner(initialSigner);
            console.log("Initial signer approved:", initialSigner);
        }

        // Optional initial chain registration.
        uint32 chainId = uint32(vm.envOr("INITIAL_CHAIN_ID", uint256(0)));
        bytes32 chainKey = vm.envOr("INITIAL_CHAIN_KEY", bytes32(0));
        if (chainId != 0 && chainKey != bytes32(0)) {
            registry.setChainKey(chainId, chainKey);
            console.log("Initial chain key set for chainId:");
            console.log(chainId);
        }

        vm.stopBroadcast();
    }
}
