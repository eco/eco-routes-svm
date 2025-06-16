import "dotenv/config";
import { ethers, JsonRpcProvider } from "ethers";

(async () => {
  const provider = new JsonRpcProvider(process.env.EVM_RPC!);
  const creator = new ethers.Wallet(process.env.PK_CREATOR!, provider);

  const solverAddr = ethers.getAddress(
    process.env.PK_SOLVER!.startsWith("0x") &&
      process.env.PK_SOLVER!.length === 42
      ? process.env.PK_SOLVER!
      : new ethers.Wallet(process.env.PK_SOLVER!).address
  );

  console.log(
    `Sending 0.02 ETH from ${await creator.getAddress()} → ${solverAddr}`
  );

  const tx = await creator.sendTransaction({
    to: solverAddr,
    value: ethers.parseEther("0.02"),
  });
  console.log(`Tx hash: ${tx.hash}`);

  await tx.wait();
  console.log("Eth transfer done");
})();
