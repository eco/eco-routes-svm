import BN from "bn.js";

export interface TokenAmount {
  token: Uint8Array; // 32-bytes
  amount: BN;
}

export interface Route {
  salt: Uint8Array; // 32-bytes
  sourceDomainId: number;
  destinationDomainId: number;
  inbox: Uint8Array; // 32-bytes
  tokens: TokenAmount[];
  calls: Uint8Array[]; // raw bytes of SvmCallDataWithAccountMetas
}

export interface Reward {
  creator: Uint8Array; // 32-bytes
  prover: Uint8Array; // 32-bytes
  tokens: TokenAmount[];
  nativeAmount: BN;
  deadline: BN;
}
