import "dotenv/config";
import { ethers, JsonRpcProvider } from "ethers";
import { TestERC20__factory } from "../evm-types";
import { TEST_USDC_ADDRESS_TESTNET } from "../constants";

const RPC = process.env.RPC_SEPOLIA!;
const AMOUNT = ethers.parseUnits("1000", 6); // 1 000 USDC

(async () => {
  const provider = new JsonRpcProvider(RPC);
  const owner = new ethers.Wallet(process.env.PK_CREATOR!, provider);
  const usdc = TestERC20__factory.connect(
    TEST_USDC_ADDRESS_TESTNET.toLowerCase(),
    owner
  );

  for (const pk of [process.env.PK_CREATOR!, process.env.PK_SOLVER!]) {
    const addr = new ethers.Wallet(pk).address;
    console.log(`Minting to ${addr}`);
    const tx = await usdc.mint(addr, AMOUNT);
    await tx.wait();
  }
  console.log("Done minting test USDC tokens");
})();
