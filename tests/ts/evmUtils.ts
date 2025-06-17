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
  "tuple(bytes32 salt,uint256 sourceDomainId,uint256 destinationDomainId," +
  "bytes32 inbox,tuple(bytes32 token,uint256 amount)[] tokens," +
  "tuple(bytes32 destination,bytes calldata,uint256 value)[] calls)";

const REWARD_TYPE =
  "tuple(bytes32 creator,bytes32 prover,uint256 deadline,uint256 nativeValue," +
  "tuple(bytes32 token,uint256 amount)[] tokens)";

export function encodeRoute(route: Route): string {
  return abi.encode(
    [ROUTE_TYPE],
    [
      {
        salt: route.salt,
        sourceDomainId: route.source,
        destinationDomainId: route.destination,
        inbox: route.inbox,
        tokens: route.tokens.map((tokenAmount) => ({
          token: tokenAmount.token,
          amount: tokenAmount.amount,
        })),
        calls: route.calls.map((c) => ({
          destination: c.target,
          calldata: c.data,
          value: c.value,
        })),
      },
    ]
  );
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

export const addressToBytes32Hex = (addr: string): string => {
  return ethers.zeroPadValue(addr, 32);
};

export function encodeTransfer(to: string, value: number): string {
  const erc20ABI = ["function transfer(address to, uint256 value)"];
  const abiInterface = new ethers.Interface(erc20ABI);
  const callData = abiInterface.encodeFunctionData("transfer", [to, value]);
  return callData;
}

export const hex32ToBytes = (hex: string): Uint8Array => {
  return ethers.getBytes(hex);
};

export const hex32ToNums = (hex: string): number[] => {
  return Array.from(ethers.getBytes(hex));
};
