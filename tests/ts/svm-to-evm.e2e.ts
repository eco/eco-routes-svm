/*
 *     1. Publish + Fund intent on Solana
 *     2. Fulfil intent on EVM
 *     3. Claim on Solana
 */
import "dotenv/config";
import {
  AnchorProvider,
  BN,
  Program,
  utils as anchorUtils,
} from "@coral-xyz/anchor";
import * as anchor from "@coral-xyz/anchor";
import {
  Connection,
  Keypair,
  PublicKey,
  SystemProgram,
  VersionedTransaction,
  TransactionMessage,
} from "@solana/web3.js";
import {
  TOKEN_PROGRAM_ID,
  createAssociatedTokenAccount,
  createMint,
  getAssociatedTokenAddressSync,
  mintTo,
} from "@solana/spl-token";
import { expect } from "chai";
import { EcoRoutes } from "../../target/types/eco_routes";
import { usdcAmount, loadKeypairFromFile, svmAddressToHex } from "./utils";
import {
  ECO_ROUTES_ID_MAINNET,
  EVM_DOMAIN_ID,
  INBOX_ADDRESS,
  INTENT_SOURCE_ADDRESS,
  MAILBOX_ID_MAINNET,
  SOLANA_DOMAIN_ID,
  TESTNET_RPC,
  MAINNET_RPC,
  USDC_DECIMALS,
  USDC_ADDRESS_MAINNET,
  USDC_MINT,
  HYPER_PROVER_ADDRESS,
} from "./constants";
import { ethers, Interface, JsonRpcProvider, Signer } from "ethers";
import {
  Inbox,
  Inbox__factory,
  IntentSource,
  IntentSource__factory,
  TestERC20,
  TestERC20__factory,
} from "./evm-types";
import { addressToBytes32Hex, hex32ToNums } from "./evmUtils";
import ecoRoutesIdl from "../../target/idl/eco_routes.json";
import { Reward, Route } from "./evmUtils";

const creatorSvm = loadKeypairFromFile("../../keys/program_auth_mainnet.json"); // SVM intent creator key
const connection = new Connection(MAINNET_RPC, "confirmed");
const provider = new AnchorProvider(connection, new anchor.Wallet(creatorSvm), {
  commitment: "confirmed",
});
const program = new Program(
  ecoRoutesIdl as anchor.Idl,
  provider
) as Program<EcoRoutes>;

const salt = (() => {
  const bytes = anchorUtils.bytes.utf8.encode("svm-evm-e2e1".padEnd(32, "\0"));
  return bytes.slice(0, 32);
})();
const saltHex = "0x" + Buffer.from(salt).toString("hex");

describe("SVM -> EVM e2e", () => {
  let usdc: TestERC20;
  let inbox: Inbox;
  let intentSource: IntentSource;
  let l2Provider: ethers.JsonRpcProvider;
  let solverEvm!: Signer;
  let evmTransferCalldata!: string;
  let intentHashHex!: string;
  let intentHashBytes!: Uint8Array;
  let rewardHashHex!: string;
  let svmUsdcMint: PublicKey = USDC_MINT;
  let route!: Route;
  let reward!: Reward;

  before("prepares intent data", async () => {
    console.log("EVM inbox hex:", addressToBytes32Hex(INBOX_ADDRESS));
    console.log(
      "SVM inbox bytes:",
      hex32ToNums(addressToBytes32Hex(INBOX_ADDRESS))
    );

    l2Provider = new JsonRpcProvider(process.env.EVM_RPC);
    solverEvm = new ethers.Wallet(process.env.PK_SOLVER!, l2Provider);

    const evmCallInterface = new Interface([
      "function transfer(address,uint256)",
    ]);
    const evmCallTransferAmount = BigInt(usdcAmount(5));

    evmTransferCalldata = evmCallInterface.encodeFunctionData("transfer", [
      await solverEvm.getAddress(),
      evmCallTransferAmount,
    ]);

    const transferUsdcEvmCall = {
      target: USDC_ADDRESS_MAINNET,
      data: evmTransferCalldata,
      value: BigInt(0),
    };

    const routeTokens = [
      {
        token: USDC_ADDRESS_MAINNET,
        amount: BigInt(usdcAmount(5)),
      },
    ];

    const rewardTokens = [
      {
        token: svmAddressToHex(svmUsdcMint),
        amount: BigInt(usdcAmount(5)),
      },
    ];

    const calls = [transferUsdcEvmCall];

    // Create route for fulfillAndProve (with regular addresses)
    route = {
      salt: saltHex,
      source: SOLANA_DOMAIN_ID,
      destination: EVM_DOMAIN_ID,
      inbox: INBOX_ADDRESS,
      tokens: routeTokens,
      calls,
    };

    // Create route for getIntentHash (with bytes32 addresses)
    const routeForHash = {
      salt: saltHex,
      source: SOLANA_DOMAIN_ID,
      destination: EVM_DOMAIN_ID,
      inbox: addressToBytes32Hex(INBOX_ADDRESS),
      tokens: routeTokens.map((token) => ({
        token: addressToBytes32Hex(token.token),
        amount: token.amount,
      })),
      calls: calls.map((call) => ({
        target: addressToBytes32Hex(call.target),
        data: call.data,
        value: call.value,
      })),
    };

    reward = {
      creator: svmAddressToHex(creatorSvm.publicKey),
      prover: svmAddressToHex(ECO_ROUTES_ID_MAINNET),
      deadline: BigInt(1756627873),
      nativeValue: BigInt(0),
      tokens: rewardTokens,
    };

    // IntentSource contract
    intentSource = IntentSource__factory.connect(
      INTENT_SOURCE_ADDRESS,
      solverEvm
    );

    usdc = TestERC20__factory.connect(USDC_ADDRESS_MAINNET, solverEvm);

    inbox = Inbox__factory.connect(INBOX_ADDRESS, solverEvm);

    const { intentHash, routeHash, rewardHash } = await intentSource[
      "getIntentHash(((bytes32,uint256,uint256,bytes32,(bytes32,uint256)[],(bytes32,bytes,uint256)[]),(bytes32,bytes32,uint256,uint256,(bytes32,uint256)[])))"
    ]({
      route: routeForHash,
      reward,
    });

    // EVM one for TestProver
    intentHashHex = intentHash;
    rewardHashHex = rewardHash;

    // store the one Solana needs
    intentHashBytes = ethers.getBytes(intentHash);

    const rewardFormatted = {
      creator: new PublicKey(Buffer.from(reward.creator.slice(2), "hex")),
      tokens: reward.tokens.map((token) => ({
        token: Array.from(Buffer.from(token.token.slice(2), "hex")),
        amount: token.amount,
      })),
      prover: Array.from(Buffer.from(reward.prover.slice(2), "hex")),
      nativeValue: reward.nativeValue,
      deadline: reward.deadline,
    };
    console.log("rewardFormatted", rewardFormatted);
    console.log(rewardFormatted.tokens);

    expect(intentHashBytes.length).equals(32);
  });

  it("Publish + Fund intent on Solana", async () => {
    const executionAuthority = PublicKey.findProgramAddressSync(
      [Buffer.from("execution_authority"), salt],
      program.programId
    )[0];

    const amountBN = new BN(usdcAmount(5));

    const executionAuthorityAta = getAssociatedTokenAddressSync(
      svmUsdcMint,
      executionAuthority,
      true
    );
    const executionAuthorityAtaData = await connection.getAccountInfo(
      executionAuthorityAta
    );
    if (!executionAuthorityAtaData) {
      await createAssociatedTokenAccount(
        connection,
        creatorSvm,
        svmUsdcMint,
        executionAuthority,
        { commitment: "confirmed" },
        undefined,
        undefined,
        true
      );
    }
    const routeSolTokenArg = [
      {
        token: hex32ToNums(addressToBytes32Hex(USDC_ADDRESS_MAINNET)),
        amount: amountBN,
      },
    ];

    const rewardSolTokenArg = [
      {
        token: Array.from(svmUsdcMint.toBytes()),
        amount: amountBN,
      },
    ];

    const destinationSol = Array.from(
      Buffer.from(ethers.getBytes(addressToBytes32Hex(USDC_ADDRESS_MAINNET)))
    );
    const calldataSol = Buffer.from(ethers.getBytes(evmTransferCalldata));
    const callsSol = [
      {
        destination: destinationSol,
        calldata: calldataSol,
      },
    ];

    console.log("SVM passed destination: ", destinationSol);
    console.log("SVM passed calldata: ", calldataSol);

    const routeSol = {
      salt: Array.from(Buffer.from(saltHex.slice(2), "hex")),
      sourceDomainId: SOLANA_DOMAIN_ID,
      destinationDomainId: EVM_DOMAIN_ID,
      inbox: hex32ToNums(addressToBytes32Hex(INBOX_ADDRESS)),
      tokens: routeSolTokenArg,
      calls: callsSol,
    };

    const rewardSol = {
      creator: creatorSvm.publicKey,
      prover: Array.from(ECO_ROUTES_ID_MAINNET.toBytes()),
      tokens: rewardSolTokenArg,
      nativeAmount: new BN(0),
      deadline: new BN(reward.deadline.toString()),
    };

    console.log("routeSol", routeSol);
    console.log("rewardSol", rewardSol);
    console.log("rewardSol.deadline", rewardSol.deadline.toString());

    // console.log(
    //   "Reward sol prover hex: ",
    //   Buffer.from(rewardSol.prover).toString("hex")
    // );
    // console.log(
    //   "Route sol destination hex: ",
    //   Buffer.from(routeSol.calls[0].destination).toString("hex")
    // );
    // console.log(
    //   "Route sol calldata hex: ",
    //   Buffer.from(routeSol.calls[0].calldata).toString("hex")
    // );
    // console.log(
    //   "Route sol inbox hex: ",
    //   Buffer.from(routeSol.inbox).toString("hex")
    // );

    // console.log("SVM passed call: ", callsSol);
    // console.log("SVM route tokens: ", routeSolTokenArg);
    // console.log("SVM reward tokens: ", rewardSolTokenArg);
    // console.log("SVM route: ", routeSol);
    // console.log("SVM reward: ", rewardSol);
    // console.log("intentHashBytes", intentHashBytes);
    // console.log("Intent hash bytes (Solana):", "0x" + Buffer.from(intentHashBytes).toString("hex"));

    const intent = PublicKey.findProgramAddressSync(
      [Buffer.from("intent"), intentHashBytes],
      program.programId
    )[0];

    const vault = PublicKey.findProgramAddressSync(
      [Buffer.from("reward"), intentHashBytes, svmUsdcMint.toBytes()],
      program.programId
    )[0];

    const publishIx = await program.methods
      .publishIntent({
        salt: Array.from(salt),
        intentHash: Array.from(intentHashBytes),
        destinationDomainId: EVM_DOMAIN_ID,
        inbox: routeSol.inbox,
        routeTokens: routeSol.tokens,
        calls: routeSol.calls,
        rewardTokens: rewardSol.tokens,
        nativeReward: rewardSol.nativeAmount,
        deadline: rewardSol.deadline,
      })
      .accountsStrict({
        intent,
        creator: creatorSvm.publicKey,
        payer: creatorSvm.publicKey,
        systemProgram: SystemProgram.programId,
      })
      .instruction();

    let blockhash = await connection.getLatestBlockhash();
    let publishIntentTx = new VersionedTransaction(
      new TransactionMessage({
        payerKey: creatorSvm.publicKey,
        recentBlockhash: blockhash.blockhash,
        instructions: [publishIx],
      }).compileToV0Message()
    );
    publishIntentTx.sign([creatorSvm]);

    const publishIntentTxSignature = await connection.sendRawTransaction(
      publishIntentTx.serialize()
    );
    await connection.confirmTransaction(
      {
        signature: publishIntentTxSignature,
        blockhash: blockhash.blockhash,
        lastValidBlockHeight: blockhash.lastValidBlockHeight,
      },
      "confirmed"
    );

    // verify intent published
    const intentAccountInfo = await connection.getAccountInfo(intent);
    expect(intentAccountInfo.data.length).to.be.greaterThan(0);

    // "Fund SPL" transfer of USDC to Intent
    const fundSplIx = await program.methods
      .fundIntentSpl({
        intentHash: Array.from(intentHashBytes),
        tokenIndex: 0,
      })
      .accountsStrict({
        intent,
        funder: creatorSvm.publicKey,
        payer: creatorSvm.publicKey,
        systemProgram: SystemProgram.programId,
        funderToken: getAssociatedTokenAddressSync(
          svmUsdcMint,
          creatorSvm.publicKey
        ),
        vault,
        mint: svmUsdcMint,
        tokenProgram: TOKEN_PROGRAM_ID,
      })
      .instruction();

    blockhash = await connection.getLatestBlockhash();
    let fundIntentTx = new VersionedTransaction(
      new TransactionMessage({
        payerKey: creatorSvm.publicKey,
        recentBlockhash: blockhash.blockhash,
        instructions: [fundSplIx],
      }).compileToV0Message()
    );
    fundIntentTx.sign([creatorSvm]);

    const fundIntentTxSignature = await connection.sendRawTransaction(
      fundIntentTx.serialize()
    );
    await connection.confirmTransaction(
      {
        signature: fundIntentTxSignature,
        blockhash: blockhash.blockhash,
        lastValidBlockHeight: blockhash.lastValidBlockHeight,
      },
      "confirmed"
    );

    // verify intent funded
    const intentAccount = await program.account.intent.fetch(intent);
    console.log("intentAccount", intentAccount);
    // expect(intentAccount.status.funded).to.be.true;
  });

  it("Fulfil intent on EVM", async () => {
    const solverEvmAddress = await solverEvm.getAddress();
    console.log("solverEvmAddress", solverEvmAddress);

    const solverUsdcBalance = await usdc.balanceOf(solverEvmAddress);
    console.log(
      "Solver USDC balance:",
      ethers.formatUnits(solverUsdcBalance, 6)
    );

    if (solverUsdcBalance < BigInt(usdcAmount(5))) {
      console.log(
        "Solver doesn't have enough USDC. This test requires the solver to have USDC tokens."
      );

      try {
        await inbox.fulfillAndProve.staticCall(
          route,
          rewardHashHex,
          solverEvmAddress,
          intentHashHex,
          HYPER_PROVER_ADDRESS,
          ethers.getBytes("0x")
        );
        console.log(
          "Static call succeeded - the transaction would work if solver had tokens"
        );
      } catch (error) {
        console.log("Static call failed:", error.message);
      }

      // Skip the actual transaction since we don't have tokens
      console.log("Skipping actual transaction due to insufficient tokens");
      return;
    }

    // The solver needs to have enough USDC to fulfill the intent
    // In a real scenario, this would be handled by the solver's own logic
    const usdcApproveTx = await usdc
      .connect(solverEvm)
      .approve(INBOX_ADDRESS, usdcAmount(5));
    await usdcApproveTx.wait(10);

    const allowance = await usdc.allowance(solverEvmAddress, INBOX_ADDRESS);
    console.log("USDC allowance:", ethers.formatUnits(allowance, 6));
    console.log("hyper prover address", HYPER_PROVER_ADDRESS);

    // hash
    const data = ethers.getBytes("0x");

    console.log("About to call fulfillAndProve with:");
    console.log("route:", route);
    console.log("rewardHashHex:", rewardHashHex);
    console.log("solverEvmAddress:", solverEvmAddress);
    console.log("intentHashHex:", intentHashHex);
    console.log("HYPER_PROVER_ADDRESS_MAINNET:", HYPER_PROVER_ADDRESS);

    const tx = await inbox.fulfillAndProve(
      route,
      rewardHashHex,
      solverEvmAddress,
      intentHashHex,
      HYPER_PROVER_ADDRESS,
      data,
      {
        gasLimit: 800_000,
      }
    );

    console.log("Transaction hash:", tx.hash);
    const receipt = await tx.wait();
    console.log("Transaction receipt:", receipt);
    console.log("Transaction status:", receipt.status);

    const fulfilledMappingSlot = await inbox.fulfilled(intentHashHex);
    console.log("fulfilled mapping result:", fulfilledMappingSlot);
    expect(fulfilledMappingSlot).to.equal(solverEvmAddress);
  });

  // Un-skip when a message passes
  it.skip("Claim intent on Solana", async () => {
    const intent = PublicKey.findProgramAddressSync(
      [Buffer.from("intent"), intentHashBytes],
      program.programId
    )[0];

    const vault = PublicKey.findProgramAddressSync(
      [Buffer.from("reward"), intentHashBytes, svmUsdcMint.toBytes()],
      program.programId
    )[0];

    // spl claim ix
    const claimSplIx = await program.methods
      .claimIntentSpl({
        intentHash: Array.from(intentHashBytes),
        tokenIndex: 0,
      })
      .accountsStrict({
        intent,
        claimer: creatorSvm.publicKey,
        payer: creatorSvm.publicKey,
        systemProgram: SystemProgram.programId,
        vault,
        claimerToken: getAssociatedTokenAddressSync(
          svmUsdcMint,
          creatorSvm.publicKey
        ),
        mint: svmUsdcMint,
        tokenProgram: TOKEN_PROGRAM_ID,
      })
      .instruction();

    const blockhash = await connection.getLatestBlockhash();
    let claimIntentTx = new VersionedTransaction(
      new TransactionMessage({
        payerKey: creatorSvm.publicKey,
        recentBlockhash: blockhash.blockhash,
        instructions: [claimSplIx],
      }).compileToV0Message()
    );
    claimIntentTx.sign([creatorSvm]);

    const claimIntentTxSignature = await connection.sendRawTransaction(
      claimIntentTx.serialize()
    );
    await connection.confirmTransaction(
      {
        signature: claimIntentTxSignature,
        blockhash: blockhash.blockhash,
        lastValidBlockHeight: blockhash.lastValidBlockHeight,
      },
      "confirmed"
    );

    // verify status to be claimed
    const intentAccount = await program.account.intent.fetch(intent);
    const claimed = intentAccount.status.claimed[0];
    expect(claimed).to.be.true;
  });
});
