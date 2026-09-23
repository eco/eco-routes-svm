use std::io::{self, Read, Write};

use anchor_lang::prelude::*;
use eco_svm_std::Bytes32;
use portal::types::Reward;
use tiny_keccak::{Hasher, Keccak};

use crate::instructions::ChainerError;
use crate::keccak_writer::KeccakWriter;
pub use crate::template::*;

/// Decimal fixed-point denominator shared with the EVM template implementation.
pub const WAD: u128 = 1_000_000_000_000_000_000;

/// Canonical Borsh commitment preimage. Every nested field is serialized here.
/// The ABI change intentionally changes order hashes and escrow addresses.
///
/// Remote vault recipients belong only to `template` data. Local custody remains
/// the order-specific escrow followed by the vault under the committed `portal`.
#[derive(AnchorSerialize, Clone, Debug)]
pub struct Order {
    /// Local reward Portal, committed rather than caller-selected.
    pub portal: Pubkey,
    pub base_mint: Pubkey,
    /// Root route's execution chain.
    pub destination: u64,
    pub template: TemplateProgram,
    /// Local reward: exactly one base-mint token, authored amount zero, native zero.
    pub reward: Reward,
    /// Initial Output = ceil(Input * scale / WAD), in the u128 domain.
    pub scale: u128,
    pub min_amount_in: u64,
    /// Callers can strengthen this with publish=true, never weaken it.
    pub require_publish: bool,
}

impl Order {
    pub fn hash(&self) -> Bytes32 {
        let mut hasher = Keccak::v256();
        let mut hash = [0; 32];
        self.serialize(&mut KeccakWriter::new(&mut hasher))
            .expect("Order borsh serialization is infallible");
        hasher.finalize(&mut hash);
        hash.into()
    }

    /// No allocation or hashing, shared by announce and chain.
    pub fn validate_template(&self) -> Result<()> {
        self.template.validate()?;
        require!(
            self.encoded_len()? <= MAX_ORDER_BYTES,
            ChainerError::OrderTooLarge
        );
        Ok(())
    }

    /// Exact Borsh size without materializing another copy of the preimage.
    pub fn encoded_len(&self) -> Result<usize> {
        let mut counter = SizeCounter(0);
        self.serialize(&mut counter)?;
        Ok(counter.0)
    }

    /// Client helper. The handler independently measures and creates this same
    /// context on-chain before validating the caller's LOCAL accounts.
    pub fn build_route(&self, amount_in: u64) -> Result<Vec<u8>> {
        self.validate_template()?;
        self.template.render(AmountContext {
            input: amount_in,
            output: scale_amount(amount_in as u128, self.scale)?,
        })
    }
}

struct SizeCounter(usize);
impl Write for SizeCounter {
    fn write(&mut self, bytes: &[u8]) -> io::Result<usize> {
        self.0 = self
            .0
            .checked_add(bytes.len())
            .ok_or_else(|| io::Error::new(io::ErrorKind::InvalidData, "order too large"))?;
        Ok(bytes.len())
    }
    fn flush(&mut self) -> io::Result<()> {
        Ok(())
    }
}

/// Cap encoded input while parsing, not just after allocating/hashing the order.
/// Template vector lengths are separately bounded before allocation.
impl AnchorDeserialize for Order {
    fn deserialize_reader<R: Read>(reader: &mut R) -> io::Result<Self> {
        let reader = &mut reader.take(MAX_ORDER_BYTES as u64);
        Ok(Self {
            portal: Pubkey::deserialize_reader(reader)?,
            base_mint: Pubkey::deserialize_reader(reader)?,
            destination: u64::deserialize_reader(reader)?,
            template: TemplateProgram::deserialize_reader(reader)?,
            reward: deserialize_reward(reader)?,
            scale: u128::deserialize_reader(reader)?,
            min_amount_in: u64::deserialize_reader(reader)?,
            require_publish: bool::deserialize_reader(reader)?,
        })
    }
}

fn deserialize_reward(reader: &mut impl Read) -> io::Result<Reward> {
    let deadline = u64::deserialize_reader(reader)?;
    let creator = Pubkey::deserialize_reader(reader)?;
    let prover = Pubkey::deserialize_reader(reader)?;
    let native_amount = u64::deserialize_reader(reader)?;
    let len = u32::deserialize_reader(reader)?;
    // Keep malformed zero/two-leg orders diagnosable by the handler, while
    // rejecting attacker-controlled huge allocations before reading token data.
    if len > 2 {
        return Err(io::Error::new(
            io::ErrorKind::InvalidData,
            "too many reward tokens",
        ));
    }
    let mut tokens = Vec::with_capacity(len as usize);
    for _ in 0..len {
        tokens.push(portal::types::TokenAmount::deserialize_reader(reader)?);
    }
    Ok(Reward {
        deadline,
        creator,
        prover,
        native_amount,
        tokens,
    })
}

/// Exact ceil(amount * scale / WAD) with a 256-bit product and a u128 result.
/// Unlike EVM uint256, both scale and the quotient are limited to u128; a result
/// outside that domain fails, even when a 32-byte item could physically encode it.
/// No heap, no intermediate narrowing, and no false overflow of a fitting result.
pub fn scale_amount(amount: u128, scale: u128) -> Result<u128> {
    require!(scale > 0, ChainerError::InvalidScale);
    let a = [amount as u64 as u128, amount >> 64];
    let b = [scale as u64 as u128, scale >> 64];
    let lo = a[0] * b[0];
    let cross0 = a[0] * b[1];
    let cross1 = a[1] * b[0];
    let mid = (lo >> 64) + (cross0 as u64 as u128) + (cross1 as u64 as u128);
    let hi = a[1] * b[1] + (cross0 >> 64) + (cross1 >> 64) + (mid >> 64);
    let limbs = [lo as u64, mid as u64, hi as u64, (hi >> 64) as u64];
    let mut quotient = [0u64; 4];
    let mut remainder = 0u128;
    for i in (0..4).rev() {
        let dividend = (remainder << 64) | limbs[i] as u128;
        quotient[i] = (dividend / WAD) as u64;
        remainder = dividend % WAD;
    }
    require!(
        quotient[2] == 0 && quotient[3] == 0,
        ChainerError::ScaleOverflow
    );
    let value = ((quotient[1] as u128) << 64) | quotient[0] as u128;
    value
        .checked_add(u128::from(remainder != 0))
        .ok_or_else(|| ChainerError::ScaleOverflow.into())
}

#[cfg(test)]
mod tests {
    use portal::types::TokenAmount;

    use super::*;

    #[test]
    fn successful_scaling_does_not_allocate_an_unused_anchor_error() {
        crate::test_alloc::start_counting();
        let result = scale_amount(u128::MAX, WAD);
        let allocations = crate::test_alloc::stop_counting();
        assert_eq!(result.unwrap(), u128::MAX);
        assert_eq!(allocations, 0);
    }

    fn order(segments: Vec<Vec<u8>>, amounts: Vec<Amount>) -> Order {
        Order {
            portal: portal::ID,
            base_mint: Pubkey::new_from_array([3u8; 32]),
            destination: 1399811150,
            template: TemplateProgram {
                vaults: vec![],
                route: Template {
                    segments,
                    items: amounts.into_iter().map(Item::Amount).collect(),
                },
            },
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

    fn encode_amount_into(out: &mut Vec<u8>, value: u128, amount: &Amount) -> Result<()> {
        out.extend_from_slice(&encode_amount(value, amount)?[..amount.width as usize]);
        Ok(())
    }

    // ---------- splice ----------

    #[test]
    fn build_route_splices_a_little_endian_u64() {
        let built = order(
            vec![vec![0xAA, 0xBB], vec![0xCC]],
            vec![Amount {
                source: AmountSource::Output,
                scale: WAD,
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
            vec![Amount {
                source: AmountSource::Output,
                scale: WAD,
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
                Amount {
                    source: AmountSource::Output,
                    scale: WAD,
                    width: 8,
                    little_endian: true,
                },
                Amount {
                    source: AmountSource::Output,
                    scale: WAD,
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
    fn build_route_with_no_items_is_the_single_segment() {
        let built = order(vec![vec![1, 2, 3]], vec![]).build_route(999).unwrap();

        assert_eq!(built, vec![1, 2, 3]);
    }

    #[test]
    fn build_route_rejects_a_segment_count_mismatch() {
        let result = order(
            vec![vec![0xAA]],
            vec![Amount {
                source: AmountSource::Output,
                scale: WAD,
                width: 8,
                little_endian: true,
            }],
        )
        .build_route(1);

        assert!(result.is_err());
    }

    #[test]
    fn build_route_rejects_too_many_items() {
        let amounts: Vec<_> = (0..MAX_ITEMS + 1)
            .map(|_| Amount {
                source: AmountSource::Output,
                scale: WAD,
                width: 8,
                little_endian: true,
            })
            .collect();
        let segments = vec![vec![]; MAX_ITEMS + 2];

        assert!(order(segments, amounts).build_route(1).is_err());
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
        let slot = Amount {
            source: AmountSource::Output,
            scale: WAD,
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
            &Amount {
                source: AmountSource::Output,
                scale: WAD,
                width: 0,
                little_endian: true
            }
        )
        .is_err());
        assert!(encode_amount_into(
            &mut out,
            1,
            &Amount {
                source: AmountSource::Output,
                scale: WAD,
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
                &Amount {
                    source: AmountSource::Output,
                    scale: WAD,
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
    /// ceiling of `~3.4e38`. A full-width product keeps the fitting quotient exact.
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
                        assert!(
                            scale_amount(amount_in as u128, scale).expect(
                                "all selected input/scale pairs have a fitting u128 quotient"
                            ) >= 1
                        );
                    });
            });
    }

    #[test]
    fn scale_amount_rejects_a_zero_scale() {
        assert!(scale_amount(1_000_000, 0).is_err());
    }

    #[test]
    fn scale_amount_reports_overflow_rather_than_wrapping() {
        // The quotient itself exceeds u128, even with full-width intermediates.
        let scale = u128::MAX / 2;

        assert!(scale_amount(u64::MAX as u128, scale).is_err());
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
/// Preserves the original amount-only rendered-output golden alongside the new
/// nested-template corpus. The Order ABI/commitment intentionally differs.
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
    use serde_json::Value;

    use super::*;

    /// The shared fixture, checked into **both** repos so each side asserts the
    /// same bytes. See the `$comment` block in the file itself for the contract.
    fn fixture() -> Value {
        serde_json::from_str(include_str!("../testdata/cross-vm-vectors.json")).unwrap()
    }

    fn vector_field<'a>(vector: &'a Value, key: &str) -> &'a str {
        vector[key].as_str().unwrap()
    }

    fn hex_to_bytes(hex: &str) -> Vec<u8> {
        (0..hex.len())
            .step_by(2)
            .map(|i| u8::from_str_radix(&hex[i..i + 2], 16).unwrap())
            .collect()
    }

    fn fixture_order(vector: &Value) -> Order {
        let segments = vector["segments"]
            .as_array()
            .unwrap()
            .iter()
            .map(|s| hex_to_bytes(s.as_str().unwrap()))
            .collect();
        // The historical fixture calls these "slots". Preserve its bytes while
        // adapting each entry to an Output item with an explicit identity scale.
        let items = vector["slots"]
            .as_array()
            .unwrap()
            .iter()
            .map(|a| {
                Item::Amount(Amount::output(
                    a["width"].as_u64().unwrap().try_into().unwrap(),
                    a["little_endian"].as_bool().unwrap(),
                ))
            })
            .collect();

        Order {
            portal: portal::ID,
            base_mint: Pubkey::new_from_array([3u8; 32]),
            destination: 1399811150,
            template: TemplateProgram {
                vaults: vec![],
                route: Template { segments, items },
            },
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
        for vector in fixture()["vectors"].as_array().unwrap() {
            let amount_in: u64 = vector_field(vector, "amount_in").parse().unwrap();
            let scale: u128 = vector_field(vector, "scale").parse().unwrap();
            let expected: u128 = vector_field(vector, "expected_amount_out").parse().unwrap();

            assert_eq!(
                scale_amount(amount_in as u128, scale).unwrap(),
                expected,
                "ceil(amount_in * scale / WAD) must match the EVM contract"
            );
        }
    }

    /// The whole point: same segments, same scalar, same bytes.
    #[test]
    fn splice_reproduces_the_evm_chainers_route_byte_for_byte() {
        for vector in fixture()["vectors"].as_array().unwrap() {
            let amount_in: u64 = vector_field(vector, "amount_in").parse().unwrap();
            let order = fixture_order(vector);

            assert_eq!(
                order.build_route(amount_in).unwrap(),
                hex_to_bytes(vector_field(vector, "expected_route")),
                "the SVM splice must reproduce the EVM chainer's route exactly"
            );
        }
    }

    /// A guard on the fixture itself: if the EVM shape ever moves, this fails with
    /// a readable message rather than the byte comparison failing opaquely.
    #[test]
    fn fixture_carries_the_amount_at_both_declared_offsets() {
        for vector in fixture()["vectors"].as_array().unwrap() {
            let expected: u64 = vector_field(vector, "expected_amount_out").parse().unwrap();
            let route = hex_to_bytes(vector_field(vector, "expected_route"));
            let read_u64_le = |at: usize| u64::from_le_bytes(route[at..at + 8].try_into().unwrap());

            let offsets = vector["slot_offsets_in_expected_route"].as_array().unwrap();
            assert_eq!(offsets.len(), 2);
            for offset in offsets {
                assert_eq!(
                    read_u64_le(offset.as_u64().unwrap().try_into().unwrap()),
                    expected
                );
            }
        }
    }

    /// The WAD the fixture declares must be the one this crate uses, or every
    /// vector in the file silently means something else.
    #[test]
    fn fixture_wad_matches_this_crate() {
        assert_eq!(
            vector_field(&fixture(), "wad").parse::<u128>().unwrap(),
            WAD
        );
    }
}
