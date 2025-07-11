import "@nomicfoundation/hardhat-ethers";
import "@typechain/hardhat";
import { HardhatUserConfig } from "hardhat/config";
import { config as dotenvConfig } from "dotenv";
dotenvConfig();

const config: HardhatUserConfig = {
  // We don't need solidity compilation since we're using ABI files directly
  // solidity: {
  //   version: "0.8.26",
  //   settings: { viaIR: true, optimizer: { enabled: true, runs: 200 } },
  // },
  paths: {
    sources: "./abis", // Point to ABI files (not used but required)
    artifacts: "./evm-artifacts",
    cache: "./evm-cache",
  },
  networks: {
    hardhat: { chainId: 11155111 },
    sepolia: {
      url: process.env.EVM_RPC!,
      accounts: [process.env.PK_CREATOR!, process.env.PK_SOLVER!],
      chainId: 11155111,
    },
  },
  typechain: {
    outDir: "evm-types",
    target: "ethers-v6",
    alwaysGenerateOverloads: false,
  },
};

export default config;
