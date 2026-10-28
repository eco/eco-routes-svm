//! Dependency-first templates. Remote recipients are data, never custody accounts.
use std::io::{self, Read, Write};

use anchor_lang::prelude::*;
use anchor_spl::associated_token::get_associated_token_address_with_program_id;
use eco_svm_std::Bytes32;

use crate::instructions::ChainerError;
use crate::types::{scale_amount, WAD};

pub const MAX_VAULTS: usize = 8;
pub const MAX_ITEMS: usize = 8;
/// What the nested vault nodes may render in total: every node's route AND reward,
/// even unused nodes. The root route has its own budget, [`MAX_ROUTE_LEN`].
pub const MAX_RENDERED_BYTES: usize = 2 * 1024;
/// The root route alone. A route this size is never copied: it is hashed from the
/// borrowed template segments, and only a publish renders it into memory.
pub const MAX_ROUTE_LEN: usize = 64 * 1024;
/// Borsh template bytes, excluding instruction/event framing: a literal root at
/// [`MAX_ROUTE_LEN`], the nested nodes at [`MAX_RENDERED_BYTES`], and the Borsh
/// framing of every template, node derivation and item at its maximum (about 4.3 KiB).
pub const MAX_ORDER_BYTES: usize = 72 * 1024;
/// The largest root a `publish` may carry. Portal emits the route in one
/// `IntentPublished` log, and the runtime silently truncates logs past its 10,000-byte
/// budget, so a larger route is refused rather than published truncated. Measured with an
/// escrow that creates its vault's ATA, the most logging a chain does: the event survives
/// whole up to 5,632 bytes, and Portal itself runs out of its own 32 KiB heap decoding and
/// re-emitting a route near 6.8 KB. This leaves about 600 bytes under the first limit.
pub const MAX_PUBLISHED_ROUTE_LEN: usize = 5_000;

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

    /// Borrow this template's segments and items; allocates nothing.
    pub fn view(&self) -> TemplateView<'_> {
        TemplateView {
            segments: Segments::Owned(&self.segments),
            items: std::borrow::Cow::Borrowed(&self.items),
        }
    }

    /// Read at most `limit` literal bytes across all segments, taking each segment's
    /// bytes from the reader as they arrive: a declared length is never allocated
    /// ahead of the data that backs it.
    fn deserialize_bounded<R: Read>(reader: &mut R, limit: usize) -> io::Result<Self> {
        let len = u32::deserialize_reader(reader)? as usize;
        if len > MAX_ITEMS + 1 {
            return Err(io::Error::new(
                io::ErrorKind::InvalidData,
                "too many segments",
            ));
        }
        let mut segments = Vec::with_capacity(len);
        let mut remaining = limit;
        for _ in 0..len {
            let segment = read_bytes(reader, remaining)?;
            remaining -= segment.len();
            segments.push(segment);
        }
        Ok(Self {
            segments,
            items: bounded_vec(reader, MAX_ITEMS)?,
        })
    }
}

/// A template's segments, either the owned ones or slices of a larger buffer.
#[derive(Debug)]
pub enum Segments<'a> {
    Owned(&'a [Vec<u8>]),
    Borrowed(Vec<&'a [u8]>),
}

impl Segments<'_> {
    fn len(&self) -> usize {
        match self {
            Self::Owned(segments) => segments.len(),
            Self::Borrowed(segments) => segments.len(),
        }
    }

    fn get(&self, index: usize) -> &[u8] {
        match self {
            Self::Owned(segments) => &segments[index],
            Self::Borrowed(segments) => segments[index],
        }
    }
}

/// A template read in place. The owned [`Template`] lends one for the small nested
/// nodes and for inline orders; the root of a staged order borrows its literal
/// segments straight from the buffer account, so a route of any size costs no heap.
#[derive(Debug)]
pub struct TemplateView<'a> {
    segments: Segments<'a>,
    items: std::borrow::Cow<'a, [Item]>,
}

impl TemplateView<'_> {
    /// Checks all geometry, parameters and dependencies without hashing/allocating.
    /// `limit` bounds the rendered length: the nested budget for a node, the root's
    /// own for the root.
    fn validate(&self, available: usize, limit: usize) -> Result<usize> {
        require!(self.items.len() <= MAX_ITEMS, ChainerError::TooManyItems);
        require!(
            self.segments.len() == self.items.len() + 1,
            ChainerError::SegmentCountMismatch
        );
        let mut len = 0usize;
        for i in 0..self.segments.len() {
            len = len
                .checked_add(self.segments.get(i).len())
                .ok_or(ChainerError::RouteTooLong)?;
            require!(len <= limit, ChainerError::RouteTooLong);
        }
        for item in self.items.iter() {
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
            require!(len <= limit, ChainerError::RouteTooLong);
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
        output.write_all(self.segments.get(0))?;
        for (i, item) in self.items.iter().enumerate() {
            match item {
                Item::Amount(amount) => {
                    output.write_all(&amount.encode(context)?[..amount.width as usize])?
                }
                Item::Vault(index) => output.write_all(&recipients[*index as usize])?,
            }
            output.write_all(self.segments.get(i + 1))?;
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
            slices[2 * i] = self.segments.get(i);
            slices[2 * i + 1] = &encoded[i][..width];
        }
        slices[2 * self.items.len()] = self.segments.get(self.segments.len() - 1);
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
    /// Commitment to the whole template program, including every nested node.
    ///
    /// SHA-256 by syscall over one contiguous Borsh copy: the preimage can be
    /// kilobytes, and the syscall is ~1 CU/byte against the ~130 CU/byte that
    /// streaming it through in-program keccak cost. Only ever a PDA seed here.
    /// A staged template is already one contiguous copy, in its buffer account, and
    /// is hashed there without another ([`TemplateProgramView::parse`]).
    pub fn hash(&self) -> Result<Bytes32> {
        crate::types::commitment_of(self)
    }

    /// Borrow this template for validation and rendering; allocates nothing.
    pub fn view(&self) -> TemplateProgramView<'_> {
        TemplateProgramView {
            vaults: std::borrow::Cow::Borrowed(&self.vaults),
            route: self.route.view(),
        }
    }

    /// Client helper. The handler independently measures and builds this same
    /// context on-chain before validating the caller's LOCAL accounts.
    pub fn build_route(&self, scale: u128, amount_in: u64) -> Result<Vec<u8>> {
        self.validate()?;
        self.render(AmountContext {
            input: amount_in,
            output: scale_amount(amount_in as u128, scale)?,
        })
    }

    pub fn validate(&self) -> Result<usize> {
        self.view().validate(crate::types::borsh_len(self)?)
    }

    pub fn render(&self, context: AmountContext) -> Result<Vec<u8>> {
        let view = self.view();
        let root_len = view.validate(crate::types::borsh_len(self)?)?;
        view.render(context, &view.recipients(context)?, root_len)
    }
}

/// A [`TemplateProgram`] read in place: the nested nodes owned (they are bounded
/// at [`MAX_RENDERED_BYTES`]), the root borrowed.
#[derive(Debug)]
pub struct TemplateProgramView<'a> {
    vaults: std::borrow::Cow<'a, [Vault]>,
    route: TemplateView<'a>,
}

impl<'a> TemplateProgramView<'a> {
    /// Read exactly one template from its canonical Borsh bytes, borrowing the root
    /// route's literal segments from `data` instead of copying them. Anything but the
    /// canonical encoding of one template, with nothing after it, is refused.
    pub fn parse(data: &'a [u8]) -> io::Result<Self> {
        let mut cursor = data;
        let vaults: Vec<Vault> = bounded_vec(&mut cursor, MAX_VAULTS)?;
        let count = u32::deserialize_reader(&mut cursor)? as usize;
        if count > MAX_ITEMS + 1 {
            return Err(io::Error::new(
                io::ErrorKind::InvalidData,
                "too many segments",
            ));
        }
        let mut segments = Vec::with_capacity(count);
        for _ in 0..count {
            let len = u32::deserialize_reader(&mut cursor)? as usize;
            if len > cursor.len() {
                return Err(io::ErrorKind::UnexpectedEof.into());
            }
            let (segment, rest) = cursor.split_at(len);
            segments.push(segment);
            cursor = rest;
        }
        let items: Vec<Item> = bounded_vec(&mut cursor, MAX_ITEMS)?;
        if !cursor.is_empty() {
            return Err(io::Error::new(
                io::ErrorKind::InvalidData,
                "bytes after the template",
            ));
        }

        Ok(Self {
            vaults: std::borrow::Cow::Owned(vaults),
            route: TemplateView {
                segments: Segments::Borrowed(segments),
                items: std::borrow::Cow::Owned(items),
            },
        })
    }

    /// Validate the geometry and every budget, given the template's Borsh length, and
    /// return the root route's rendered length.
    pub fn validate(&self, encoded_len: usize) -> Result<usize> {
        // The template is the large half of an order, so the byte cap lives here
        // rather than on the parameters, which are small and fixed-shape.
        require!(encoded_len <= MAX_ORDER_BYTES, ChainerError::OrderTooLarge);
        require!(self.vaults.len() <= MAX_VAULTS, ChainerError::TooManyVaults);
        let mut total = 0usize;
        for (i, node) in self.vaults.iter().enumerate() {
            // The dependency-first constraint excludes self/forward/missing refs
            // and cycles, even in nodes the root does not reference.
            node.derivation.validate()?;
            total += node.route.view().validate(i, MAX_RENDERED_BYTES)?;
            total += node.reward.view().validate(i, MAX_RENDERED_BYTES)?;
            require!(
                total <= MAX_RENDERED_BYTES,
                ChainerError::RenderedBytesExceeded
            );
        }
        self.route.validate(self.vaults.len(), MAX_ROUTE_LEN)
    }

    /// Every node's recipient, in order. Each node's route and reward are hashed in
    /// place; no node is rendered.
    pub fn recipients(&self, context: AmountContext) -> Result<[[u8; 32]; MAX_VAULTS]> {
        let mut recipients = [[0; 32]; MAX_VAULTS];
        for (i, node) in self.vaults.iter().enumerate() {
            // Stream both templates; SBF's bump allocator does not reclaim Vecs.
            let route_hash = node.route.view().hash(context, &recipients[..i])?;
            let reward_hash = node.reward.view().hash(context, &recipients[..i])?;
            let hash = portal::types::intent_hash(
                node.destination,
                &route_hash.into(),
                &reward_hash.into(),
            );
            recipients[i] = node.derivation.recipient(&hash.into())?;
        }
        Ok(recipients)
    }

    /// Keccak of the root route as rendered, without rendering it: the root's literal
    /// segments are hashed where they lie.
    pub fn route_hash(
        &self,
        context: AmountContext,
        recipients: &[[u8; 32]; MAX_VAULTS],
    ) -> Result<[u8; 32]> {
        self.route.hash(context, &recipients[..self.vaults.len()])
    }

    /// The root route as bytes, into one buffer of exactly `root_len`. Only a publish
    /// needs it: the commitment is [`Self::route_hash`]. `recipients` are the ones already
    /// computed for that hash; finding them again would repeat every node's PDA search.
    pub fn render(
        &self,
        context: AmountContext,
        recipients: &[[u8; 32]; MAX_VAULTS],
        root_len: usize,
    ) -> Result<Vec<u8>> {
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

/// A length-prefixed byte string of at most `max` bytes, read as the bytes arrive:
/// the prefix is checked against `max` but never trusted as an allocation size.
fn read_bytes(reader: &mut impl Read, max: usize) -> io::Result<Vec<u8>> {
    let len = u32::deserialize_reader(reader)? as usize;
    if len > max {
        return Err(io::Error::new(
            io::ErrorKind::InvalidData,
            "template bound exceeded",
        ));
    }
    let mut bytes = Vec::new();
    reader.take(len as u64).read_to_end(&mut bytes)?;
    if bytes.len() != len {
        return Err(io::ErrorKind::UnexpectedEof.into());
    }
    Ok(bytes)
}

/// A nested template: its literal bytes are bounded by the nested budget.
impl AnchorDeserialize for Template {
    fn deserialize_reader<R: Read>(reader: &mut R) -> io::Result<Self> {
        Self::deserialize_bounded(reader, MAX_RENDERED_BYTES)
    }
}

/// Cap encoded input while parsing, not just after allocating or hashing. The
/// template is the untrusted large half, so the bound belongs here; vector
/// lengths are separately bounded before allocation, below. This is the owned,
/// inline form, which a transaction packet keeps small; a staged template is read
/// in place by [`TemplateProgramView::parse`] instead.
impl AnchorDeserialize for TemplateProgram {
    fn deserialize_reader<R: Read>(reader: &mut R) -> io::Result<Self> {
        let reader = &mut reader.take(MAX_ORDER_BYTES as u64);
        Ok(Self {
            vaults: bounded_vec(reader, MAX_VAULTS)?,
            route: Template::deserialize_bounded(reader, MAX_ROUTE_LEN)?,
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
