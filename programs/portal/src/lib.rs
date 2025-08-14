//! # Portal Program
//!
//! The Portal program is the core component of the Eco Routes cross-chain intent protocol on Solana.
//! It manages the complete lifecycle of cross-chain intents, from creation and funding to fulfillment
//! and reward settlement.
//!
//! ## Overview
//!
//! The Portal program enables users to create "intents" - specifications for cross-chain operations
//! that are fulfilled by solvers in exchange for rewards. This follows the ERC-7683 intent standard
//! and provides a unified interface for cross-chain interactions.
//!
//! ## Core Intent Lifecycle
//!
//! 1. **Publish**: Users create intents specifying destination chains, routes, and rewards
//! 2. **Fund**: Users escrow tokens/SOL to incentivize solver execution
//! 3. **Fulfill**: Solvers execute the requested operations on destination chains
//! 4. **Prove**: Proof of fulfillment is submitted (via hyper-prover or local-prover)
//! 5. **Withdraw**: Solvers claim their earned rewards after successful proof validation
//! 6. **Refund**: Users can reclaim escrowed funds if intents expire unfulfilled
//!
//! ## Key Features
//!
//! - **Cross-Chain Compatibility**: Supports any blockchain via Hyperlane integration
//! - **ERC-7683 Standard**: Implements standard intent protocol interfaces
//! - **Flexible Rewards**: Support for native SOL and SPL token rewards
//! - **Proof Validation**: Integrates with both cross-chain and local proof systems
//! - **Gas Optimization**: Efficient account management and rent reclamation
//! - **Security**: Multi-layered validation and state machine enforcement
//!
//! ## Architecture
//!
//! The Portal program works in conjunction with:
//! - **Hyper-Prover**: For cross-chain intent proof delivery via Hyperlane
//! - **Local-Prover**: For same-chain (Solana) intent proof validation
//! - **eco-svm-std**: Shared types and utilities across the protocol
//!
//! ## State Management
//!
//! All intent state is managed through deterministic PDAs (Program Derived Addresses):
//! - Intent accounts track fulfillment status and reward details
//! - FulfillMarker accounts prevent double fulfillment
//! - WithdrawnMarker accounts prevent double withdrawal
//! - Vault accounts hold escrowed reward tokens
//!
//! ## Security Considerations
//!
//! - All state transitions are validated through strict state machine enforcement
//! - Cross-chain fulfillment validation is handled by prover programs
//! - Account ownership and authorization is enforced at every instruction
//! - Temporal constraints prevent expired intent fulfillment
//!
//! ## Usage
//!
//! This program is designed to be called by:
//! - End users creating and funding intents
//! - Solvers fulfilling intents and claiming rewards
//! - Proof systems (hyper-prover/local-prover) validating fulfillment
//! - Refund mechanisms for expired intents

use anchor_lang::prelude::*;

declare_id!("52gVFYqekRiSUxWwCKPNKw9LhBsVxbZiLSnGVsTBGh5F");

pub mod events;
pub mod instructions;
pub mod state;
pub mod types;

use instructions::*;

#[program]
pub mod portal {
    use super::*;

    /// Publishes a new cross-chain intent specification.
    ///
    /// Creates an intent that specifies the destination chain, route operations,
    /// and reward structure. This intent can then be funded and fulfilled by solvers.
    ///
    /// # Arguments
    /// * `ctx` - Program context containing required accounts
    /// * `args` - Intent parameters including destination, route, and rewards
    ///
    /// # Events
    /// Emits `IntentPublished` event for off-chain indexing and solver discovery.
    pub fn publish(ctx: Context<Publish>, args: PublishArgs) -> Result<()> {
        publish_intent(ctx, args)
    }

    /// Funds an intent with reward tokens to incentivize solver execution.
    ///
    /// Escrows native SOL and/or SPL tokens into secure vault accounts. Multiple
    /// funding calls are supported for partial funding scenarios.
    ///
    /// # Arguments
    /// * `ctx` - Program context with vault and token accounts
    /// * `args` - Funding parameters specifying intent hash and amounts
    ///
    /// # Security
    /// - Validates intent exists and is in fundable state
    /// - Ensures proper token account ownership and authorization
    /// - Creates rent-exempt vault accounts for token storage
    pub fn fund<'info>(ctx: Context<'_, '_, '_, 'info, Fund<'info>>, args: FundArgs) -> Result<()> {
        fund_intent(ctx, args)
    }

    /// Refunds escrowed tokens back to the original funder.
    ///
    /// Allows recovery of funds for intents that have expired without fulfillment
    /// or have been explicitly cancelled. Only callable by the original funder.
    ///
    /// # Arguments
    /// * `ctx` - Program context with vault and recipient accounts
    /// * `args` - Refund parameters specifying intent hash and amounts
    ///
    /// # Security
    /// - Validates intent is in refundable state (expired or cancelled)
    /// - Ensures only original funder can claim refunds
    /// - Prevents refunds of already fulfilled intents
    pub fn refund<'info>(
        ctx: Context<'_, '_, '_, 'info, Refund<'info>>,
        args: RefundArgs,
    ) -> Result<()> {
        refund_intent(ctx, args)
    }

    /// Withdraws earned rewards to solvers after successful intent fulfillment.
    ///
    /// Transfers escrowed tokens from vault accounts to the solver who successfully
    /// fulfilled the intent. Only callable after valid proof submission.
    ///
    /// # Arguments
    /// * `ctx` - Program context with vault and recipient accounts  
    /// * `args` - Withdrawal parameters specifying intent hash and amounts
    ///
    /// # Security
    /// - Validates intent has been proven fulfilled
    /// - Ensures only authorized claimant can withdraw
    /// - Prevents double withdrawal through marker accounts
    pub fn withdraw<'info>(
        ctx: Context<'_, '_, '_, 'info, Withdraw<'info>>,
        args: WithdrawArgs,
    ) -> Result<()> {
        withdraw_intent(ctx, args)
    }

    /// Marks an intent as fulfilled after successful execution on destination chain.
    ///
    /// Called by solvers to indicate they have completed the requested operations.
    /// This creates a fulfillment marker that can be validated by proof systems.
    ///
    /// # Arguments
    /// * `ctx` - Program context with fulfill marker accounts
    /// * `args` - Fulfillment parameters including intent hash and calldata
    ///
    /// # Events
    /// Emits `IntentFulfilled` event for proof system coordination.
    ///
    /// # Security
    /// - Validates intent exists and is in fulfillable state
    /// - Prevents duplicate fulfillment through marker accounts
    /// - Records fulfillment details for proof validation
    pub fn fulfill<'info>(
        ctx: Context<'_, '_, '_, 'info, Fulfill<'info>>,
        args: FulfillArgs,
    ) -> Result<()> {
        fulfill_intent(ctx, args)
    }

    /// Notifies prover programs that intents have been fulfilled.
    ///
    /// Tells the prover program that these intents are fulfilled and to do whatever
    /// is necessary so that the prover on the source chain can verify fulfillment.
    /// This typically involves sending messages back to the origin chain to enable
    /// reward release. The specific implementation is handled by the prover program.
    ///
    /// # Arguments
    /// * `ctx` - Program context with prover coordination accounts
    /// * `args` - Parameters specifying which fulfilled intents to notify the prover about
    ///
    /// # Security
    /// - Validates intent fulfillment status before notifying prover
    /// - Ensures only authorized prover programs can be called
    /// - Coordinates with prover programs for source chain verification
    pub fn prove<'info>(
        ctx: Context<'_, '_, '_, 'info, Prove<'info>>,
        args: ProveArgs,
    ) -> Result<()> {
        prove_intent(ctx, args)
    }
}
