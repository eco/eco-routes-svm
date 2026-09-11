//! Dependency-first templates. Remote recipients are data, never custody accounts.
use std::io::{self, Read, Write};

use anchor_lang::prelude::*;
use anchor_spl::associated_token::get_associated_token_address_with_program_id;

use crate::instructions::ChainerError;
use crate::types::{scale_amount, WAD};

pub const MAX_VAULTS: usize = 8;
pub const MAX_ITEMS: usize = 8;
/// Includes every node's route AND reward and the root, even unused nodes.
pub const MAX_RENDERED_BYTES: usize = 2 * 1024;
pub const MAX_ROUTE_LEN: usize = 2 * 1024;
/// Borsh Order bytes, excluding instruction/event framing. Also bounds announcements.
pub const MAX_ORDER_BYTES: usize = 4 * 1024;

/// Borsh enum tags: Input=0, Output=1. Unknown tags fail deserialization.
#[derive(AnchorSerialize, AnchorDeserialize, Clone, Copy, Debug, PartialEq)]
pub enum AmountSource {
    Input,
    Output,
}

#[derive(AnchorSerialize, AnchorDeserialize, Clone, Debug, PartialEq)]
pub struct Amount {
    pub source: AmountSource,
    /// Additional positive WAD scale, applied AFTER selecting the initial context.
    pub scale: u128,
    pub width: u8,
    pub little_endian: bool,
}

impl Amount {
    /// Equivalent rendered output to an old amount-only slot (not its commitment).
    pub fn output(width: u8, little_endian: bool) -> Self {
        Self {
            source: AmountSource::Output,
            scale: WAD,
            width,
            little_endian,
        }
    }

    fn validate(&self) -> Result<()> {
        require!(self.scale > 0, ChainerError::InvalidScale);
        require!(
            (1..=32).contains(&self.width),
            ChainerError::InvalidAmountWidth
        );
        Ok(())
    }

    fn encode(&self, context: AmountContext) -> Result<[u8; 32]> {
        let source = match self.source {
            AmountSource::Input => context.input as u128,
            AmountSource::Output => context.output,
        };
        encode_amount(scale_amount(source, self.scale)?, self)
    }
}

/// Borsh tags: Amount=0, Vault=1. Vault indices are u8, never bumps.
#[derive(AnchorSerialize, AnchorDeserialize, Clone, Debug, PartialEq)]
pub enum Item {
    Amount(Amount),
    Vault(u8),
}

#[derive(AnchorSerialize, Clone, Debug, PartialEq)]
pub struct Template {
    pub segments: Vec<Vec<u8>>,
    pub items: Vec<Item>,
}

impl Template {
    pub fn literal(bytes: Vec<u8>) -> Self {
        Self {
            segments: vec![bytes],
            items: vec![],
        }
    }

    /// Checks all geometry, parameters and dependencies without hashing/allocating.
    fn validate(&self, available: usize) -> Result<usize> {
        require!(self.items.len() <= MAX_ITEMS, ChainerError::TooManyItems);
        require!(
            self.segments.len() == self.items.len() + 1,
            ChainerError::SegmentCountMismatch
        );
        let mut len = 0usize;
        for segment in &self.segments {
            len = len
                .checked_add(segment.len())
                .ok_or(ChainerError::RouteTooLong)?;
            require!(len <= MAX_RENDERED_BYTES, ChainerError::RouteTooLong);
        }
        for item in &self.items {
            let width = match item {
                Item::Amount(amount) => {
                    amount.validate()?;
                    amount.width as usize
                }
                Item::Vault(index) => {
                    require!(
                        (*index as usize) < available,
                        ChainerError::InvalidVaultReference
                    );
                    32
                }
            };
            len = len.checked_add(width).ok_or(ChainerError::RouteTooLong)?;
            require!(len <= MAX_RENDERED_BYTES, ChainerError::RouteTooLong);
        }
        Ok(len)
    }

    /// The caller has validated the complete graph; no node is recursively rendered.
    fn write(
        &self,
        output: &mut impl Write,
        context: AmountContext,
        recipients: &[[u8; 32]],
    ) -> Result<()> {
        output.write_all(&self.segments[0])?;
        for (item, segment) in self.items.iter().zip(&self.segments[1..]) {
            match item {
                Item::Amount(amount) => {
                    output.write_all(&amount.encode(context)?[..amount.width as usize])?
                }
                Item::Vault(index) => output.write_all(&recipients[*index as usize])?,
            }
            output.write_all(segment)?;
        }
        Ok(())
    }

    fn hash(&self, context: AmountContext, recipients: &[[u8; 32]]) -> Result<[u8; 32]> {
        // Native scatter/gather Keccak: literal segments stay borrowed, encoded
        // items live on the stack. No intermediate route/reward Vec, and no
        // in-program Keccak permutation cost for each downstream template.
        let mut encoded = [[0; 32]; MAX_ITEMS];
        for (i, item) in self.items.iter().enumerate() {
            encoded[i] = match item {
                Item::Amount(amount) => amount.encode(context)?,
                Item::Vault(index) => recipients[*index as usize],
            };
        }
        let mut slices = [&[][..]; 2 * MAX_ITEMS + 1];
        for (i, item) in self.items.iter().enumerate() {
            let width = match item {
                Item::Amount(amount) => amount.width as usize,
                Item::Vault(_) => 32,
            };
            slices[2 * i] = &self.segments[i];
            slices[2 * i + 1] = &encoded[i][..width];
        }
        slices[2 * self.items.len()] = self.segments.last().unwrap();
        Ok(solana_keccak_hasher::hashv(&slices[..2 * self.items.len() + 1]).to_bytes())
    }
}

#[derive(AnchorSerialize, AnchorDeserialize, Clone, Debug, PartialEq)]
pub struct EvmDerivation {
    pub portal: [u8; 20],
    pub prefix: u8,
    pub implementation: [u8; 20],
    pub init_code_hash: [u8; 32],
}

#[derive(AnchorSerialize, AnchorDeserialize, Clone, Debug, PartialEq)]
pub struct SolanaDerivation {
    pub portal: Pubkey,
    pub token_program: Pubkey,
    pub mint: Pubkey,
}

/// Borsh tags: Evm=0 (includes TRON), Solana=1. No opaque bytes or bump fields.
#[derive(AnchorSerialize, AnchorDeserialize, Clone, Debug, PartialEq)]
pub enum Derivation {
    Evm(EvmDerivation),
    Solana(SolanaDerivation),
}

impl Derivation {
    pub fn validate(&self) -> Result<()> {
        match self {
            Self::Evm(c) => {
                require!(c.portal != [0; 20], ChainerError::MissingRemotePortal);
                require!(c.prefix != 0, ChainerError::MissingCreate2Prefix);
                require!(
                    c.implementation != [0; 20],
                    ChainerError::MissingImplementation
                );
                require!(
                    c.init_code_hash != [0; 32],
                    ChainerError::MissingInitCodeHash
                );
            }
            Self::Solana(c) => {
                require!(
                    c.portal != Pubkey::default(),
                    ChainerError::MissingRemotePortal
                );
                require!(
                    c.token_program != Pubkey::default(),
                    ChainerError::MissingTokenProgram
                );
                require!(c.mint != Pubkey::default(), ChainerError::MissingRemoteMint);
            }
        }
        Ok(())
    }

    /// 32-byte recipient. The supplied implementation/init-code relationship is
    /// not attestable here; it is the order author's explicit remote configuration.
    pub fn recipient(&self, intent_hash: &[u8; 32]) -> Result<[u8; 32]> {
        self.validate()?;
        Ok(match self {
            Self::Evm(c) => {
                let mut recipient = solana_keccak_hasher::hashv(&[
                    &[c.prefix],
                    &c.portal,
                    intent_hash,
                    &c.init_code_hash,
                ])
                .to_bytes();
                recipient[..12].fill(0);
                recipient
            }
            Self::Solana(c) => {
                let (vault, _) = Pubkey::find_program_address(&[b"vault", intent_hash], &c.portal);
                get_associated_token_address_with_program_id(&vault, &c.mint, &c.token_program)
                    .to_bytes()
            }
        })
    }
}

#[derive(AnchorSerialize, AnchorDeserialize, Clone, Debug, PartialEq)]
pub struct Vault {
    /// Downstream route-execution chain, NOT necessarily its reward-vault chain.
    pub destination: u64,
    pub route: Template,
    /// Entire target-protocol reward encoding, including ABI offsets when EVM.
    pub reward: Template,
    pub derivation: Derivation,
}

#[derive(AnchorSerialize, Clone, Debug, PartialEq)]
pub struct TemplateProgram {
    pub vaults: Vec<Vault>,
    pub route: Template,
}

/// Fixed once, immediately after the on-chain measurement. Output is NOT u64.
#[derive(Clone, Copy)]
pub struct AmountContext {
    pub input: u64,
    pub output: u128,
}

impl TemplateProgram {
    pub fn validate(&self) -> Result<usize> {
        require!(self.vaults.len() <= MAX_VAULTS, ChainerError::TooManyVaults);
        let mut total = 0usize;
        for (i, node) in self.vaults.iter().enumerate() {
            // The dependency-first constraint excludes self/forward/missing refs
            // and cycles, even in nodes the root does not reference.
            node.derivation.validate()?;
            total += node.route.validate(i)?;
            total += node.reward.validate(i)?;
            require!(
                total <= MAX_RENDERED_BYTES,
                ChainerError::RenderedBytesExceeded
            );
        }
        let root = self.route.validate(self.vaults.len())?;
        require!(root <= MAX_ROUTE_LEN, ChainerError::RouteTooLong);
        require!(
            total + root <= MAX_RENDERED_BYTES,
            ChainerError::RenderedBytesExceeded
        );
        Ok(root)
    }

    pub fn render(&self, context: AmountContext) -> Result<Vec<u8>> {
        let root_len = self.validate()?;
        let mut recipients = [[0; 32]; MAX_VAULTS];
        for (i, node) in self.vaults.iter().enumerate() {
            // Stream both templates; SBF's bump allocator does not reclaim Vecs.
            let route_hash = node.route.hash(context, &recipients[..i])?;
            let reward_hash = node.reward.hash(context, &recipients[..i])?;
            let hash = portal::types::intent_hash(
                node.destination,
                &route_hash.into(),
                &reward_hash.into(),
            );
            recipients[i] = node.derivation.recipient(&hash.into())?;
        }
        let mut route = Vec::with_capacity(root_len);
        self.route
            .write(&mut route, context, &recipients[..self.vaults.len()])?;
        Ok(route)
    }
}

/// Fixed-width encoding without narrowing the u128 amount context.
pub fn encode_amount(amount: u128, config: &Amount) -> Result<[u8; 32]> {
    config.validate()?;
    let width = config.width as usize;
    require!(
        width >= 16 || amount >> (width * 8) == 0,
        ChainerError::AmountExceedsWidth
    );
    let mut out = [0; 32];
    for (i, byte) in amount.to_le_bytes().iter().take(width).enumerate() {
        out[if config.little_endian {
            i
        } else {
            width - 1 - i
        }] = *byte;
    }
    Ok(out)
}

/// Read length prefixes before allocating, including when invoked through CPI.
fn bounded_vec<T: AnchorDeserialize>(reader: &mut impl Read, max: usize) -> io::Result<Vec<T>> {
    let len = u32::deserialize_reader(reader)? as usize;
    if len > max {
        return Err(io::Error::new(
            io::ErrorKind::InvalidData,
            "template bound exceeded",
        ));
    }
    let mut values = Vec::with_capacity(len);
    for _ in 0..len {
        values.push(T::deserialize_reader(reader)?);
    }
    Ok(values)
}

impl AnchorDeserialize for Template {
    fn deserialize_reader<R: Read>(reader: &mut R) -> io::Result<Self> {
        let len = u32::deserialize_reader(reader)? as usize;
        if len > MAX_ITEMS + 1 {
            return Err(io::Error::new(
                io::ErrorKind::InvalidData,
                "too many segments",
            ));
        }
        let mut segments = Vec::with_capacity(len);
        let mut remaining = MAX_RENDERED_BYTES;
        for _ in 0..len {
            let segment = bounded_vec::<u8>(reader, remaining)?;
            remaining -= segment.len();
            segments.push(segment);
        }
        Ok(Self {
            segments,
            items: bounded_vec(reader, MAX_ITEMS)?,
        })
    }
}

impl AnchorDeserialize for TemplateProgram {
    fn deserialize_reader<R: Read>(reader: &mut R) -> io::Result<Self> {
        Ok(Self {
            vaults: bounded_vec(reader, MAX_VAULTS)?,
            route: Template::deserialize_reader(reader)?,
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn shared_nodes_and_amounts_allocate_only_the_presized_root() {
        let mut vaults = Vec::new();
        for i in 0..MAX_VAULTS {
            let item = if i == 0 {
                Item::Amount(Amount::output(8, true))
            } else {
                Item::Vault((i - 1) as u8)
            };
            let template = Template {
                segments: vec![vec![], vec![]],
                items: vec![item],
            };
            vaults.push(Vault {
                destination: 480,
                route: template.clone(),
                reward: template,
                derivation: Derivation::Evm(EvmDerivation {
                    portal: [1; 20],
                    prefix: 255,
                    implementation: [2; 20],
                    init_code_hash: [3; 32],
                }),
            });
        }
        let program = TemplateProgram {
            vaults,
            route: Template {
                segments: vec![vec![]; MAX_ITEMS + 1],
                items: vec![Item::Vault((MAX_VAULTS - 1) as u8); MAX_ITEMS],
            },
        };
        crate::test_alloc::start_counting();
        let result = program.render(AmountContext {
            input: 17,
            output: 19,
        });
        let allocations = crate::test_alloc::stop_counting();
        assert_eq!(result.unwrap().len(), 32 * MAX_ITEMS);
        assert_eq!(
            allocations, 1,
            "no per-node, per-reference or success-path error allocations"
        );
    }
}
