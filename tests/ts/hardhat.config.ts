import "@nomicfoundation/hardhat-ethers";
import "@typechain/hardhat";
import { HardhatUserConfig } from "hardhat/config";

const config: HardhatUserConfig = {
  solidity: "0.8.26",
  paths: {
    sources: "./evm-contracts",
    artifacts: "./evm-artifacts",
    cache: "./evm-cache",
  },
  networks: {
    hardhat: { chainId: 11155111 },
  },
  typechain: {
    outDir: "evm-types",
    target: "ethers-v6",
  },
};

export default config;
