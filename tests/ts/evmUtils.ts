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

const abi = ethers.AbiCoder.defaultAbiCoder();
const ROUTE_TYPE =
  "tuple(bytes32 salt,uint256 source,uint256 destination,bytes32 inbox," +
  "tuple(bytes32 token,uint256 amount)[] tokens," +
  "tuple(bytes32 target,bytes data,uint256 value)[] calls)";

const REWARD_TYPE =
  "tuple(bytes32 creator,bytes32 prover,uint256 deadline,uint256 nativeValue," +
  "tuple(bytes32 token,uint256 amount)[] tokens)";

export function encodeRoute(route: Route): string {
  return abi.encode([ROUTE_TYPE], [route]);
}

export function encodeReward(reward: Reward): string {
  return abi.encode([REWARD_TYPE], [reward]);
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
