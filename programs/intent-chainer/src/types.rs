use anchor_lang::prelude::*;
use eco_svm_std::Bytes32;
use portal::types::Reward;
use tiny_keccak::{Hasher, Keccak};

use crate::instructions::ChainerError;
use crate::keccak_writer::KeccakWriter;

/// Fixed-point denominator for [`Order::scale`].
///
/// Decimal, not binary, so every power-of-ten unit conversion is exact in both
/// directions: 6-to-18 decimals is `1e30`, 18-to-6 is `1e6`, and a same-unit
/// lane is `WAD`, all integers. A binary denominator (Q64, Q128) cannot do
/// that — `2^128 / 1e12` is not an integer, so a downscaling lane would lean on
/// rounding to recover a value it should compute exactly. Matches the EVM
/// `IntentChainer.WAD` and the WAD convention v3 adopts for `RewardToken.rate`.
pub const WAD: u128 = 1_000_000_000_000_000_000;

/// Upper bound on slots per order, bounding the splice loop's compute.
/// Matches the EVM `IntentChainer.MAX_SLOTS`.
pub const MAX_SLOTS: usize = 8;

/// Upper bound on the spliced route length.
///
/// **Set by measurement, not by arithmetic.** Three ceilings were candidates and
/// the intuitive ranking was wrong; `chain` at various route lengths, publishing
/// through portal, measures:
///
/// | route bytes | compute units | transaction log bytes |
/// |------------:|--------------:|----------------------:|
/// |        1024 |       450_496 |                 3_819 |
/// |        2048 |       749_781 |                 5_181 |
/// |        2560 |       877_855 |                 5_861 |
/// |        3072 |     1_049_099 |                 6_546 |
/// |        3584 |    heap OOM   |                     — |
///
/// - The **heap** binds first. This program does not install a custom allocator,
///   so it runs on the stock 32 KB heap, and the route exists there several times
///   over — Anchor's argument deserialization, the splice, and the `publish` CPI's
///   own serialization — on an allocator that never frees. Past ~3 KB that is an
///   unrecoverable `ProgramFailedToComplete`.
/// - **Compute** binds next, at roughly 300 CU per route byte, because
///   `portal::publish` hashes the route with `tiny_keccak` in-program rather than
///   through the keccak syscall. 3 KB already costs 1.05M of the 1.4M a
///   transaction can request.
/// - The **log budget** never binds in the reachable range, which is the
///   correction worth recording: it looked like the dangerous ceiling because it
///   fails *silently* — `LogCollector` drops an oversized `Program data:` line
///   without failing the transaction — but the two hard ceilings arrive first and
///   fail loudly. `published_event_survives_the_log_budget_at_max_route_len`
///   asserts the event is still byte-exact at the cap so that stays true if the
///   constant is ever raised.
///
/// 2 KB leaves ~1 KB of margin under the heap cliff and costs ~750k CU, so a
/// caller publishing at the cap must raise its compute limit above the 400k
/// default. Raising this constant requires re-running those measurements, and
/// past ~3 KB requires the `flash-fulfiller` treatment: a custom `BumpAllocator`
/// plus a mandatory `request_heap_frame` on every client transaction.
pub const MAX_ROUTE_LEN: usize = 2 * 1024;

/// Minimum time intent2's reward deadline must clear the current slot by.
///
/// Intent2's deadlines are fixed when intent1 is authored, but intent1 may be
/// fulfilled at any point up to its own route deadline. Publishing an intent2
/// that is already expired is recoverable — the escrow refunds to
/// `reward.creator` after the deadline — but it burns intent1 for nothing, so it
/// fails loudly here instead. Matches the EVM `MIN_DEADLINE_BUFFER`.
pub const MIN_DEADLINE_BUFFER: u64 = 5 * 60;

/// A position in intent2's route bytes that receives the destination amount.
///
/// Every slot receives the **same** scalar, `ceil(amount_in * scale / WAD)` —
/// what intent2's solver must supply on the destination. It appears once as the
/// route's token-leg amount and again inside any call that moves it. Because
/// both want the identical number, no per-slot discriminator is needed.
///
/// Width and byte order are the **destination VM's**, not Solana's: a Solana
/// route carries Borsh 8-byte little-endian `u64`s, while an EVM route carries
/// `abi.encode`d 32-byte big-endian words.
#[derive(AnchorSerialize, AnchorDeserialize, Clone, Debug, PartialEq)]
pub struct Slot {
    /// Bytes written, 1..=32. A value that does not fit reverts rather than truncating.
    pub width: u8,
    /// True for Borsh/Solana byte order, false for EVM big-endian words.
    pub little_endian: bool,
}

/// A fully committed intent2, minus the one amount that does not exist yet.
///
/// The route is carried as literal **segments** surrounding the [`Slot`]
/// positions rather than as whole bytes plus numeric write offsets. The route is
/// rebuilt by concatenation — `segments[0] ‖ enc(slots[0]) ‖ segments[1] ‖ …` —
/// so a mis-stated write position is **not expressible**, and the same encoding
/// serves an EVM destination and a Borsh one without this program knowing which
/// it is holding.
///
/// Offsets would be actively wrong here, for two independent reasons:
///
/// - Canonical `abi.encode(Route)` puts every argument inside `calls[k].data` at
///   an absolute offset `≡ 4 (mod 32)`, because the 4-byte selector shifts the
///   payload — so the 32-byte alignment invariant one would naturally assert
///   rejects every real EVM swap route.
/// - In a Solana route the amount appears twice, and the second position moves
///   with the call's account list while the first does not.
///
/// The whole struct is hashed into [`crate::state::order_commitment`], which
/// seeds the escrow authority — so every field here is bound to the address
/// intent1 delivers into. See the module docs on `lib.rs` for why that is the
/// authorization anchor.
#[derive(AnchorSerialize, AnchorDeserialize, Clone, Debug)]
pub struct Order {
    /// The single mint measured in the escrow and escrowed as intent2's reward.
    pub base_mint: Pubkey,
    /// Intent2's destination chain id.
    pub destination: u64,
    /// Literal route bytes between slots. MUST be `slots.len() + 1` entries; the
    /// first and last may be empty.
    pub segments: Vec<Vec<u8>>,
    /// Positions receiving the measured amount, in route order.
    pub slots: Vec<Slot>,
    /// Intent2's reward. MUST carry exactly one leg, in `base_mint`, with a zero
    /// `amount` and no native amount. The leg's amount is overwritten with the
    /// measured amount at chain time; requiring it to be authored as zero is what
    /// makes the commitment preimage canonical, so the caller cannot present a
    /// materially different order that hashes the same way.
    pub reward: Reward,
    /// The **entire** source-to-destination transform, [`WAD`]-denominated:
    /// `amount_out = ceil(amount_in * scale / WAD)`.
    ///
    /// It carries both the unit conversion and intent2's solver spread, because
    /// those compose into one ratio and there is no reason to commit two numbers
    /// where one will do.
    ///
    /// THE UNIT PART exists because "the same token" is not the same unit across
    /// chains: USDC is 6 decimals on Solana and Base, but Binance-Peg USDC on BNB
    /// Chain is 18.
    ///
    /// THE SPREAD PART is proportional, not flat. The reward leg escrows the whole
    /// measured `amount_in` while the route obliges only `amount_in * scale`, so
    /// the difference is the solver's entire margin and it grows with the amount.
    /// That is a deliberate trade, and the same one the EVM contract makes: a flat
    /// fee would price destination gas independently of size, but it cannot be
    /// folded into a ratio, and the only thing that moves `amount_in` here is swap
    /// slippage — a percent or so around a known expectation — over which the two
    /// are indistinguishable. Use `min_amount_in` to express "too small to be
    /// worth filling"; it says that directly.
    ///
    /// Rounding is toward the **user** (up), because this value is the solver's
    /// delivery floor — rounding down would quietly shave the last unit off what
    /// the user receives on every downscaling lane.
    pub scale: u128,
    /// Floor on the measured amount. Below it `chain` fails, so a swap that
    /// under-delivered leaves the escrow intact for a later, larger measurement
    /// instead of publishing an intent nobody will fill.
    pub min_amount_in: u64,
    /// Whether `chain` MUST emit portal's canonical `IntentPublished`.
    ///
    /// Committed rather than left to the caller, because every other field that
    /// decides the outcome is committed and this one gates *discoverability*.
    /// `announce_order` makes an order public, and `chain` is permissionless and
    /// needs nothing else — so with the flag caller-chosen, a solver watching
    /// `OrderAnnounced` could front-run the author with `publish = false`, fund
    /// intent2, and keep it out of the stream every other solver keys on, then
    /// fill it uncontested. Recoverable (anyone can call `portal::publish`
    /// afterward with the route and reward) but an exclusivity window the author
    /// never agreed to.
    ///
    /// The caller may still *strengthen* this — passing `publish = true` when the
    /// order does not require it — so an author who wants the private path
    /// (`false`) keeps it, and one who wants discoverability can no longer have it
    /// taken away. The two features pull against each other exactly here:
    /// durability wants the order public, permissionless `chain` wants it private.
    pub require_publish: bool,
}

impl Order {
    /// Keccak over the Borsh encoding of the whole order. Seeds the escrow
    /// authority, so this is the value that binds intent1's delivery address to
    /// exactly one order.
    pub fn hash(&self) -> Bytes32 {
        let mut hasher = Keccak::v256();
        let mut hash = [0u8; 32];

        self.serialize(&mut KeccakWriter::new(&mut hasher))
            .expect("Order borsh serialization is infallible");
        hasher.finalize(&mut hash);

        hash.into()
    }

    /// Total length of the route this order splices, without building it.
    pub fn route_len(&self) -> Result<usize> {
        let segments: usize = self
            .segments
            .iter()
            .try_fold(0usize, |acc, segment| acc.checked_add(segment.len()))
            .ok_or(ChainerError::RouteTooLong)?;

        self.slots
            .iter()
            .try_fold(segments, |acc, slot| acc.checked_add(slot.width as usize))
            .ok_or(ChainerError::RouteTooLong.into())
    }

    /// Rebuild intent2's route bytes, splicing `amount_out` into every slot.
    ///
    /// Concatenation, not overwriting: the author commits the bytes AROUND each
    /// amount rather than an offset INTO a blob, so there is no arithmetic that
    /// can land a write on a Borsh vector length, an SPL account pubkey, or an ABI
    /// tail offset.
    pub fn build_route(&self, amount_out: u128) -> Result<Vec<u8>> {
        require!(self.slots.len() <= MAX_SLOTS, ChainerError::TooManySlots);
        require!(
            self.segments.len() == self.slots.len() + 1,
            ChainerError::SegmentCountMismatch
        );

        let route_len = self.route_len()?;
        require!(route_len <= MAX_ROUTE_LEN, ChainerError::RouteTooLong);

        // Pre-sized so the splice never reallocates. Solana's allocator does not
        // free, so a doubling `Vec` would retain every intermediate buffer.
        let mut route = Vec::with_capacity(route_len);
        route.extend_from_slice(&self.segments[0]);

        self.slots
            .iter()
            .zip(self.segments.iter().skip(1))
            .try_for_each(|(slot, segment)| {
                encode_amount_into(&mut route, amount_out, slot)?;
                route.extend_from_slice(segment);

                Result::Ok(())
            })?;

        Ok(route)
    }
}

/// Append `amount` to `out` in one slot's width and byte order.
///
/// Reverts rather than truncating when the value does not fit. Silent truncation
/// is the dangerous case: an amount wrapped into a Solana `u64` would publish an
/// intent2 that is well-formed, fillable, and pays out a fraction of what was
/// escrowed.
pub fn encode_amount_into(out: &mut Vec<u8>, amount: u128, slot: &Slot) -> Result<()> {
    let width = slot.width as usize;
    require!((1..=32).contains(&width), ChainerError::InvalidSlotWidth);
    // A `u128` is 16 bytes, so anything 16 bytes wide or wider always fits and the
    // shift below would itself overflow.
    require!(
        width >= 16 || amount >> (width * 8) == 0,
        ChainerError::AmountExceedsSlotWidth
    );

    // Zero-fill the slot, then write positionally into it. A big-endian slot fills
    // back to front, so it needs the space to exist first; `bytes` past index 15 is
    // absent for a `u128`, which is the zero high-order padding a wide slot wants.
    let start = out.len();
    out.resize(start + width, 0);

    let bytes = amount.to_le_bytes();
    (0..width).for_each(|i| {
        let byte = bytes.get(i).copied().unwrap_or(0);
        let offset = if slot.little_endian { i } else { width - 1 - i };
        out[start + offset] = byte;
    });

    Ok(())
}

/// `ceil(amount_in * scale / WAD)`, the destination obligation.
///
/// The fraction is reduced by `gcd(scale, WAD)` before multiplying, which is what
/// keeps every realistic lane inside `u128`. Solana has no 256-bit integer and
/// SBF's 128-bit division is a compiler intrinsic, so the naive
/// `amount_in * scale` would overflow exactly on the lanes the unit conversion
/// exists for: a 6→18 lane is `scale = 1e30`, and 1M USDC (`amount_in = 1e12`)
/// gives `1e42` against a `u128` ceiling of `~3.4e38`. Reduction turns that same
/// lane into `amount_in * 1e12 / 1`, which fits comfortably.
///
/// Because unit conversions are powers of ten and `WAD` is `1e18`, the reduced
/// numerator is small for every lane in the table; a `scale` pathological enough
/// to overflow anyway fails loudly as [`ChainerError::ScaleOverflow`] rather than
/// wrapping.
///
/// With `amount_in >= 1` and a reduced numerator `>= 1`, the ceiling is always at
/// least 1, so a zero obligation is unreachable and needs no explicit check —
/// the same property the EVM contract relies on.
pub fn scale_amount(amount_in: u64, scale: u128) -> Result<u128> {
    require!(scale > 0, ChainerError::InvalidScale);

    let divisor = gcd(scale, WAD);
    let numerator = scale / divisor;
    let denominator = WAD / divisor;

    (amount_in as u128)
        .checked_mul(numerator)
        .map(|product| product.div_ceil(denominator))
        .ok_or(ChainerError::ScaleOverflow.into())
}

fn gcd(mut a: u128, mut b: u128) -> u128 {
    while b != 0 {
        let remainder = a % b;
        a = b;
        b = remainder;
    }

    a
}

#[cfg(test)]
mod tests {
    use portal::types::TokenAmount;

    use super::*;

    fn order(segments: Vec<Vec<u8>>, slots: Vec<Slot>) -> Order {
        Order {
            base_mint: Pubkey::new_from_array([3u8; 32]),
            destination: 1399811150,
            segments,
            slots,
            reward: Reward {
                deadline: 1_700_000_000,
                creator: Pubkey::new_from_array([1u8; 32]),
                prover: Pubkey::new_from_array([2u8; 32]),
                native_amount: 0,
                tokens: vec![TokenAmount {
                    token: Pubkey::new_from_array([3u8; 32]),
                    amount: 0,
                }],
            },
            scale: WAD,
            min_amount_in: 1,
            require_publish: false,
        }
    }

    // ---------- splice ----------

    #[test]
    fn build_route_splices_a_little_endian_u64() {
        let built = order(
            vec![vec![0xAA, 0xBB], vec![0xCC]],
            vec![Slot {
                width: 8,
                little_endian: true,
            }],
        )
        .build_route(0x0102030405060708)
        .unwrap();

        assert_eq!(
            built,
            vec![0xAA, 0xBB, 0x08, 0x07, 0x06, 0x05, 0x04, 0x03, 0x02, 0x01, 0xCC]
        );
    }

    #[test]
    fn build_route_splices_a_big_endian_word() {
        let built = order(
            vec![vec![], vec![]],
            vec![Slot {
                width: 32,
                little_endian: false,
            }],
        )
        .build_route(0x0102030405060708)
        .unwrap();

        let mut expected = vec![0u8; 24];
        expected.extend_from_slice(&[0x01, 0x02, 0x03, 0x04, 0x05, 0x06, 0x07, 0x08]);
        assert_eq!(built, expected);
    }

    /// The Solana lane: the amount appears twice, both little-endian u64, and
    /// every other byte must survive untouched.
    #[test]
    fn build_route_splices_both_solana_positions() {
        let built = order(
            vec![vec![0x11; 4], vec![0x22; 3], vec![0x33; 2]],
            vec![
                Slot {
                    width: 8,
                    little_endian: true,
                },
                Slot {
                    width: 8,
                    little_endian: true,
                },
            ],
        )
        .build_route(1_000_000)
        .unwrap();

        let amount = 1_000_000u64.to_le_bytes();
        let mut expected = vec![0x11; 4];
        expected.extend_from_slice(&amount);
        expected.extend_from_slice(&[0x22; 3]);
        expected.extend_from_slice(&amount);
        expected.extend_from_slice(&[0x33; 2]);

        assert_eq!(built, expected);
        goldie::assert_json!(built);
    }

    #[test]
    fn build_route_with_no_slots_is_the_single_segment() {
        let built = order(vec![vec![1, 2, 3]], vec![]).build_route(999).unwrap();

        assert_eq!(built, vec![1, 2, 3]);
    }

    #[test]
    fn build_route_rejects_a_segment_count_mismatch() {
        let result = order(
            vec![vec![0xAA]],
            vec![Slot {
                width: 8,
                little_endian: true,
            }],
        )
        .build_route(1);

        assert!(result.is_err());
    }

    #[test]
    fn build_route_rejects_too_many_slots() {
        let slots: Vec<_> = (0..MAX_SLOTS + 1)
            .map(|_| Slot {
                width: 8,
                little_endian: true,
            })
            .collect();
        let segments = vec![vec![]; MAX_SLOTS + 2];

        assert!(order(segments, slots).build_route(1).is_err());
    }

    #[test]
    fn build_route_rejects_a_route_over_the_cap() {
        let result = order(vec![vec![0u8; MAX_ROUTE_LEN + 1]], vec![]).build_route(1);

        assert!(result.is_err());
    }

    #[test]
    fn build_route_accepts_a_route_at_the_cap() {
        let result = order(vec![vec![0u8; MAX_ROUTE_LEN]], vec![]).build_route(1);

        assert!(result.is_ok());
    }

    // ---------- slot encoding ----------

    /// The load-bearing property: a value that does not fit its width must revert,
    /// never truncate. An amount wrapped into a Solana u64 would publish an
    /// intent2 that is well-formed, fillable, and pays a fraction of the escrow.
    #[test]
    fn encode_amount_rejects_a_value_wider_than_its_slot() {
        let mut out = vec![];
        let slot = Slot {
            width: 8,
            little_endian: true,
        };

        assert!(encode_amount_into(&mut out, u64::MAX as u128, &slot).is_ok());

        let mut out = vec![];
        assert!(encode_amount_into(&mut out, u64::MAX as u128 + 1, &slot).is_err());
    }

    #[test]
    fn encode_amount_rejects_a_zero_or_oversized_width() {
        let mut out = vec![];

        assert!(encode_amount_into(
            &mut out,
            1,
            &Slot {
                width: 0,
                little_endian: true
            }
        )
        .is_err());
        assert!(encode_amount_into(
            &mut out,
            1,
            &Slot {
                width: 33,
                little_endian: true
            }
        )
        .is_err());
    }

    /// A 16-byte-or-wider slot always fits a `u128`, and the fits-in-width shift
    /// would itself overflow — so the width guard must short-circuit, not shift.
    #[test]
    fn encode_amount_handles_widths_at_and_above_the_u128_size() {
        [16, 20, 32].into_iter().for_each(|width| {
            let mut out = vec![];
            assert!(encode_amount_into(
                &mut out,
                u128::MAX,
                &Slot {
                    width,
                    little_endian: false
                }
            )
            .is_ok());
            assert_eq!(out.len(), width as usize);
        });
    }

    // ---------- scale ----------

    #[test]
    fn scale_amount_is_identity_at_wad() {
        assert_eq!(scale_amount(1_000_000, WAD).unwrap(), 1_000_000);
    }

    #[test]
    fn scale_amount_applies_a_proportional_spread() {
        // 100 bps off 1 USDC
        assert_eq!(scale_amount(1_000_000, WAD / 100 * 99).unwrap(), 990_000);
    }

    /// The lane the unit conversion exists for, and the one a naive
    /// `amount_in * scale` overflows: `1e12 * 1e30 = 1e42` against a `u128`
    /// ceiling of `~3.4e38`. gcd reduction makes it exact and cheap.
    #[test]
    fn scale_amount_upscales_six_to_eighteen_decimals_without_overflow() {
        let scale = 10u128.pow(30);

        assert_eq!(
            scale_amount(1_000_000_000_000, scale).unwrap(),
            1_000_000_000_000_000_000_000_000
        );
    }

    #[test]
    fn scale_amount_downscales_eighteen_to_six_decimals_exactly() {
        let scale = 10u128.pow(6);

        assert_eq!(
            scale_amount(1_000_000_000_000_000_000, scale).unwrap(),
            1_000_000
        );
    }

    #[test]
    fn scale_amount_upscales_with_a_spread() {
        let scale = 10u128.pow(30) / 100 * 99;

        assert_eq!(
            scale_amount(1_000_000, scale).unwrap(),
            990_000_000_000_000_000
        );
    }

    /// Rounding is toward the user, because the written value is the solver's
    /// delivery floor.
    #[test]
    fn scale_amount_rounds_up() {
        // 1 unit at 0.5x would be 0.5; the user must not lose it.
        assert_eq!(scale_amount(1, WAD / 2).unwrap(), 1);
        assert_eq!(scale_amount(3, WAD / 2).unwrap(), 2);
    }

    /// Ceil rounding is what makes a zero obligation unreachable, which is why
    /// there is no explicit zero-obligation check anywhere in the program.
    #[test]
    fn scale_amount_never_returns_zero_for_a_nonzero_input() {
        [1u64, 2, 7, 1_000_000, u64::MAX]
            .into_iter()
            .for_each(|amount_in| {
                [1u128, 2, WAD / 1_000_000, WAD, 10u128.pow(30)]
                    .into_iter()
                    .for_each(|scale| {
                        assert!(scale_amount(amount_in, scale).unwrap_or(1) >= 1);
                    });
            });
    }

    #[test]
    fn scale_amount_rejects_a_zero_scale() {
        assert!(scale_amount(1_000_000, 0).is_err());
    }

    #[test]
    fn scale_amount_reports_overflow_rather_than_wrapping() {
        // A scale coprime with WAD and large enough that reduction cannot help.
        let scale = u128::MAX / 2;

        assert!(scale_amount(u64::MAX, scale).is_err());
    }

    // ---------- commitment ----------

    /// The heap is the binding ceiling on route length (see [`MAX_ROUTE_LEN`]), and
    /// the order — segments included — is streamed through it whole to derive the
    /// commitment. `borsh::to_vec(&order)` would materialise a second full copy on
    /// an allocator that never frees; [`KeccakWriter`] is what avoids it.
    ///
    /// Asserted on `hash()` itself, not on the serialization path underneath it.
    /// An earlier version of this test measured `serialize` directly and passed
    /// while `hash` still called `borsh::to_vec` — it was guarding a path
    /// production did not take. Exactly zero is the right bar, and it is the bar
    /// portal's equivalent holds: a reintroduced `to_vec` shows up as one.
    #[test]
    fn order_hash_does_not_allocate() {
        let order = order(vec![vec![7u8; MAX_ROUTE_LEN]], vec![]);
        let _ = order.hash(); // warm up any lazy init before counting

        crate::test_alloc::start_counting();
        let _ = order.hash();

        assert_eq!(
            crate::test_alloc::stop_counting(),
            0,
            "the order must stream into the hasher without being copied"
        );
    }

    #[test]
    fn order_hash_is_deterministic() {
        let a = order(vec![vec![1]], vec![]);
        let b = order(vec![vec![1]], vec![]);

        assert_eq!(a.hash(), b.hash());
        goldie::assert_json!(a.hash().as_ref());
    }

    #[test]
    fn order_hash_changes_with_every_field() {
        let base = order(vec![vec![1]], vec![]);
        let baseline = base.hash();

        let mut segments = order(vec![vec![2]], vec![]);
        assert_ne!(segments.hash(), baseline);

        segments = order(vec![vec![1]], vec![]);
        segments.destination += 1;
        assert_ne!(segments.hash(), baseline);

        let mut scale = order(vec![vec![1]], vec![]);
        scale.scale += 1;
        assert_ne!(scale.hash(), baseline);

        let mut floor = order(vec![vec![1]], vec![]);
        floor.min_amount_in += 1;
        assert_ne!(floor.hash(), baseline);

        let mut mint = order(vec![vec![1]], vec![]);
        mint.base_mint = Pubkey::new_from_array([9u8; 32]);
        assert_ne!(mint.hash(), baseline);

        let mut reward = order(vec![vec![1]], vec![]);
        reward.reward.creator = Pubkey::new_from_array([9u8; 32]);
        assert_ne!(reward.hash(), baseline);
    }

    /// The log budget is no longer the binding ceiling (see [`MAX_ROUTE_LEN`]),
    /// but it is the one that fails *silently*, so keep asserting the cap sits
    /// inside it. The integration suite additionally proves the event is present
    /// and byte-exact at the cap.
    #[test]
    fn route_len_bounds_are_within_the_log_budget() {
        const LOG_MESSAGES_BYTES_LIMIT: usize = 10 * 1000;

        // `IntentPublished { intent_hash, destination, route, reward }` plus the
        // 8-byte discriminator and Borsh length prefixes, generously over-counted.
        let event_overhead = 8 + 32 + 8 + 4 + 200;
        let base64 = (MAX_ROUTE_LEN + event_overhead).div_ceil(3) * 4;
        // "Program data: " prefix and the surrounding log lines.
        let headroom = 512;

        assert!(
            base64 + headroom < LOG_MESSAGES_BYTES_LIMIT,
            "MAX_ROUTE_LEN={MAX_ROUTE_LEN} expands to {base64} base64 bytes, \
             which does not fit the {LOG_MESSAGES_BYTES_LIMIT}-byte log budget"
        );
    }
}

/// Cross-VM agreement with the EVM `IntentChainer`.
///
/// The two ports owe each other agreement on the **bytes they produce**, not on
/// the shape of the input that produces them — and nothing else in either suite
/// checks that. This is the one failure mode that would otherwise surface as a
/// solver delivering the wrong amount on a live lane: a transform mismatch, an
/// endianness flip, or ceil landing on the wrong side would all pass every
/// same-side test and still disagree across the boundary.
///
/// The fixture is not hand-written. It was captured from
/// `eco-routes@e354167 test/chain/IntentChainerBorsh.t.sol`, whose segments are
/// cut from the route the **production** encoder
/// (`DepositAddress_USDCTransfer_Solana`) emits, and whose expectation is the
/// route the EVM `IntentChainer` published for `amount_in = 2_500_000` at
/// `scale = 0.96e18`. Both sides therefore splice the same literal segments and
/// must land on the same bytes.
#[cfg(test)]
mod cross_vm_tests {
    use portal::types::TokenAmount;

    use super::*;

    /// The shared fixture, checked into **both** repos so each side asserts the
    /// same bytes. See the `$comment` block in the file itself for the contract.
    const VECTORS: &str = include_str!("../testdata/cross-vm-vectors.json");

    /// Minimal field lift out of the fixture. A JSON dependency is not worth
    /// adding to an on-chain crate for one test file, and the shape is fixed.
    fn vector_field<'a>(vector: &'a str, key: &str) -> &'a str {
        let at = vector
            .find(&format!("\"{key}\""))
            .unwrap_or_else(|| panic!("fixture is missing {key}"));
        let rest = &vector[at + key.len() + 2..];
        let open = rest.find('"').expect("value must be a JSON string");
        let close = rest[open + 1..].find('"').expect("unterminated value");

        &rest[open + 1..open + 1 + close]
    }

    fn hex_to_bytes(hex: &str) -> Vec<u8> {
        (0..hex.len())
            .step_by(2)
            .map(|i| u8::from_str_radix(&hex[i..i + 2], 16).unwrap())
            .collect()
    }

    /// Every hex string inside the vector's `segments` array.
    fn segments(vector: &str) -> Vec<Vec<u8>> {
        let start = vector
            .find("\"segments\"")
            .expect("fixture is missing segments");
        let body = &vector[start..vector[start..].find(']').unwrap() + start];

        body.match_indices('"')
            .map(|(at, _)| at)
            .collect::<Vec<_>>()
            .chunks(2)
            .filter_map(|pair| match pair {
                [open, close] => Some(&body[open + 1..*close]),
                _ => None,
            })
            .filter(|candidate| {
                candidate.len() > 16 && candidate.chars().all(|c| c.is_ascii_hexdigit())
            })
            .map(hex_to_bytes)
            .collect()
    }

    fn fixture_order(vector: &str) -> Order {
        let segments = segments(vector);
        let slots = (0..segments.len() - 1)
            .map(|_| Slot {
                width: 8,
                little_endian: true,
            })
            .collect();

        Order {
            base_mint: Pubkey::new_from_array([3u8; 32]),
            destination: 1399811150,
            segments,
            slots,
            reward: Reward {
                deadline: 1_700_000_000,
                creator: Pubkey::new_from_array([1u8; 32]),
                prover: Pubkey::new_from_array([2u8; 32]),
                native_amount: 0,
                tokens: vec![TokenAmount {
                    token: Pubkey::new_from_array([3u8; 32]),
                    amount: 0,
                }],
            },
            scale: vector_field(vector, "scale").parse().unwrap(),
            min_amount_in: 1,
            require_publish: false,
        }
    }

    /// The transform must agree across the boundary before the bytes can.
    #[test]
    fn scale_agrees_with_the_evm_chainer() {
        let amount_in: u64 = vector_field(VECTORS, "amount_in").parse().unwrap();
        let scale: u128 = vector_field(VECTORS, "scale").parse().unwrap();
        let expected: u128 = vector_field(VECTORS, "expected_amount_out")
            .parse()
            .unwrap();

        assert_eq!(
            scale_amount(amount_in, scale).unwrap(),
            expected,
            "ceil(amount_in * scale / WAD) must match the EVM contract"
        );
    }

    /// The whole point: same segments, same scalar, same bytes.
    #[test]
    fn splice_reproduces_the_evm_chainers_route_byte_for_byte() {
        let amount_in: u64 = vector_field(VECTORS, "amount_in").parse().unwrap();
        let order = fixture_order(VECTORS);
        let amount_out = scale_amount(amount_in, order.scale).unwrap();

        assert_eq!(
            order.build_route(amount_out).unwrap(),
            hex_to_bytes(vector_field(VECTORS, "expected_route")),
            "the SVM splice must reproduce the EVM chainer's route exactly"
        );
    }

    /// A guard on the fixture itself: if the EVM shape ever moves, this fails with
    /// a readable message rather than the byte comparison failing opaquely.
    #[test]
    fn fixture_carries_the_amount_at_both_declared_offsets() {
        const TOKENS_AMOUNT_OFFSET: usize = 116;
        const SPL_AMOUNT_OFFSET: usize = 169;

        let expected: u64 = vector_field(VECTORS, "expected_amount_out")
            .parse()
            .unwrap();
        let route = hex_to_bytes(vector_field(VECTORS, "expected_route"));
        let read_u64_le = |at: usize| u64::from_le_bytes(route[at..at + 8].try_into().unwrap());

        assert_eq!(read_u64_le(TOKENS_AMOUNT_OFFSET), expected);
        assert_eq!(read_u64_le(SPL_AMOUNT_OFFSET), expected);
    }

    /// The WAD the fixture declares must be the one this crate uses, or every
    /// vector in the file silently means something else.
    #[test]
    fn fixture_wad_matches_this_crate() {
        assert_eq!(vector_field(VECTORS, "wad").parse::<u128>().unwrap(), WAD);
    }
}
