import "dotenv/config";
import { ethers, JsonRpcProvider } from "ethers";
import { TestERC20__factory } from "../evm-types";

const INITIAL_MINT_UI = "1000";

(async () => {
  const provider = new JsonRpcProvider(process.env.EVM_RPC!);

  const owner = new ethers.Wallet(process.env.PK_CREATOR!, provider);
  console.log("Deployer (owner):", owner.address);

  // deploy the test contract
  console.log("\nDeploying TestERC20...");
  const usdc = await new TestERC20__factory(owner).deploy(
    "Test USDC",
    "USDC",
    6
  );
  await usdc.waitForDeployment();
  console.log("Deployed contrat's address:", usdc.target, "\n");

  // mint some tokens to creator & solver
  const amount = ethers.parseUnits(INITIAL_MINT_UI, 6);

  const solverAddr = new ethers.Wallet(process.env.PK_SOLVER!).address;

  console.log(`Minting USDC to owner..`);
  await (await usdc.mint(owner.address, amount)).wait();

  console.log(`Minting USDC to solver..`);
  await (await usdc.mint(solverAddr, amount)).wait();

  // dsplay balances
  const dec = await usdc.decimals();
  const ownerBalance = await usdc.balanceOf(owner.address);
  const solverBalance = await usdc.balanceOf(solverAddr);

  console.log("\nBalances:");
  console.log(`${owner.address} :`, ethers.formatUnits(ownerBalance, dec));
  console.log(`${solverAddr} :`, ethers.formatUnits(solverBalance, dec));
})();
