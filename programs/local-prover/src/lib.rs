//! # Local-Prover Program
//!
//! The Local-Prover program provides lightweight fulfillment verification for same-chain (Solana) 
//! intent validation in the Eco Routes protocol. Unlike the Hyper-Prover which handles
//! cross-chain scenarios via Hyperlane, the Local-Prover creates fulfillment records directly 
//! on Solana for intents fulfilled within the Solana ecosystem.
//!
//! ## Overview
//!
//! When intents specify Solana (chain ID 1) as their destination, fulfillment verification can be
//! handled locally without requiring cross-chain messaging infrastructure. The Local-Prover
//! program handles this optimization by creating fulfillment records directly on Solana, enabling
//! faster and more cost-effective reward settlement for same-chain operations.
//!
//! ## Core Functionality
//!
//! ### Direct Fulfillment Record Creation (`prove`)
//! - Creates fulfillment records for validated intent hash-claimant pairs
//! - Validates that the destination chain matches Solana's chain ID
//! - Emits `IntentProven` events for off-chain indexing and portal integration
//! - Uses deterministic PDA addressing for consistent fulfillment record location
//!
//! ### Account Cleanup (`close_proof`)  
//! - Closes fulfillment records after successful validation to reclaim rent
//! - Optimizes storage costs by cleaning up completed fulfillment records
//! - Maintains efficient account lifecycle management
//!
//! ## Architecture Benefits
//!
//! ### Performance Optimization
//! - **No Cross-Chain Delay**: Eliminates Hyperlane message transmission time
//! - **Lower Gas Costs**: Avoids cross-chain messaging fees and complexity
//! - **Immediate Finality**: Fulfillment record creation and validation happen atomically
//! - **Simplified Integration**: Direct fulfillment verification without external dependencies
//!
//! ### Security Model
//! - **Portal Authorization**: Only authorized portal dispatcher can create fulfillment records
//! - **Chain ID Validation**: Ensures records are only created for Solana destinations
//! - **Deterministic PDAs**: Prevents fulfillment record account collision and manipulation
//! - **Event Emission**: Enables verification and monitoring by off-chain systems
//!
//! ## Integration with Portal Program
//!
//! The Local-Prover integrates seamlessly with the Portal program:
//!
//! 1. **Intent Creation**: Users create intents with Solana as destination
//! 2. **Fulfillment**: Solvers execute operations directly on Solana
//! 3. **Fulfillment Record Creation**: Local-Prover creates fulfillment records for fulfilled intents
//! 4. **Reward Settlement**: Portal validates fulfillment records and releases rewards to solvers
//! 5. **Cleanup**: Fulfillment records are closed to reclaim rent
//!
//! ## State Management
//!
//! The program maintains minimal state for maximum efficiency:
//! - **ProofAccount**: Wrapper around `eco_svm_std::prover::Proof` structure for fulfillment records
//! - **PDA Derivation**: Uses `proof` + `intent_hash` + `program_id` for addressing
//! - **Account Extensions**: Implements `AccountExt` trait for standardized initialization
//!
//! ## Event System
//!
//! Emits standardized events compatible with the broader Eco Routes ecosystem:
//! - **IntentProven**: Contains intent hash, claimant address, and destination chain
//! - **CPI Events**: Uses `emit_cpi!` for cross-program event visibility
//! - **Off-chain Integration**: Events enable indexing, monitoring, and solver coordination
//!
//! ## Comparison with Hyper-Prover
//!
//! | Feature | Local-Prover | Hyper-Prover |
//! |---------|--------------|---------------|
//! | Use Case | Same-chain (Solana) intents | Cross-chain intents |
//! | Messaging | Direct on-chain record creation | Hyperlane cross-chain messages |
//! | Latency | Immediate | Depends on Hyperlane finality |
//! | Cost | Lower (no messaging fees) | Higher (cross-chain messaging) |
//! | Security | Portal validation | ISM + Portal validation |
//! | Complexity | Simple direct record creation | Complex cross-chain coordination |
//!
//! ## Usage Patterns
//!
//! ### Typical Flow
//! 1. Solver fulfills Solana-based intent
//! 2. Portal calls Local-Prover with intent hash and claimant
//! 3. Local-Prover validates destination is Solana (chain ID 1)  
//! 4. Creates fulfillment record account with deterministic PDA
//! 5. Emits `IntentProven` event for portal coordination
//! 6. Portal validates fulfillment record and releases rewards
//! 7. Fulfillment record account is closed to reclaim rent
//!
//! ### Error Handling
//! - **Invalid Domain**: Rejects fulfillment records for non-Solana destinations
//! - **Invalid Destination**: Validates destination chain ID matches expectation
//! - **Invalid Proof**: Prevents creation of invalid or duplicate fulfillment records
//! - **Already Proven**: Prevents double-record creation for same intent
//!
//! ## Integration Points
//!
//! The Local-Prover coordinates with:
//! - **Portal Program**: Primary caller for fulfillment record generation and validation
//! - **eco-svm-std**: Shared types and utilities for fulfillment record data structures  
//! - **Off-chain Solvers**: Indirect integration through portal program calls
//! - **Indexing Systems**: Event consumption for monitoring and analytics

use anchor_lang::prelude::*;
use eco_svm_std::prover;

declare_id!("34pNy1Kn6VzTrEK8fg1z24fknE8r1EYncASV7wQh1x6j");

pub mod instructions;
pub mod state;

use instructions::*;

#[program]
pub mod local_prover {

    use super::*;

    /// Creates fulfillment records for same-chain (Solana) intent validation.
    ///
    /// Creates fulfillment records for intents that have been fulfilled on Solana,
    /// eliminating the need for cross-chain messaging. This provides a more efficient
    /// path for reward settlement when both origin and destination are Solana.
    ///
    /// # Arguments
    /// * `ctx` - Program context with fulfillment record accounts and authority validation
    /// * `args` - Arguments containing domain ID, intent hashes, and claimant data
    ///
    /// # Security
    /// - Validates that destination domain matches Solana chain ID (1)
    /// - Ensures only authorized portal dispatcher can create fulfillment records  
    /// - Uses deterministic PDA addressing to prevent record manipulation
    /// - Prevents duplicate record creation through account existence checks
    ///
    /// # Events
    /// Emits `IntentProven` event for each validated intent hash-claimant pair,
    /// enabling off-chain indexing and portal program coordination.
    ///
    /// # Errors
    /// - `InvalidDomainId`: If domain ID doesn't match expected Solana chain ID
    /// - `InvalidDestination`: If destination chain is not Solana 
    /// - `InvalidProof`: If record data is malformed or accounts don't match
    /// - `IntentAlreadyProven`: If fulfillment record already exists for intent hash
    pub fn prove<'info>(
        ctx: Context<'_, '_, '_, 'info, Prove<'info>>,
        args: prover::ProveArgs,
    ) -> Result<()> {
        prove_intent(ctx, args)
    }

    /// Closes fulfillment record accounts to reclaim rent after successful validation.
    ///
    /// Performs cleanup of fulfillment record accounts that are no longer needed, typically
    /// after the Portal program has validated the fulfillment record and released rewards.
    /// This prevents account bloat and reclaims rent for storage optimization.
    ///
    /// # Arguments  
    /// * `ctx` - Program context with fulfillment record account to close
    ///
    /// # Security
    /// - Validates fulfillment record account ownership and proper PDA derivation
    /// - Ensures only authorized entities can close fulfillment record accounts
    /// - Prevents premature closure of active or unvalidated fulfillment records
    ///
    /// # Note
    /// This instruction should only be called after the Portal program has
    /// consumed the fulfillment record for reward settlement. Premature closure may prevent
    /// proper reward distribution to solvers.
    pub fn close_proof(ctx: Context<CloseProof>) -> Result<()> {
        instructions::close_proof(ctx)
    }
}
