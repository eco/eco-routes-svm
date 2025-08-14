//! # Hyper-Prover Program
//!
//! The Hyper-Prover program provides cross-chain message dispatch for the Eco Routes
//! intent protocol. It integrates with Hyperlane's cross-chain messaging infrastructure to 
//! transmit intent fulfillment notifications from Solana to other blockchain networks.
//!
//! ## Overview
//!
//! When intents are fulfilled on destination chains other than Solana, notification of fulfillment
//! must be communicated back to the origin chain. The Hyper-Prover program handles this critical
//! function by dispatching messages via Hyperlane and leveraging Hyperlane's security guarantees
//! for cross-chain message delivery.
//!
//! ## Core Functionality
//!
//! ### Message Dispatch (`prove`)
//! - Creates fulfillment records for validated intent hash-claimant pairs
//! - Generates Hyperlane messages containing fulfillment notification data
//! - Dispatches messages to origin chains via Hyperlane mailbox
//!
//! ### Message Handling (`handle`)
//! - Receives and processes incoming Hyperlane messages
//! - Validates message authenticity and sender authorization
//! - Creates fulfillment records based on validated message data
//!
//! ### Security Module Integration (`ism`, `ism_account_metas`)
//! - Implements Hyperlane's Interchain Security Module (ISM) interface
//! - Provides security validation for incoming cross-chain messages
//! - Enables custom security policies and validation rules
//!
//! ### Account Management (`close_proof`)
//! - Cleans up fulfillment record accounts after validation to reclaim rent
//! - Optimizes storage costs and prevents account bloat
//! - Maintains efficient account lifecycle management
//!
//! ## Hyperlane Integration
//!
//! The program integrates deeply with Hyperlane's architecture:
//!
//! - **Mailbox Program**: For reliable cross-chain message dispatch and receipt
//! - **ISM (Interchain Security Module)**: For custom security validation logic  
//! - **Message Routing**: Automatic routing to correct destination chains
//! - **Security Guarantees**: Inherits Hyperlane's security properties
//!
//! ## Security Model
//!
//! ### Message Validation
//! - All incoming messages are validated through ISM before processing
//! - Sender authorization prevents unauthorized message creation
//! - Message integrity verification ensures authentic communication
//!
//! ### Message Authenticity  
//! - Messages are tied to specific intent hashes and claimant addresses
//! - PDA derivation ensures deterministic and secure account addressing
//! - Event emission enables off-chain verification and indexing
//!
//! ### Access Control
//! - Configuration restricts authorized message senders
//! - Process authority validation prevents unauthorized operations
//! - Account ownership verification at every instruction boundary
//!
//! ## State Management
//!
//! The program maintains minimal but critical state:
//! - **Config**: Stores authorized sender addresses and program configuration
//! - **ProofAccount**: Holds fulfillment record data with destination chain and claimant information
//! - **Process Authority**: PDA-based authority for secure message processing
//!
//! ## Architecture Integration
//!
//! The Hyper-Prover works in coordination with:
//! - **Portal Program**: Source of intent creation and reward settlement
//! - **Origin Chain Contracts**: Recipients of proof messages for reward release
//! - **Hyperlane Infrastructure**: Message transport and security layer
//! - **Off-chain Solvers**: Entities that fulfill intents and trigger proof generation
//!
//! ## Usage Patterns
//!
//! 1. **Initialization**: Configure authorized senders and security parameters
//! 2. **Intent Fulfillment**: Solvers fulfill intents on destination chains
//! 3. **Message Dispatch**: Generate and dispatch fulfillment notifications via Hyperlane
//! 4. **Message Receipt**: Handle incoming fulfillment messages from other chains
//! 5. **Cleanup**: Close fulfillment record accounts to optimize storage costs
//!
//! ## Event Emission
//!
//! The program emits events for:
//! - Fulfillment record creation and validation
//! - Cross-chain message dispatch
//! - Account lifecycle changes
//! - Error conditions and security violations
//!
//! These events enable off-chain indexing, monitoring, and integration with solver infrastructure.

use anchor_lang::prelude::*;
use eco_svm_std::prover;

declare_id!("B4pMQaAGPZ7Mza9XnDxJfXZ1cUa4aa67zrNkv8zYAjx4");

pub mod hyperlane;
pub mod instructions;
pub mod state;

use instructions::*;

#[program]
pub mod hyper_prover {
    use super::*;

    /// Initializes the hyper-prover with configuration and authorized senders.
    ///
    /// Sets up the program's operational parameters including which addresses
    /// are authorized to send proof messages via Hyperlane integration.
    ///
    /// # Arguments
    /// * `ctx` - Program context with configuration accounts
    /// * `args` - Initialization parameters including authorized senders
    ///
    /// # Security
    /// - Creates secure configuration account with proper ownership
    /// - Validates authorized sender addresses for message filtering
    /// - Establishes process authority for secure operations
    pub fn init(ctx: Context<Init>, args: InitArgs) -> Result<()> {
        instructions::init(ctx, args)
    }

    /// Closes fulfillment record accounts to reclaim rent after successful validation.
    ///
    /// Performs cleanup of fulfillment record accounts that are no longer needed, allowing
    /// rent to be reclaimed and preventing unnecessary account bloat.
    ///
    /// # Arguments
    /// * `ctx` - Program context with fulfillment record account to close
    ///
    /// # Security
    /// - Validates fulfillment record account ownership and state
    /// - Ensures only authorized entities can close accounts
    /// - Prevents premature closure of active fulfillment records
    pub fn close_proof(ctx: Context<CloseProof>) -> Result<()> {
        instructions::close_proof(ctx)
    }

    /// Dispatches cross-chain fulfillment notification messages via Hyperlane.
    ///
    /// Creates fulfillment records for validated intent fulfillments and sends
    /// corresponding notification messages to origin chains via Hyperlane mailbox.
    ///
    /// # Arguments
    /// * `ctx` - Program context with fulfillment record and mailbox accounts
    /// * `args` - Message arguments including intent hashes and claimant data
    ///
    /// # Security
    /// - Validates all intent hash-claimant pairs before message dispatch
    /// - Ensures proper authorization through portal dispatcher validation
    /// - Creates secure fulfillment records with deterministic PDA addressing
    ///
    /// # Events
    /// Emits `IntentProven` events for off-chain indexing and validation.
    pub fn prove(ctx: Context<Prove>, args: prover::ProveArgs) -> Result<()> {
        prove_intent(ctx, args)
    }

    /// Handles incoming Hyperlane messages to create fulfillment records.
    ///
    /// Processes cross-chain messages received via Hyperlane mailbox, validates
    /// their authenticity, and creates corresponding fulfillment records for intent
    /// fulfillments that occurred on remote chains.
    ///
    /// # Arguments
    /// * `ctx` - Program context with message handling accounts
    /// * `origin` - Origin chain identifier for the message
    /// * `sender` - Address of the message sender on origin chain
    /// * `payload` - Message payload containing fulfillment notification data
    ///
    /// # Security
    /// - Validates message sender is authorized through ISM
    /// - Verifies message integrity and authenticity
    /// - Prevents replay attacks through proper account management
    ///
    /// # Note
    /// This instruction uses a custom discriminator to integrate with Hyperlane's
    /// message handling infrastructure.
    #[instruction(discriminator = &hyperlane::HANDLE_DISCRIMINATOR)]
    pub fn handle<'info>(
        ctx: Context<'_, '_, '_, 'info, Handle<'info>>,
        origin: u32,
        sender: [u8; 32],
        payload: Vec<u8>,
    ) -> Result<()> {
        instructions::handle(ctx, origin, sender, payload)
    }

    /// Provides account metadata for dynamic account discovery in message handling.
    ///
    /// Hyperlane infrastructure calls this function to determine what accounts
    /// are needed for message processing before calling the main handle function.
    ///
    /// # Arguments
    /// * `ctx` - Program context for metadata generation
    /// * `origin` - Origin chain identifier
    /// * `sender` - Message sender address
    /// * `payload` - Message payload for account determination
    ///
    /// # Returns
    /// Account metadata required for message processing operations.
    #[instruction(discriminator = &hyperlane::HANDLE_ACCOUNT_METAS_DISCRIMINATOR)]
    pub fn handle_account_metas(
        ctx: Context<HandleAccountMetas>,
        origin: u32,
        sender: [u8; 32],
        payload: Vec<u8>,
    ) -> Result<()> {
        instructions::handle_account_metas(ctx, origin, sender, payload)
    }

    /// Implements Hyperlane's Interchain Security Module (ISM) interface.
    ///
    /// Provides security validation for incoming cross-chain messages according
    /// to custom security policies. This ensures only authorized and valid
    /// messages are processed by the handle function.
    ///
    /// # Arguments
    /// * `ctx` - Program context with ISM validation accounts
    ///
    /// # Security
    /// - Validates message authenticity through Hyperlane's security mechanisms
    /// - Enforces sender authorization policies
    /// - Provides customizable security rules for different message types
    ///
    /// # Note
    /// Uses Hyperlane's standard ISM discriminator for protocol compatibility.
    #[instruction(discriminator = &hyperlane::INTERCHAIN_SECURITY_MODULE_DISCRIMINATOR)]
    pub fn ism(ctx: Context<Ism>) -> Result<()> {
        instructions::ism(ctx)
    }

    /// Provides account metadata for ISM validation operations.
    ///
    /// Called by Hyperlane infrastructure to determine what accounts are needed
    /// for security module validation before calling the main ism function.
    ///
    /// # Arguments  
    /// * `ctx` - Program context for ISM metadata generation
    ///
    /// # Returns
    /// Account metadata required for ISM validation operations.
    #[instruction(discriminator = &hyperlane::INTERCHAIN_SECURITY_MODULE_ACCOUNT_METAS_DISCRIMINATOR)]
    pub fn ism_account_metas(ctx: Context<IsmAccountMetas>) -> Result<()> {
        instructions::ism_account_metas(ctx)
    }
}
