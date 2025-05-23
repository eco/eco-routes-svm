import { ethers } from "ethers";
import { USDC_DECIMALS } from "./constants";

export interface TokenAmount {
  token: string;
  amount: bigint;
}
export interface Route {
  salt: string; // bytes32 hex
  source: number;
  destination: number;
  inbox: string; // address
  tokens: TokenAmount[];
  calls: { target: string; data: string; value: bigint }[];
}
export interface Reward {
  creator: string;
  prover: string;
  deadline: bigint;
  nativeValue: bigint;
  tokens: TokenAmount[];
}

export function encodeRoute(route: Route): string {
  return ethers.solidityPacked(
    [
      "bytes32",
      "uint256",
      "uint256",
      "address",
      "tuple(address token,uint256 amount)[]",
      "tuple(address target,bytes data,uint256 value)[]",
    ],
    [
      route.salt,
      route.source,
      route.destination,
      route.inbox,
      route.tokens.map((t) => [t.token, t.amount]),
      route.calls.map((c) => [c.target, c.data, c.value]),
    ]
  );
}

export function encodeReward(r: Reward): string {
  return ethers.solidityPacked(
    [
      "address",
      "address",
      "uint256",
      "uint256",
      "tuple(address token,uint256 amount)[]",
    ],
    [
      r.creator,
      r.prover,
      r.deadline,
      r.nativeValue,
      r.tokens.map((t) => [t.token, t.amount]),
    ]
  );
}

export function hashIntent(routeEnc: string, rewardEnc: string): string {
  return ethers.keccak256(
    ethers.solidityPacked(
      ["bytes32", "bytes32"],
      [ethers.keccak256(routeEnc), ethers.keccak256(rewardEnc)]
    )
  );
}

export const evmUsdcAmount = (ui: string | number): bigint =>
  ethers.parseUnits(ui.toString(), USDC_DECIMALS);

export const addressToBytes32 = (addr: string): Uint8Array => {
  return Uint8Array.from(
    Buffer.concat([Buffer.alloc(12), Buffer.from(addr.slice(2), "hex")])
  );
};
