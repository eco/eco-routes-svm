import { keccak_256 } from "@noble/hashes/sha3";
import { BN } from "@coral-xyz/anchor";
import { Reward, Route } from "./types";
import { Reward as EvmReward, Route as EvmRoute } from "./evmUtils";
import path from "path";
import { readFileSync } from "fs";
import {
  Keypair,
  PublicKey,
  SystemProgram,
  TransactionInstruction,
} from "@solana/web3.js";
import { homedir } from "os";
import * as anchor from "@coral-xyz/anchor";
import {
  EVM_DOMAIN_ID,
  IGP_PROGRAM_ID_TESTNET,
  USDC_DECIMALS,
} from "./constants";
import { ethers } from "ethers";
import { keccak_256 as keccak } from "@noble/hashes/sha3";
import { concatBytes } from "@noble/hashes/utils";
import { serialize } from "borsh";

const u32 = (number: number) => {
  const array = new Uint8Array(32);
  new DataView(array.buffer).setUint32(28, number, false);
  return array;
};

const bn = (bigNumber: BN) => {
  const array = new Uint8Array(32);
  const bytes = bigNumber.toArray("be", 32);
  array.set(bytes);
  return array;
};

const hexToBytes = (hex: string) => ethers.getBytes(hex); // 0x hex to Uint8Array

// Concatenate buffers and hash with Keccak-256
export function generateIntentHash(route: Route, reward: Reward): Uint8Array {
  const parts: Uint8Array[] = [
    route.salt,
    u32(route.sourceDomainId),
    u32(route.destinationDomainId),
    route.inbox,

    ...route.tokens.flatMap((tokenAmount) => [
      hexToBytes(tokenAmount.token),
      bn(tokenAmount.amount),
    ]),
    ...route.calls,

    reward.creator,
    reward.prover,
    ...reward.tokens.flatMap((tokenAmount) => [
      hexToBytes(tokenAmount.token),
      bn(tokenAmount.amount),
    ]),
    bn(reward.nativeAmount),
    bn(reward.deadline),
  ];

  const total = parts.reduce((n, p) => n + p.length, 0);
  const buffer = new Uint8Array(total);
  let off = 0;
  for (const part of parts) {
    buffer.set(part, off);
    off += part.length;
  }

  return keccak_256(buffer);
}

const u256be = (number: bigint | number) => {
  const buffer = new Uint8Array(32);
  const bn = BigInt(number);
  for (let i = 0; i < 32; ++i) {
    buffer[31 - i] = Number((bn >> BigInt(i * 8)) & BigInt(0xff));
  }
  return buffer;
};

/** faithful port of `hash_route()` in on-chain encoding.rs */
function hashRoute(route: EvmRoute): Uint8Array {
  const tokenAmounts = route.tokens;
  const calls = route.calls;
  const tokenArraySize = 32 + tokenAmounts.length * 64;
  const callHeadsSize = 32 + 32 * calls.length;

  /* ---------- helper that pushes a 32-byte word into `parts` ---------- */
  const parts: Uint8Array[] = [];
  const w = (u8: Uint8Array) => parts.push(u8);

  /* fixed-length head -------------------------------------------------- */
  w(ethers.getBytes(route.salt)); // bytes32
  w(u256be(route.source)); // u32 → u256
  w(u256be(route.destination));
  w(ethers.getBytes(route.inbox)); // bytes32
  w(u256be(32 * 6)); // offset(tokens)
  w(u256be(32 * 6 + tokenArraySize)); // offset(calls)

  /* tokens-------------------------------------------------------------- */
  w(u256be(tokenAmounts.length));
  tokenAmounts.forEach((tokenAmount) => {
    w(ethers.getBytes(tokenAmount.token));
    w(u256be(tokenAmount.amount));
  });

  /* calls -------------------------------------------------------------- */
  w(u256be(calls.length));
  let running = callHeadsSize;
  calls.forEach((call) => {
    w(u256be(running));
    running += 96 + 32 + ((call.data.length + 31) & ~31);
  });

  calls.forEach((call) => {
    w(ethers.getBytes(call.target)); // destination
    w(u256be(96)); // calldata offset
    w(u256be(Number(call.value))); // value (always 0 here)

    // the dynamic bytes itself ----------------------------------------
    w(u256be(call.data.length));
    w(u256be(call.data.length));
    const padded = new Uint8Array((call.data.length + 31) & ~31);
    padded.set(ethers.getBytes(call.data));
    parts.push(padded); // already 32-byte padded
  });

  return keccak(concatBytes(...parts));
}

/** faithful port of `hash_reward()` in encoding.rs */
function hashReward(reward: EvmReward): Uint8Array {
  const tokenAmounts = reward.tokens;
  const parts: Uint8Array[] = [];
  const w = (u8: Uint8Array) => parts.push(u8);

  w(ethers.getBytes(reward.creator));
  w(ethers.getBytes(reward.prover));
  w(u256be(reward.deadline));
  w(u256be(reward.nativeValue));
  w(u256be(32 * 5)); // offset(tokens)

  w(u256be(tokenAmounts.length));
  tokenAmounts.forEach((tokenAmount) => {
    w(ethers.getBytes(tokenAmount.token));
    w(u256be(tokenAmount.amount));
  });

  return keccak(concatBytes(...parts));
}

/** exact copy of `get_intent_hash()` */
export function calcProgramHash(
  route: EvmRoute,
  reward: EvmReward
): Uint8Array {
  return keccak(Buffer.concat([hashRoute(route), hashReward(reward)]));
}

export const usdcAmount = (usdcUi: number) => usdcUi * 10 ** USDC_DECIMALS;

export function loadKeypairFromFile(filePath: string): Keypair {
  const resolvedPath = path.resolve(
    filePath.startsWith("~") ? filePath.replace("~", homedir()) : filePath
  );
  const loadedKeyBytes = Uint8Array.from(
    JSON.parse(readFileSync(resolvedPath, "utf8"))
  );
  return Keypair.fromSecretKey(loadedKeyBytes);
}

export const svmAddressToHex = (address: PublicKey): string => {
  return "0x" + address.toBuffer().toString("hex");
};

// 1 byte is discriminator (03) + 32 bytes message_id + u32 destination_domain + u64 gas_amount
export function encodePayForGasData(
  messageId: Uint8Array, // 32 bytes
  destinationDomain: number,
  gasAmount: bigint
): Buffer {
  if (messageId.length !== 32) {
    throw new Error("messageId must be exactly 32 bytes");
  }

  const buf = Buffer.alloc(1 + 32 + 4 + 8);

  buf[0] = 3;
  buf.set(messageId, 1);
  buf.writeUInt32LE(destinationDomain, 33);
  let gasAmountOffset = 37;
  let v = gasAmount;
  for (let i = 0; i < 8; i++) {
    buf[gasAmountOffset + i] = Number((v >> BigInt(8 * i)) & BigInt(0xff));
  }
  return buf;
}

export const buildPayForGasIx = (
  solverPubkey: PublicKey,
  dispatchedMessagePda: PublicKey,
  uniqueMessage: PublicKey
): TransactionInstruction => {
  const gasPaymentSeeds = (uniqueMessage: PublicKey) => [
    anchor.utils.bytes.utf8.encode("hyperlane_igp"),
    anchor.utils.bytes.utf8.encode("-"),
    anchor.utils.bytes.utf8.encode("gas_payment"),
    anchor.utils.bytes.utf8.encode("-"),
    uniqueMessage.toBuffer(),
  ];

  const [programDataPda] = PublicKey.findProgramAddressSync(
    [
      anchor.utils.bytes.utf8.encode("hyperlane_igp"),
      anchor.utils.bytes.utf8.encode("-"),
      anchor.utils.bytes.utf8.encode("program_data"),
    ],
    IGP_PROGRAM_ID_TESTNET
  );
  const igpPda = new PublicKey("9SQVtTNsbipdMzumhzi6X8GwojiSMwBfqAhS7FgyTcqy");
  const overheadIgp = new PublicKey(
    "hBHAApi5ZoeCYHqDdCKkCzVKmBdwywdT3hMqe327eZB"
  );
  const [gasPaymentPda] = PublicKey.findProgramAddressSync(
    gasPaymentSeeds(uniqueMessage),
    IGP_PROGRAM_ID_TESTNET
  );

  // 300k should be sufficient for the test for gas amount,
  const GAS_AMOUNT = BigInt(300_000);

  const data = encodePayForGasData(
    dispatchedMessagePda.toBuffer(),
    EVM_DOMAIN_ID,
    GAS_AMOUNT
  );

  console.log("data: ", data);

  const accountsMeta = [
    { pubkey: SystemProgram.programId, isSigner: false, isWritable: false },
    { pubkey: solverPubkey, isSigner: true, isWritable: true },
    { pubkey: programDataPda, isSigner: false, isWritable: true },
    { pubkey: uniqueMessage, isSigner: true, isWritable: false },
    { pubkey: gasPaymentPda, isSigner: false, isWritable: true },
    { pubkey: igpPda, isSigner: false, isWritable: true },
    { pubkey: overheadIgp, isSigner: false, isWritable: false },
  ];

  console.log("Accounts meta: ", accountsMeta);

  const ix = new TransactionInstruction({
    programId: IGP_PROGRAM_ID_TESTNET,
    keys: accountsMeta,
    data,
  });

  return ix;
};

class SerializableAccountMeta {
  pubkey: Uint8Array; // 32 bytes
  is_signer: number; // u8  (0 / 1)
  is_writable: number; // u8
  constructor(k: {
    pubkey: Uint8Array;
    isSigner: boolean;
    isWritable: boolean;
  }) {
    this.pubkey = k.pubkey;
    this.is_signer = k.isSigner ? 1 : 0;
    this.is_writable = k.isWritable ? 1 : 0;
  }
}

class SvmCallData {
  instruction_data: Uint8Array;
  num_account_metas: number;
  constructor(f: { instruction_data: Uint8Array; num_account_metas: number }) {
    this.instruction_data = f.instruction_data;
    this.num_account_metas = f.num_account_metas;
  }
}

class SvmCallDataWithAccountMetas {
  svm_call_data: SvmCallData;
  account_metas: SerializableAccountMeta[];
  constructor(h: SvmCallData, m: SerializableAccountMeta[]) {
    this.svm_call_data = h;
    this.account_metas = m;
  }
}

const svmSchema = new Map<any, any>([
  [
    SerializableAccountMeta,
    {
      kind: "struct",
      fields: [
        ["pubkey", [32]],
        ["is_signer", "u8"],
        ["is_writable", "u8"],
      ],
    },
  ],
  [
    SvmCallData,
    {
      kind: "struct",
      fields: [
        ["instruction_data", ["u8"]],
        ["num_account_metas", "u8"],
      ],
    },
  ],
  [
    SvmCallDataWithAccountMetas,
    {
      kind: "struct",
      fields: [
        ["svm_call_data", SvmCallData],
        ["account_metas", [SerializableAccountMeta]],
      ],
    },
  ],
]);

// calldata + meta list (for Sepolia publish)
export function wrapIxFull(ix: TransactionInstruction) {
  const dest = ix.programId.toBytes();

  const header = new SvmCallData({
    instruction_data: ix.data,
    num_account_metas: ix.keys.length,
  });

  const metas = ix.keys.map(
    (key) =>
      new SerializableAccountMeta({
        pubkey: key.pubkey.toBytes(),
        isSigner: key.isSigner,
        isWritable: key.isWritable,
      })
  );

  const full = new SvmCallDataWithAccountMetas(header, metas);
  const calldata = serialize(svmSchema, full);

  return { destination: dest, calldata };
}

// calldata only (for Solana fulfil call)
export function wrapIxHeaderOnly(ix: TransactionInstruction) {
  const dest = ix.programId.toBytes();

  const header = new SvmCallData({
    instruction_data: ix.data,
    num_account_metas: ix.keys.length,
  });

  const calldata = serialize(svmSchema, header);

  return { destination: dest, calldata };
}
