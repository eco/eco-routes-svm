import { PublicKey } from "@solana/web3.js";

export const ECO_ROUTES_ID_TESTNET = new PublicKey("aEGzbWJhZ7RX8uCmeG4jVfskQe6eoP7zcdoHmY2PWys");
export const ECO_ROUTES_ID_MAINNET = new PublicKey("a6BKzp2ixm6ogEJ268UT4UGFMLnsgPWnVm93vsjupc3");
export const MAILBOX_ID_MAINNET = new PublicKey("E588QtVUvresuXq2KoNEwAmoifCzYGpRBdHByN9KQMbi");
export const MAILBOX_ID_TESTNET = new PublicKey("75HBBLae3ddeneJVrZeyrDfv6vb7SMC3aCpBucSXS5aR");
export const SPL_NOOP_ID = new PublicKey("noopb9bkMVfRPU8AsbpTUg8AQkHtKwMYZiFUjNRtMmV");
export const EVM_DOMAIN_ID = 10; // optimism mainnet domaind id
export const SOLANA_DOMAIN_ID = 1399811149; // solana mainnet domaind id
export const DEVNET_RPC = "https://api.devnet.solana.com";
export const MAINNET_RPC =
	"https://mainnet.helius-rpc.com/?api-key=b274219f-935d-42c1-8209-efdbf2115ec3";
export const TESTNET_RPC = "https://api.testnet.solana.com";
export const USDC_MINT = new PublicKey("2u1tszSeqZ3qBWF3uNGPFc8TzMk2tdiwknnRMWGWjGWH");
export const USDC_DECIMALS = 6;
export const SOLVER_PLACEHOLDER_PUBKEY = new PublicKey(
	"So1ver1111111111111111111111111111111111111"
);

// EVM testnet contracts on Sepolia
export const INTENT_SOURCE_ADDRESS_TESTNET =
	"0xf784eCE056cb95CD486C7fBef218AE1a7a5dE27d".toLowerCase();
export const INBOX_ADDRESS_TESTNET = "0xb5670A91Ab60c14231316b59f3c311A7Fd342eE8".toLowerCase();
export const STORAGE_PROVER_ADDRESS_TESTNET =
	"0x1947e422b769e0568b692096B06fd9C39E25150d".toLowerCase();
export const TEST_USDC_ADDRESS_TESTNET = "0x72A0CE0Da1E62BAF7FBB48ea347EB038836D091a".toLowerCase();

export const IGP_FEE_RECEIVER_TESTNET = new PublicKey(
	"9SQVtTNsbipdMzumhzi6X8GwojiSMwBfqAhS7FgyTcqy"
);
export const IGP_PROGRAM_ID_TESTNET = new PublicKey("5p7Hii6CJL4xGBYYTGEQmH9LnUSZteFJUu9AVLDExZX2");

// Solana mainnet IGP addresses
export const IGP_PROGRAM_ID = new PublicKey("BhNcatUDC2D5JTyeaqrdSukiVFsEHK7e3hVmKMztwefv");
export const IGP_PDA = new PublicKey("JAvHW21tYXE9dtdG83DReqU2b4LUexFuCbtJT5tF8X6M");
export const OVERHEAD_IGP = new PublicKey("AkeHBbE5JkwVppujCQQ6WuxsVsJtruBAjUo6fDCFp6fF");
export const DISPATCHED_MSG_PDA_HEADER_LEN: number = 52;

// EVM Optimism Addresses
export const INTENT_SOURCE_ADDRESS = "0x916BC960e3c1F9A06f25e970692b68dD7C0D3beE".toLowerCase();
export const INBOX_ADDRESS = "0xF5e916cF82a59Bc55013F6ADF2D65dba4bD886eE".toLowerCase();
export const HYPER_PROVER_ADDRESS = "0x641e9F48514516527AF84E5388DC3A8F656A9239".toLowerCase();
export const USDC_ADDRESS_MAINNET = "0x0b2C639c533813f4Aa9D7837CAf62653d097Ff85".toLowerCase();
