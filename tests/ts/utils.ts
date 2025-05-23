import { keccak_256 } from "@noble/hashes/sha3";
import { BN } from "@coral-xyz/anchor";
import { Reward, Route } from "./types";
import path from "path";
import { readFileSync } from "fs";
import { Keypair, PublicKey } from "@solana/web3.js";
import { homedir } from "os";
import { USDC_DECIMALS } from "./constants";

// Concatenate buffers and hash with Keccak-256
export function generateIntentHash(route: Route, reward: Reward): Uint8Array {
  const concat = [
    route.salt,
    new Uint8Array(Uint32Array.of(route.sourceDomainId).buffer),
    new Uint8Array(Uint32Array.of(route.destinationDomainId).buffer),
    route.inbox,
    ...route.tokens.flatMap((t) => [t.token, toU8(t.amount)]),
    ...route.calls,
    reward.creator,
    reward.prover,
    ...reward.tokens.flatMap((t) => [t.token, toU8(t.amount)]),
    toU8(reward.nativeAmount),
    toU8(reward.deadline),
  ].reduce((acc, cur) => {
    const tmp = new Uint8Array(acc.length + cur.length);
    tmp.set(acc);
    tmp.set(cur, acc.length);
    return tmp;
  }, new Uint8Array());

  return keccak_256(concat);
}

function toU8(bn: BN): Uint8Array {
  const buf = Buffer.alloc(8);
  buf.writeBigUInt64LE(BigInt(bn.toString()));
  return new Uint8Array(buf);
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
