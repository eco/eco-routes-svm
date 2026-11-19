//! Test-only decoding of independently generated, pinned cross-VM fixtures.
use anchor_lang::prelude::Pubkey;
use intent_chainer::types::*;
use serde_json::Value;

pub fn hex(value: &str) -> Vec<u8> {
    let s = value.strip_prefix("0x").unwrap_or(value);
    (0..s.len())
        .step_by(2)
        .map(|i| u8::from_str_radix(&s[i..i + 2], 16).unwrap())
        .collect()
}

pub fn bytes<const N: usize>(value: &Value) -> [u8; N] {
    hex(value.as_str().unwrap()).try_into().unwrap()
}

pub fn template(value: &Value) -> Template {
    Template {
        segments: value["segments"]
            .as_array()
            .unwrap()
            .iter()
            .map(|v| hex(v.as_str().unwrap()))
            .collect(),
        items: value["items"]
            .as_array()
            .unwrap()
            .iter()
            .map(|v| {
                if let Some(index) = v.get("vault") {
                    return Item::Vault(index.as_u64().unwrap().try_into().unwrap());
                }
                let a = &v["amount"];
                Item::Amount(Amount {
                    source: match a["source"].as_str().unwrap() {
                        "Input" => AmountSource::Input,
                        "Output" => AmountSource::Output,
                        _ => panic!("unknown source"),
                    },
                    scale: a["scale"].as_str().unwrap().parse().unwrap(),
                    width: a["width"].as_u64().unwrap().try_into().unwrap(),
                    little_endian: a["little_endian"].as_bool().unwrap(),
                })
            })
            .collect(),
    }
}

pub fn derivation(value: &Value) -> Derivation {
    if let Some(c) = value.get("evm") {
        Derivation::Evm(EvmDerivation {
            portal: bytes(&c["portal"]),
            prefix: c["prefix"].as_u64().unwrap().try_into().unwrap(),
            implementation: bytes(&c["implementation"]),
            init_code_hash: bytes(&c["init_code_hash"]),
        })
    } else {
        let c = &value["solana"];
        Derivation::Solana(SolanaDerivation {
            portal: Pubkey::new_from_array(bytes(&c["portal"])),
            token_program: Pubkey::new_from_array(bytes(&c["token_program"])),
            mint: Pubkey::new_from_array(bytes(&c["mint"])),
        })
    }
}

pub fn program(value: &Value) -> TemplateProgram {
    TemplateProgram {
        vaults: value["vaults"]
            .as_array()
            .unwrap()
            .iter()
            .map(|v| Vault {
                destination: v["destination"].as_u64().unwrap(),
                route: template(&v["route"]),
                reward: template(&v["reward"]),
                derivation: derivation(&v["derivation"]),
            })
            .collect(),
        route: template(&value["route"]),
    }
}

pub fn nested() -> Value {
    serde_json::from_str(include_str!("nested-vectors.json")).unwrap()
}
pub fn solana() -> Value {
    serde_json::from_str(include_str!("solana-vault-vectors.json")).unwrap()
}
