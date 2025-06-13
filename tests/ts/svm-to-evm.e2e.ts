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
  ECO_ROUTES_ID_TESTNET,
  EVM_DOMAIN_ID,
  INBOX_ADDRESS_TESTNET,
  INTENT_SOURCE_ADDRESS_TESTNET,
  MAILBOX_ID_TESTNET,
  SOLANA_DOMAIN_ID,
  STORAGE_PROVER_ADDRESS_TESTNET,
  TEST_USDC_ADDRESS_TESTNET,
  TESTNET_RPC,
  USDC_DECIMALS,
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

const creatorSvm = loadKeypairFromFile("../../keys/program_auth.json"); // SVM intent creator key
const connection = new Connection(TESTNET_RPC, "confirmed");
const provider = new AnchorProvider(connection, new anchor.Wallet(creatorSvm), {
  commitment: "confirmed",
});
const program = new Program(
  ecoRoutesIdl as anchor.Idl,
  provider
) as Program<EcoRoutes>;

const salt = (() => {
  const bytes = anchorUtils.bytes.utf8.encode("svm-evm-e2e".padEnd(32, "\0"));
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
  let mockSvmUsdcMint: PublicKey;
  let route!: Route;
  let reward!: Reward;

  before(
    "creates a testet usdc and mints to an SVM intent creator",
    async () => {
      console.log("EVM inbox hex:", addressToBytes32Hex(INBOX_ADDRESS_TESTNET));
      console.log(
        "SVM inbox bytes:",
        hex32ToNums(addressToBytes32Hex(INBOX_ADDRESS_TESTNET))
      );

      // Reward token mint
      const usdcMint = Keypair.generate();
      mockSvmUsdcMint = usdcMint.publicKey;

      await createMint(
        connection,
        creatorSvm, // payer
        creatorSvm.publicKey, // mint authority
        null, // freeze authority
        USDC_DECIMALS, // decimals
        usdcMint, // mint keypair
        {
          commitment: "confirmed",
        }
      );

      const ata = await createAssociatedTokenAccount(
        connection,
        creatorSvm,
        mockSvmUsdcMint,
        creatorSvm.publicKey,
        {
          commitment: "confirmed",
        }
      );

      await mintTo(
        connection,
        creatorSvm,
        mockSvmUsdcMint,
        ata,
        creatorSvm,
        usdcAmount(1000), // amount to mint
        [],
        {
          commitment: "confirmed",
        }
      );

      l2Provider = new JsonRpcProvider(process.env.RPC_SEPOLIA);
      solverEvm = new ethers.Wallet(process.env.PK_SOLVER!, l2Provider);

      const evmCallInterface = new Interface([
        "function transfer(address,uint256)",
      ]);
      const evmCallTransferAmount = BigInt(usdcAmount(5));

      evmTransferCalldata = evmCallInterface.encodeFunctionData("transfer", [
        await solverEvm.getAddress(), // recipient on Sepolia
        evmCallTransferAmount,
      ]);

      const transferUsdcEvmCall = {
        target: addressToBytes32Hex(TEST_USDC_ADDRESS_TESTNET),
        data: evmTransferCalldata,
        value: BigInt(0),
      };

      const routeTokens = [
        {
          token: addressToBytes32Hex(TEST_USDC_ADDRESS_TESTNET),
          amount: BigInt(usdcAmount(5)),
        },
      ];

      const rewardTokens = [
        {
          token: svmAddressToHex(mockSvmUsdcMint),
          amount: BigInt(usdcAmount(5)),
        },
      ];

      const calls = [transferUsdcEvmCall];

      // Get EVM intent and hashes
      route = {
        salt: saltHex,
        source: SOLANA_DOMAIN_ID,
        destination: EVM_DOMAIN_ID,
        inbox: addressToBytes32Hex(INBOX_ADDRESS_TESTNET),
        tokens: routeTokens,
        calls,
      };

      reward = {
        creator: svmAddressToHex(creatorSvm.publicKey),
        prover: svmAddressToHex(ECO_ROUTES_ID_TESTNET),
        deadline: BigInt(1000000000000),
        nativeValue: BigInt(0),
        tokens: rewardTokens,
      };

      // IntentSource contract
      intentSource = IntentSource__factory.connect(
        INTENT_SOURCE_ADDRESS_TESTNET,
        solverEvm
      );

      usdc = TestERC20__factory.connect(TEST_USDC_ADDRESS_TESTNET, solverEvm);

      inbox = Inbox__factory.connect(INBOX_ADDRESS_TESTNET, solverEvm);

      const { intentHash, routeHash, rewardHash } = await intentSource[
        "getIntentHash(((bytes32,uint256,uint256,bytes32,(bytes32,uint256)[],(bytes32,bytes,uint256)[]),(bytes32,bytes32,uint256,uint256,(bytes32,uint256)[])))"
      ]({
        route,
        reward,
      });
      // EVM one for TestProver
      intentHashHex = intentHash;
      rewardHashHex = rewardHash;

      // store the one Solana needs
      intentHashBytes = ethers.getBytes(intentHash);

      console.log("EVM passed call: ", transferUsdcEvmCall);
      console.log("EVM route tokens: ", routeTokens);
      console.log("EVM reward tokens: ", rewardTokens);
      console.log("EVM route: ", route);
      console.log("EVM reward: ", reward);
      console.log("Intent hash bytes (for an SVM call): ", intentHashBytes);
      console.log("Route hash hex (EVM): ", routeHash);
      console.log("Reward hash hex (EVM): ", rewardHash);
      console.log("Intent hash hex (EVM): ", intentHashHex);

      expect(intentHashBytes.length).equals(32);
    }
  );

  it("Publish + Fund intent on Solana", async () => {
    const executionAuthority = PublicKey.findProgramAddressSync(
      [Buffer.from("execution_authority"), salt],
      program.programId
    )[0];

    const amountBN = new BN(usdcAmount(5));

    const executionAuthorityAta = getAssociatedTokenAddressSync(
      mockSvmUsdcMint,
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
        mockSvmUsdcMint,
        executionAuthority,
        { commitment: "confirmed" },
        undefined,
        undefined,
        true
      );
    }
    const routeSolTokenArg = [
      {
        token: hex32ToNums(addressToBytes32Hex(TEST_USDC_ADDRESS_TESTNET)),
        amount: amountBN,
      },
    ];

    const rewardSolTokenArg = [
      {
        token: Array.from(mockSvmUsdcMint.toBytes()),
        amount: amountBN,
      },
    ];

    const destinationSol = Array.from(
      Buffer.from(
        ethers.getBytes(addressToBytes32Hex(TEST_USDC_ADDRESS_TESTNET))
      )
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
      inbox: hex32ToNums(addressToBytes32Hex(INBOX_ADDRESS_TESTNET)),
      tokens: routeSolTokenArg,
      calls: callsSol,
    };

    const rewardSol = {
      creator: creatorSvm.publicKey,
      prover: Array.from(ECO_ROUTES_ID_TESTNET.toBytes()),
      tokens: rewardSolTokenArg,
      nativeAmount: new BN(0),
      deadline: new BN(1000000000000),
    };

    console.log(
      "Reward sol prover hex: ",
      Buffer.from(rewardSol.prover).toString("hex")
    );
    console.log(
      "Route sol destination hex: ",
      Buffer.from(routeSol.calls[0].destination).toString("hex")
    );
    console.log(
      "Route sol calldata hex: ",
      Buffer.from(routeSol.calls[0].calldata).toString("hex")
    );
    console.log(
      "Route sol inbox hex: ",
      Buffer.from(routeSol.inbox).toString("hex")
    );

    console.log("SVM passed call: ", callsSol);
    console.log("SVM route tokens: ", routeSolTokenArg);
    console.log("SVM reward tokens: ", rewardSolTokenArg);
    console.log("SVM route: ", routeSol);
    console.log("SVM reward: ", rewardSol);

    const intent = PublicKey.findProgramAddressSync(
      [Buffer.from("intent"), intentHashBytes],
      program.programId
    )[0];

    const vault = PublicKey.findProgramAddressSync(
      [Buffer.from("reward"), intentHashBytes, mockSvmUsdcMint.toBytes()],
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
          mockSvmUsdcMint,
          creatorSvm.publicKey
        ),
        vault,
        mint: mockSvmUsdcMint,
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
    expect(intentAccount.status.funded).to.be.true;
  });

  it("Fulfil intent on EVM", async () => {
    const solverEvmAddress = await solverEvm.getAddress();

    await usdc.connect(solverEvm).approve(INBOX_ADDRESS_TESTNET, usdcAmount(5));

    await inbox.fulfillAndProve(
      route,
      rewardHashHex,
      solverEvmAddress,
      intentHashHex,
      STORAGE_PROVER_ADDRESS_TESTNET,
      "0x",
      { gasLimit: 1_000_000 }
    );

    const fulfilledMappingSlot = await inbox.fulfilled(intentHashHex);
    expect(fulfilledMappingSlot).to.equal(solverEvmAddress);
  });

  it.skip("Claim intent on Solana", async () => {
    const intent = PublicKey.findProgramAddressSync(
      [Buffer.from("intent"), intentHashBytes],
      program.programId
    )[0];

    const vault = PublicKey.findProgramAddressSync(
      [Buffer.from("reward"), intentHashBytes, mockSvmUsdcMint.toBytes()],
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
          mockSvmUsdcMint,
          creatorSvm.publicKey
        ),
        mint: mockSvmUsdcMint,
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
