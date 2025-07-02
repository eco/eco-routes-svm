/* -*- c-basic-offset: 4 -*- */
// SPDX-License-Identifier: MIT
pragma solidity ^0.8.26;

/**
 * @notice Represents a single contract call with encoded function data
 * @dev Used to execute arbitrary function calls on the destination chain
 * @param target The contract address to call
 * @param data ABI-encoded function call data
 * @param value Amount of native tokens to send with the call
 */
struct Call {
    address target;
    bytes data;
    uint256 value;
}

/**
 * @notice Represents a token amount pair
 * @dev Used to specify token rewards and transfers
 * @param token Address of the ERC20 token contract
 * @param amount Amount of tokens in the token's smallest unit
 */
struct TokenAmount {
    address token;
    uint256 amount;
}

/**
 * @notice Defines the routing and execution instructions for cross-chain messages
 * @dev Contains all necessary information to route and execute a message on the destination chain
 * @param salt Unique identifier provided by the intent creator, used to prevent duplicates
 * @param source Chain ID where the intent originated
 * @param destination Target chain ID where the calls should be executed
 * @param inbox Address of the inbox contract on the destination chain that receives messages
 * @param tokens Array of tokens required for execution of calls on destination chain
 * @param calls Array of contract calls to execute on the destination chain in sequence
 */
struct Route {
    bytes32 salt;
    uint256 source;
    uint256 destination;
    address inbox;
    TokenAmount[] tokens;
    Call[] calls;
}

/**
 * @notice Defines the reward and validation parameters for cross-chain execution
 * @dev Specifies who can execute the intent and what rewards they receive
 * @param creator Address that created the intent and has authority to modify/cancel
 * @param prover Address of the prover contract that must approve execution
 * @param deadline Timestamp after which the intent can no longer be executed
 * @param nativeValue Amount of native tokens offered as reward
 * @param tokens Array of ERC20 tokens and amounts offered as additional rewards
 */
struct Reward {
    address creator;
    address prover;
    uint256 deadline;
    uint256 nativeValue;
    TokenAmount[] tokens;
}

/**
 * @notice Complete cross-chain intent combining routing and reward information
 * @dev Main structure used to process and execute cross-chain messages
 * @param route Routing and execution instructions
 * @param reward Reward and validation parameters
 */
struct Intent {
    Route route;
    Reward reward;
}