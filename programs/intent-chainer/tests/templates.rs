use anchor_lang::prelude::*;
use intent_chainer::state::escrow_authority_pda;
use intent_chainer::types::*;
use portal::types::{Reward, TokenAmount};
use tiny_keccak::{Hasher, Keccak};

#[path = "../testdata/fixture_parser.rs"]
mod fixtures;
use fixtures::{bytes, hex};

fn encode(value: &impl AnchorSerialize) -> Vec<u8> {
    let mut out = vec![];
    value.serialize(&mut out).unwrap();
    out
}
fn selected_template(o: &mut Order, index: usize) -> &mut Template {
    if index == o.template.vaults.len() * 2 {
        &mut o.template.route
    } else if index.is_multiple_of(2) {
        &mut o.template.vaults[index / 2].route
    } else {
        &mut o.template.vaults[index / 2].reward
    }
}
fn context() -> AmountContext {
    AmountContext {
        input: 515,
        output: 1030,
    }
}
fn evm() -> Derivation {
    Derivation::Evm(EvmDerivation {
        portal: [1; 20],
        prefix: 255,
        implementation: [2; 20],
        init_code_hash: [3; 32],
    })
}
fn node() -> Vault {
    Vault {
        destination: 480,
        route: Template::literal(vec![1]),
        reward: Template::literal(vec![2]),
        derivation: evm(),
    }
}
fn single(item: Item) -> Template {
    Template {
        segments: vec![vec![], vec![]],
        items: vec![item],
    }
}
fn p(route: Template) -> TemplateProgram {
    TemplateProgram {
        vaults: vec![],
        route,
    }
}
fn hash(data: &[u8]) -> [u8; 32] {
    let mut h = Keccak::v256();
    let mut out = [0; 32];
    h.update(data);
    h.finalize(&mut out);
    out
}
fn order(template: TemplateProgram) -> Order {
    Order {
        portal: portal::ID,
        base_mint: Pubkey::new_from_array([3; 32]),
        destination: 8453,
        template,
        reward: Reward {
            deadline: 2_000_000_000,
            creator: Pubkey::new_from_array([4; 32]),
            prover: Pubkey::new_from_array([5; 32]),
            native_amount: 0,
            tokens: vec![TokenAmount {
                token: Pubkey::new_from_array([3; 32]),
                amount: 0,
            }],
        },
        scale: WAD,
        min_amount_in: 1,
        require_publish: true,
    }
}

#[test]
fn pinned_evm_harness_agrees_on_routes_full_rewards_hashes_and_recipients() {
    for case in fixtures::nested()["cases"].as_array().unwrap() {
        let program = fixtures::program(&case["program"]);
        let context = AmountContext {
            input: case["input"].as_str().unwrap().parse().unwrap(),
            output: case["output"].as_str().unwrap().parse().unwrap(),
        };
        assert_eq!(
            program.render(context).unwrap(),
            hex(case["expected_route"].as_str().unwrap()),
            "{}",
            case["name"]
        );
        for (i, node) in program.vaults.iter().enumerate() {
            let render = |template: &Template| {
                TemplateProgram {
                    vaults: program.vaults[..i].to_vec(),
                    route: template.clone(),
                }
                .render(context)
                .unwrap()
            };
            let route = render(&node.route);
            let reward = render(&node.reward);
            let expected = &case["nodes"][i];
            assert_eq!(route, hex(expected["route"].as_str().unwrap()));
            assert_eq!(reward, hex(expected["reward"].as_str().unwrap()));
            let intent_hash = portal::types::intent_hash(
                node.destination,
                &hash(&route).into(),
                &hash(&reward).into(),
            );
            assert_eq!(intent_hash.as_ref(), bytes::<32>(&expected["hash"]));
            assert_eq!(
                node.derivation.recipient(&intent_hash).unwrap(),
                bytes::<32>(&expected["recipient"])
            );
        }
    }
}

#[test]
fn changing_measurement_changes_both_nested_templates_hash_and_recipient() {
    let data = fixtures::nested();
    for pair in data["cases"].as_array().unwrap()[..8].chunks(2) {
        assert_eq!(pair[0]["program"], pair[1]["program"]);
        for field in ["route", "reward", "hash", "recipient"] {
            assert_ne!(
                pair[0]["nodes"][0][field], pair[1]["nodes"][0][field],
                "{field}"
            );
        }
    }
}

#[test]
fn solana_portal_golden_and_all_128_sdk_vectors_choose_canonical_bumps() {
    let data = fixtures::solana();
    let config = SolanaDerivation {
        portal: Pubkey::new_from_array(bytes(&data["portal"])),
        token_program: Pubkey::new_from_array(bytes(&data["tokenProgram"])),
        mint: Pubkey::new_from_array(bytes(&data["mint"])),
    };
    for v in std::iter::once(&data["golden"])
        .chain([&data["borsh1000"], &data["borsh1001"]])
        .chain(data["vectors"].as_array().unwrap())
    {
        let h = bytes::<32>(&v["intentHash"]);
        let (vault, bump) = Pubkey::find_program_address(&[b"vault", &h], &config.portal);
        assert_eq!(vault.to_bytes(), bytes::<32>(&v["vault"]));
        assert_eq!(bump as u64, v["vaultBump"].as_u64().unwrap());
        let (ata, bump) = Pubkey::find_program_address(
            &[
                vault.as_ref(),
                config.token_program.as_ref(),
                config.mint.as_ref(),
            ],
            &anchor_spl::associated_token::ID,
        );
        assert_eq!(ata.to_bytes(), bytes::<32>(&v["ata"]));
        assert_eq!(bump as u64, v["ataBump"].as_u64().unwrap());
        assert_eq!(
            Derivation::Solana(config.clone()).recipient(&h).unwrap(),
            ata.to_bytes()
        );
    }
    let h = bytes::<32>(&data["golden"]["intentHash"]);
    let noncanonical =
        Pubkey::create_program_address(&[b"vault", &h, &[254]], &config.portal).unwrap();
    assert_ne!(
        noncanonical.to_bytes(),
        bytes::<32>(&data["golden"]["vault"])
    );
    // A caller cannot serialize a bump into this schema, even if it is off-curve.
    let mut encoded = encode(&Derivation::Solana(config));
    encoded.extend_from_slice(&[254, 255]);
    assert!(Derivation::try_from_slice(&encoded).is_err());
}

#[test]
fn pinned_borsh_reward_fixture_uses_the_entire_portal_serialization() {
    let data = fixtures::solana();
    let mint = Pubkey::new_from_array(bytes(&data["mint"]));
    let mut creator = [0; 32];
    creator[31] = 3;
    let mut prover = [0; 32];
    prover[31] = 7;
    let reward = Reward {
        deadline: 2_000_000_000,
        creator: Pubkey::new_from_array(creator),
        prover: Pubkey::new_from_array(prover),
        native_amount: 0,
        tokens: vec![TokenAmount {
            token: mint,
            amount: 0,
        }],
    };
    let mut prefix = encode(&reward);
    prefix.truncate(prefix.len() - 8);
    let program = TemplateProgram {
        vaults: vec![Vault {
            destination: 1000,
            route: Template::literal(vec![1, 2, 3]),
            reward: Template {
                segments: vec![prefix, vec![]],
                items: vec![Item::Amount(Amount::output(8, true))],
            },
            derivation: Derivation::Solana(SolanaDerivation {
                portal: Pubkey::new_from_array(bytes(&data["portal"])),
                token_program: Pubkey::new_from_array(bytes(&data["tokenProgram"])),
                mint,
            }),
        }],
        route: single(Item::Vault(0)),
    };
    for amount in [1000, 1001] {
        assert_eq!(
            program
                .render(AmountContext {
                    input: amount,
                    output: amount as u128
                })
                .unwrap(),
            bytes::<32>(&data[format!("borsh{amount}")]["ata"])
        );
    }
}

#[test]
fn fixed_initial_context_positive_scale_width_and_ceil() {
    let input = Item::Amount(Amount {
        source: AmountSource::Input,
        scale: WAD / 2,
        width: 8,
        little_endian: true,
    });
    assert_eq!(
        p(single(input)).render(context()).unwrap(),
        hex("0201000000000000")
    );
    let config = Amount {
        source: AmountSource::Output,
        scale: WAD / 1_000_000_000_000,
        width: 8,
        little_endian: true,
    };
    let output = 1_000_000_000_000_000_000_000_001;
    assert!(output > u64::MAX as u128);
    assert_eq!(
        p(single(Item::Amount(config)))
            .render(AmountContext { input: 1, output })
            .unwrap(),
        1_000_000_000_001u64.to_le_bytes()
    );
    for width in 1..=32 {
        for little_endian in [false, true] {
            let config = Amount::output(width, little_endian);
            let max = if width < 16 {
                (1u128 << (width * 8)) - 1
            } else {
                u128::MAX
            };
            for value in [0, max, max & 0x0123456789abcdef_fedcba9876543210] {
                // Independent native integer encoding checks EVERY byte, including
                // padding and asymmetric values, not just the count of 0xff bytes.
                let mut expected = vec![0; width as usize];
                let significant = width.min(16) as usize;
                if little_endian {
                    expected[..significant].copy_from_slice(&value.to_le_bytes()[..significant]);
                } else {
                    expected[width as usize - significant..]
                        .copy_from_slice(&value.to_be_bytes()[16 - significant..]);
                }
                let encoded = encode_amount(value, &config).unwrap();
                assert_eq!(&encoded[..width as usize], expected);
                assert!(encoded[width as usize..].iter().all(|byte| *byte == 0));
            }
            if width < 16 {
                assert!(encode_amount(max + 1, &config).is_err());
            }
        }
    }
    for config in [
        Amount::output(0, false),
        Amount::output(33, true),
        Amount {
            scale: 0,
            ..Amount::output(8, true)
        },
    ] {
        assert!(p(single(Item::Amount(config))).render(context()).is_err());
    }
}

#[test]
fn full_width_product_and_quotient_match_independent_bigint_vectors() {
    for v in fixtures::nested()["numeric"].as_array().unwrap() {
        let result = scale_amount(
            v["amount"].as_str().unwrap().parse().unwrap(),
            v["scale"].as_str().unwrap().parse().unwrap(),
        );
        if let Some(expected) = v["expected"].as_str() {
            assert_eq!(result.unwrap(), expected.parse::<u128>().unwrap(), "{v}");
        } else {
            assert!(result.is_err(), "{v}");
        }
    }
    assert_eq!(scale_amount(u128::MAX, WAD).unwrap(), u128::MAX);
    assert!(scale_amount(1, 0).is_err());
    assert!(p(single(Item::Amount(Amount {
        scale: u128::MAX,
        ..Amount::output(32, false)
    })))
    .render(AmountContext {
        input: 1,
        output: u128::MAX
    })
    .is_err());
    // A positive remainder when the floor already equals u128::MAX must fail.
    // Independent BigInt product: floor=u128::MAX, remainder=92240510829748332.
    assert!(scale_amount(340282366920938463123092240510829748332, WAD + 1).is_err());
}

#[test]
fn every_missing_remote_parameter_fails() {
    let Derivation::Evm(base) = evm() else {
        unreachable!()
    };
    for c in [
        EvmDerivation {
            portal: [0; 20],
            ..base.clone()
        },
        EvmDerivation {
            prefix: 0,
            ..base.clone()
        },
        EvmDerivation {
            implementation: [0; 20],
            ..base.clone()
        },
        EvmDerivation {
            init_code_hash: [0; 32],
            ..base
        },
    ] {
        assert!(Derivation::Evm(c).recipient(&[4; 32]).is_err());
    }
    let base = SolanaDerivation {
        portal: portal::ID,
        token_program: anchor_spl::token::ID,
        mint: Pubkey::new_from_array([3; 32]),
    };
    for c in [
        SolanaDerivation {
            portal: Pubkey::default(),
            ..base.clone()
        },
        SolanaDerivation {
            token_program: Pubkey::default(),
            ..base.clone()
        },
        SolanaDerivation {
            mint: Pubkey::default(),
            ..base
        },
    ] {
        assert!(Derivation::Solana(c).recipient(&[4; 32]).is_err());
    }
}

#[test]
fn geometry_dependencies_cycles_and_every_work_bound_are_rejected() {
    assert!(p(Template {
        segments: vec![],
        items: vec![]
    })
    .validate()
    .is_err());
    assert!(p(Template {
        segments: vec![vec![], vec![]],
        items: vec![]
    })
    .validate()
    .is_err());
    assert!(p(Template {
        segments: vec![vec![]; MAX_ITEMS + 2],
        items: vec![Item::Amount(Amount::output(1, true)); MAX_ITEMS + 1]
    })
    .validate()
    .is_err());
    assert!(p(Template::literal(vec![0; MAX_ROUTE_LEN + 1]))
        .validate()
        .is_err());
    for index in [0, 1, 255] {
        assert!(p(single(Item::Vault(index))).validate().is_err());
    }
    for index in [0, 1, 255] {
        for reward in [false, true] {
            let mut node = node();
            *if reward {
                &mut node.reward
            } else {
                &mut node.route
            } = single(Item::Vault(index));
            assert!(TemplateProgram {
                vaults: vec![node.clone(), node],
                route: Template::literal(vec![])
            }
            .validate()
            .is_err());
        }
    }
    assert!(TemplateProgram {
        vaults: vec![node(); MAX_VAULTS + 1],
        route: Template::literal(vec![])
    }
    .validate()
    .is_err());
    let mut program = TemplateProgram {
        vaults: vec![node()],
        route: Template::literal(vec![0; MAX_RENDERED_BYTES - 2]),
    };
    assert!(program.validate().is_ok());
    program.route.segments[0].push(0);
    assert!(program.validate().is_err());
    // Bound serialization independently of rendered size: small values with many items.
    let tiny = Template {
        segments: vec![vec![]; MAX_ITEMS + 1],
        items: vec![Item::Amount(Amount::output(1, true)); MAX_ITEMS],
    };
    let mut node = node();
    node.route = tiny.clone();
    node.reward = tiny.clone();
    let order = order(TemplateProgram {
        vaults: vec![node; MAX_VAULTS],
        route: tiny,
    });
    assert!(encode(&order).len() > MAX_ORDER_BYTES);
    assert!(order.validate_template().is_err());
    assert!(Order::try_from_slice(&encode(&order)).is_err());
}

#[test]
fn canonical_borsh_rejects_invalid_tags_trailing_bytes_truncation_and_allocation_bombs() {
    for tag in 2..=255 {
        assert!(Derivation::try_from_slice(&[tag]).is_err());
        assert!(Item::try_from_slice(&[tag]).is_err());
        assert!(AmountSource::try_from_slice(&[tag]).is_err());
    }
    for data in [
        encode(&evm()),
        encode(&Derivation::Solana(SolanaDerivation {
            portal: portal::ID,
            token_program: anchor_spl::token::ID,
            mint: portal::ID,
        })),
    ] {
        for len in 0..data.len() {
            assert!(Derivation::try_from_slice(&data[..len]).is_err());
        }
        let mut padded = data;
        padded.push(0);
        assert!(Derivation::try_from_slice(&padded).is_err());
    }
    assert!(TemplateProgram::try_from_slice(&u32::MAX.to_le_bytes()).is_err());
    assert!(Template::try_from_slice(&u32::MAX.to_le_bytes()).is_err());
    let mut segment_bomb = 1u32.to_le_bytes().to_vec();
    segment_bomb.extend_from_slice(&u32::MAX.to_le_bytes());
    assert!(Template::try_from_slice(&segment_bomb).is_err());
    let mut amount = encode(&Amount::output(8, true));
    *amount.last_mut().unwrap() = 2;
    assert!(Amount::try_from_slice(&amount).is_err());
}

#[test]
fn every_nested_field_changes_both_commitment_and_custody() {
    let base = order(fixtures::program(
        &fixtures::nested()["cases"][8]["program"],
    ));
    let baseline = base.hash();
    let escrow = escrow_authority_pda(&baseline).0;
    let check = |changed: Order| {
        assert_ne!(changed.hash(), baseline);
        assert_ne!(escrow_authority_pda(&changed.hash()).0, escrow);
        assert_eq!(changed.hash().as_ref(), hash(&encode(&changed)));
    };
    macro_rules! changed {
        ($field:ident, $value:expr) => {{
            let mut o = base.clone();
            o.$field = $value;
            check(o);
        }};
    }
    changed!(portal, Pubkey::new_unique());
    changed!(base_mint, Pubkey::new_unique());
    changed!(destination, base.destination + 1);
    changed!(scale, base.scale + 1);
    changed!(min_amount_in, 2);
    changed!(require_publish, false);
    for field in 0..6 {
        let mut o = base.clone();
        match field {
            0 => o.reward.deadline += 1,
            1 => o.reward.creator = Pubkey::new_unique(),
            2 => o.reward.prover = Pubkey::new_unique(),
            3 => o.reward.native_amount = 1,
            4 => o.reward.tokens[0].token = Pubkey::new_unique(),
            _ => o.reward.tokens[0].amount = 1,
        }
        check(o);
    }
    for node_index in 0..base.template.vaults.len() {
        let mut o = base.clone();
        o.template.vaults[node_index].destination += 1;
        check(o);
        for field in 0..4 {
            let mut o = base.clone();
            match &mut o.template.vaults[node_index].derivation {
                Derivation::Evm(c) => match field {
                    0 => c.portal[0] ^= 1,
                    1 => c.prefix ^= 1,
                    2 => c.implementation[0] ^= 1,
                    _ => c.init_code_hash[0] ^= 1,
                },
                Derivation::Solana(c) => match field {
                    0 => c.portal = Pubkey::new_unique(),
                    1 => c.token_program = Pubkey::new_unique(),
                    2 => c.mint = Pubkey::new_unique(),
                    _ => o.template.vaults[node_index].derivation = evm(),
                },
            }
            check(o);
        }
    }
    for template_index in 0..base.template.vaults.len() * 2 + 1 {
        let mut probe = base.clone();
        for i in 0..selected_template(&mut probe, template_index).segments.len() {
            let mut o = base.clone();
            selected_template(&mut o, template_index).segments[i].push(9);
            check(o);
        }
        for i in 0..selected_template(&mut probe, template_index).items.len() {
            for field in 0..5 {
                let mut o = base.clone();
                let item = &mut selected_template(&mut o, template_index).items[i];
                match item {
                    Item::Amount(a) => match field {
                        0 => {
                            a.source = match a.source {
                                AmountSource::Input => AmountSource::Output,
                                AmountSource::Output => AmountSource::Input,
                            }
                        }
                        1 => a.scale += 1,
                        2 => a.width ^= 1,
                        3 => a.little_endian = !a.little_endian,
                        _ => *item = Item::Vault(0),
                    },
                    Item::Vault(index) => *index ^= 1,
                }
                check(o);
            }
        }
    }
    let mut o = base.clone();
    o.template.vaults.swap(0, 1);
    check(o);
    let mut o = base.clone();
    o.template.vaults.push(node());
    check(o);
}
