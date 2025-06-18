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
  PublicKey,
  SystemProgram,
  VersionedTransaction,
  TransactionMessage,
} from "@solana/web3.js";
import {
  TOKEN_PROGRAM_ID,
  createAssociatedTokenAccount,
  getAssociatedTokenAddressSync,
} from "@solana/spl-token";
import { expect } from "chai";
import { EcoRoutes } from "../../target/types/eco_routes";
import { usdcAmount, loadKeypairFromFile, svmAddressToHex } from "./utils";
import {
  ECO_ROUTES_ID_MAINNET,
  EVM_DOMAIN_ID,
  INBOX_ADDRESS,
  INTENT_SOURCE_ADDRESS,
  SOLANA_DOMAIN_ID,
  MAINNET_RPC,
  USDC_ADDRESS_MAINNET,
  USDC_MINT,
  HYPER_PROVER_ADDRESS,
} from "./constants";
import { ethers, JsonRpcProvider, Signer } from "ethers";
import {
  HyperProver,
  HyperProver__factory,
  Inbox,
  Inbox__factory,
  IntentSource,
  IntentSource__factory,
  TestERC20,
  TestERC20__factory,
} from "./evm-types";
import { addressToBytes32Hex, encodeTransfer, hex32ToNums } from "./evmUtils";
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
  const bytes = anchorUtils.bytes.utf8.encode(
    "svm-evm-e2e31248".padEnd(32, "\0")
  );
  return bytes.slice(0, 32);
})();
const saltHex = "0x" + Buffer.from(salt).toString("hex");

const inboxErrorInterface = new ethers.Interface([
  "error InsufficientFee(uint256 _requiredFee)",
  "error NativeTransferFailed()",
  "error ChainIdTooLarge(uint256 _chainId)",
  "error UnauthorizedHandle(address _sender)",
  "error UnauthorizedProve(address _sender)",
  "error UnauthorizedIncomingProof(address _sender)",
  "error MailboxCannotBeZeroAddress()",
  "error RouterCannotBeZeroAddress()",
  "error InboxCannotBeZeroAddress()",
  "error ProverCannotBeZeroAddress()",
  "error InvalidOriginChainId()",
  "error NotFulfilled(bytes32)",
  "error AlreadyProven(bytes32)",
  "error InsufficientFee(uint256)",
  "error ArrayLengthMismatch()",
  "error ChainIdTooLarge(uint256)",
  "error SenderCannotBeZeroAddress()",
]);

describe("SVM -> EVM e2e", () => {
  let usdc: TestERC20;
  let inbox: Inbox;
  let hyperProver: HyperProver;
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
  let routeForHash!: any;

  before("prepares intent data", async () => {
    console.log("EVM inbox hex:", addressToBytes32Hex(INBOX_ADDRESS));
    console.log(
      "SVM inbox bytes:",
      hex32ToNums(addressToBytes32Hex(INBOX_ADDRESS))
    );

    l2Provider = new JsonRpcProvider(process.env.EVM_RPC);
    solverEvm = new ethers.Wallet(process.env.PK_SOLVER!, l2Provider);

    const evmCallTransferAmount = BigInt(usdcAmount(5));

    evmTransferCalldata = encodeTransfer(
      await solverEvm.getAddress(),
      Number(evmCallTransferAmount)
    );

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
        amount: BigInt(usdcAmount(1)),
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
    routeForHash = {
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
    hyperProver = HyperProver__factory.connect(HYPER_PROVER_ADDRESS, solverEvm);

    const { intentHash, rewardHash } = await intentSource[
      "getIntentHash(((bytes32,uint256,uint256,bytes32,(bytes32,uint256)[],(bytes32,bytes,uint256)[]),(bytes32,bytes32,uint256,uint256,(bytes32,uint256)[])))"
    ]({
      route: routeForHash,
      reward,
    });

    // EVM one for HyperProver
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
        amount: new BN(usdcAmount(1)),
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

    // The solver needs to have enough USDC to fulfill the intent
    // In a real scenario, this would be handled by the solver's own logic
    const usdcApproveTx = await usdc
      .connect(solverEvm)
      .approve(INBOX_ADDRESS, usdcAmount(10));
    await usdcApproveTx.wait(10);

    const allowance = await usdc.allowance(solverEvmAddress, INBOX_ADDRESS);
    console.log("USDC allowance:", ethers.formatUnits(allowance, 6));
    console.log("hyper prover address", HYPER_PROVER_ADDRESS);

    const sourceChainProver = ethers.zeroPadValue(
      svmAddressToHex(ECO_ROUTES_ID_MAINNET),
      32
    );
    const data = ethers.AbiCoder.defaultAbiCoder().encode(
      ["bytes32", "bytes", "address"],
      [sourceChainProver, "0x", ethers.ZeroAddress]
    );

    console.log("About to call fulfillAndProve with:");
    console.log("route:", route);
    console.log("rewardHashHex:", rewardHashHex);
    console.log("solverEvmAddress:", solverEvmAddress);
    console.log("intentHashHex:", intentHashHex);
    console.log("HYPER_PROVER_ADDRESS_MAINNET:", HYPER_PROVER_ADDRESS);

    const requiredFee = await hyperProver.fetchFee(
      SOLANA_DOMAIN_ID,
      [intentHashHex],
      [solverEvmAddress], // claimant on Solana (32-byte address later)
      data
    );

    console.log("Required fee:", requiredFee.toString());

    // add 5% to the fee (to be safe)
    const buffer =
      requiredFee / BigInt(20) > ethers.parseEther("0.0005")
        ? requiredFee / BigInt(20)
        : ethers.parseEther("0.0005");

    async function diagnosePotentialRevert() {
      try {
        await inbox.fulfillAndProve.staticCall(
          route,
          rewardHashHex,
          solverEvmAddress,
          intentHashHex,
          HYPER_PROVER_ADDRESS,
          data,
          { value: requiredFee + buffer, gasLimit: 900_000 }
        );
        console.log(
          "callStatic succeeded (on-chain revert is gas/fee related)"
        );
      } catch (err: any) {
        const revertData: string = err.data ?? err.error?.data ?? "";
        const desc = inboxErrorInterface.parseError(revertData);

        console.log("name:", desc.name);
        console.log("selector:", desc.selector);
        console.log("signature:", desc.signature);
        console.log("args:", desc.args);
        throw err;
      }
    }
    await diagnosePotentialRevert();

    const fulfillTx = await inbox.fulfillAndProve(
      route,
      rewardHashHex,
      // TODO: figure out how to pass an SVM 32-byte address
      // (should the Inbox contract be updated?)
      solverEvmAddress,
      intentHashHex,
      HYPER_PROVER_ADDRESS,
      data,
      { value: requiredFee + buffer, gasLimit: 900_000 }
    );

    console.log("Fulfill ransaction hash:", fulfillTx.hash);
    const fulfillTxReceipt = await fulfillTx.wait(3);
    console.log("Fulfill transaction receipt:", fulfillTxReceipt);
    console.log("Fulfill transaction status:", fulfillTxReceipt.status);

    const fulfilledMappingSlot = await inbox.fulfilled(intentHashHex);
    console.log("Fulfilled mapping result:", fulfilledMappingSlot);
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
