/*
 *     1. Publish + Fund intent on EVM
 *     2. Fulfil intent on SVM
 *     3. Claim on EVM
 */
import "dotenv/config";
import {
  Connection,
  Keypair,
  PublicKey,
  SystemProgram,
  VersionedTransaction,
  TransactionMessage,
  ComputeBudgetProgram,
  TransactionInstruction,
} from "@solana/web3.js";
import {
  AnchorProvider,
  BN,
  Program,
  utils as anchorUtils,
} from "@coral-xyz/anchor";
import * as anchor from "@coral-xyz/anchor";
import {
  createAssociatedTokenAccount,
  createMint,
  createTransferCheckedInstruction,
  getAssociatedTokenAddressSync,
  mintTo,
  TOKEN_2022_PROGRAM_ID,
  TOKEN_PROGRAM_ID,
} from "@solana/spl-token";
import { expect } from "chai";
import { ethers, JsonRpcProvider, Signer } from "ethers";
import {
  TestERC20__factory,
  TestProver__factory,
  TestERC20,
  TestProver,
  IntentSource__factory,
  IntentSource,
} from "./evm-types";
import { EcoRoutes } from "../../target/types/eco_routes";
import {
  TESTNET_RPC,
  MAILBOX_ID_TESTNET,
  SPL_NOOP_ID,
  SOLANA_DOMAIN_ID,
  EVM_DOMAIN_ID,
  TEST_USDC_ADDRESS_TESTNET,
  INTENT_SOURCE_ADDRESS_TESTNET,
  STORAGE_PROVER_ADDRESS_TESTNET,
  INBOX_ADDRESS_TESTNET,
  USDC_DECIMALS,
} from "./constants";
import {
  evmUsdcAmount,
  Route,
  Reward,
  addressToBytes32Hex,
  hex32ToBytes,
  hex32ToNums,
} from "./evmUtils";
import {
  loadKeypairFromFile,
  svmAddressToHex,
  usdcAmount,
  wrapIxFull,
  wrapIxHeaderOnly,
} from "./utils";
import ecoRoutesIdl from "../../target/idl/eco_routes.json";

const solver = loadKeypairFromFile("../../keys/program_auth.json"); // SVM solver key
const connection = new Connection(TESTNET_RPC, "confirmed");
const provider = new AnchorProvider(connection, new anchor.Wallet(solver), {
  commitment: "confirmed",
});
const program = new Program(
  ecoRoutesIdl as anchor.Idl,
  provider
) as Program<EcoRoutes>;

let intentHashBytes!: Uint8Array;
let route!: Route;
let reward!: Reward;

const salt = (() => {
  const bytes = anchorUtils.bytes.utf8.encode("evm-svm-e2e".padEnd(32, "\0"));
  return bytes.slice(0, 32);
})();
const saltHex = "0x" + Buffer.from(salt).toString("hex");

describe("EVM → SVM e2e", () => {
  let usdc: TestERC20;
  let intentSource: IntentSource;
  let testProver: TestProver;
  let creatorEvm!: Signer;
  let solverEvm!: Signer;
  let intentHashHex!: string;
  let routeHashHex!: string;
  let mockSvmUsdcMint: PublicKey;

  let testReceiver: Keypair = Keypair.generate();
  let transferTokenIx: TransactionInstruction;

  before("creates a testet usdc and mints to solver", async () => {
    // Route token mint
    const usdcMint = Keypair.generate();
    mockSvmUsdcMint = usdcMint.publicKey;

    await createMint(
      connection,
      solver,
      solver.publicKey,
      null,
      USDC_DECIMALS,
      usdcMint,
      {
        commitment: "confirmed",
      }
    );

    const ata = await createAssociatedTokenAccount(
      connection,
      solver,
      mockSvmUsdcMint,
      solver.publicKey,
      {
        commitment: "confirmed",
      }
    );

    await mintTo(
      connection,
      solver,
      mockSvmUsdcMint,
      ata,
      solver,
      usdcAmount(1000),
      [],
      {
        commitment: "confirmed",
      }
    );
  });

  it("publishes & funds an intent on EVM", async () => {
    const l2Provider = new JsonRpcProvider(process.env.RPC_SEPOLIA);
    creatorEvm = new ethers.Wallet(process.env.PK_CREATOR!, l2Provider);
    solverEvm = new ethers.Wallet(process.env.PK_SOLVER!, l2Provider);

    // mock USDC
    usdc = TestERC20__factory.connect(TEST_USDC_ADDRESS_TESTNET, creatorEvm);

    // IntentSource contract
    intentSource = IntentSource__factory.connect(
      INTENT_SOURCE_ADDRESS_TESTNET,
      creatorEvm
    );

    // TestProver and give its address to reward.prover
    testProver = TestProver__factory.connect(
      STORAGE_PROVER_ADDRESS_TESTNET,
      creatorEvm
    );

    const intentSourceAddress = await intentSource.getAddress();

    const amountU64 = usdcAmount(5); // 5_000_000
    const amountU256 = BigInt(amountU64);

    const executionAuthority = PublicKey.findProgramAddressSync(
      [Buffer.from("execution_authority"), salt],
      program.programId
    )[0];
    const executionAuthortiyAta = getAssociatedTokenAddressSync(
      mockSvmUsdcMint,
      executionAuthority,
      true
    );

    const testReceiverAta = getAssociatedTokenAddressSync(
      mockSvmUsdcMint,
      testReceiver.publicKey
    );

    await createAssociatedTokenAccount(
      connection,
      solver,
      mockSvmUsdcMint,
      testReceiver.publicKey
    );

    transferTokenIx = createTransferCheckedInstruction(
      executionAuthortiyAta,
      mockSvmUsdcMint,
      testReceiverAta,
      executionAuthority,
      usdcAmount(5),
      USDC_DECIMALS
    );

    transferTokenIx.keys.forEach((k) => {
      if (k.pubkey.equals(executionAuthority)) {
        k.isSigner = true;
        k.isWritable = true; // must be writable – the program mutates it
      }
    });

    console.log("transfer token ix before :", transferTokenIx);

    // createAtaSvmCall = wrapSvmCallIx(createAtaIx);
    const transferCheckedSvmCall = wrapIxFull(transferTokenIx);
    transferTokenIx.keys.forEach((k) => {
      if (k.pubkey.equals(executionAuthority)) {
        // remove it for SVM ix so that we don't have to sign the tx with this pda
        k.isSigner = false;
      }
    });

    const routeTokens = [
      {
        token: svmAddressToHex(mockSvmUsdcMint),
        amount: amountU256,
      },
    ];

    const rewardTokens = [
      {
        token: addressToBytes32Hex(TEST_USDC_ADDRESS_TESTNET),
        amount: amountU256,
      },
    ];

    const calls = [
      {
        target:
          "0x" +
          Buffer.from(transferCheckedSvmCall.destination).toString("hex"),
        data:
          "0x" + Buffer.from(transferCheckedSvmCall.calldata).toString("hex"),
        value: BigInt(0),
      },
    ];

    route = {
      salt: saltHex,
      source: EVM_DOMAIN_ID,
      destination: SOLANA_DOMAIN_ID,
      inbox: addressToBytes32Hex(INBOX_ADDRESS_TESTNET),
      tokens: routeTokens,
      calls,
    };

    reward = {
      creator: addressToBytes32Hex(await creatorEvm.getAddress()),
      prover: addressToBytes32Hex(STORAGE_PROVER_ADDRESS_TESTNET),
      deadline: BigInt(1000000000000),
      nativeValue: BigInt(0),
      tokens: rewardTokens,
    };

    console.log("EVM passed calls: ", calls);
    console.log("EVM route tokens: ", routeTokens);
    console.log("EVM reward tokens: ", rewardTokens);
    console.log("EVM route: ", route);
    console.log("EVM reward: ", reward);

    await usdc.approve(intentSourceAddress, evmUsdcAmount(10));
    const publishTx = await intentSource[
      "publishAndFund(((bytes32,uint256,uint256,bytes32,(bytes32,uint256)[],(bytes32,bytes,uint256)[]),(bytes32,bytes32,uint256,uint256,(bytes32,uint256)[])),bool)"
    ]({ route, reward }, true);
    await publishTx.wait();
    console.log(
      "vault address: ",
      await intentSource[
        "intentVaultAddress(((bytes32,uint256,uint256,bytes32,(bytes32,uint256)[],(bytes32,bytes,uint256)[]),(bytes32,bytes32,uint256,uint256,(bytes32,uint256)[])))"
      ]({ route, reward })
    );

    const { intentHash, routeHash, rewardHash } = await intentSource[
      "getIntentHash(((bytes32,uint256,uint256,bytes32,(bytes32,uint256)[],(bytes32,bytes,uint256)[]),(bytes32,bytes32,uint256,uint256,(bytes32,uint256)[])))"
    ]({
      route,
      reward,
    });
    // EVM one for TestProver
    intentHashHex = intentHash;
    routeHashHex = routeHash;

    // store the one Solana needs
    intentHashBytes = ethers.getBytes(intentHash);

    console.log("Intent hash bytes (for an SVM call): ", intentHashBytes);
    console.log("Route hash hex (EVM): ", routeHash);
    console.log("Reward hash hex (EVM): ", rewardHash);
    console.log("Intent hash hex (EVM): ", intentHashHex);

    expect(intentHashBytes.length).equals(32);
  });

  it("fulfills intent on Solana", async () => {
    const executionAuthority = PublicKey.findProgramAddressSync(
      [Buffer.from("execution_authority"), salt],
      program.programId
    )[0];

    const dispatchAuthority = PublicKey.findProgramAddressSync(
      [Buffer.from("dispatch_authority")],
      program.programId
    )[0];

    const intentFulfillmentMarker = PublicKey.findProgramAddressSync(
      [Buffer.from("intent_fulfillment_marker"), intentHashBytes],
      program.programId
    )[0];

    const outboxPda = PublicKey.findProgramAddressSync(
      [Buffer.from("hyperlane"), Buffer.from("-"), Buffer.from("outbox")],
      MAILBOX_ID_TESTNET
    )[0];

    const uniqueMessage = Keypair.generate();

    const dispatchedMessagePda = PublicKey.findProgramAddressSync(
      [
        Buffer.from("hyperlane"),
        Buffer.from("-"),
        Buffer.from("dispatched_message"),
        Buffer.from("-"),
        uniqueMessage.publicKey.toBuffer(),
      ],
      MAILBOX_ID_TESTNET
    )[0];

    const amountBN = new BN(usdcAmount(5));
    const evmUsdcAddress = await usdc.getAddress();

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
        solver,
        mockSvmUsdcMint,
        executionAuthority,
        { commitment: "confirmed" },
        undefined,
        undefined,
        true
      );
    }

    const solverAta = getAssociatedTokenAddressSync(
      mockSvmUsdcMint,
      solver.publicKey
    );

    const routeSolTokenArg = [
      {
        token: Array.from(mockSvmUsdcMint.toBytes()),
        amount: amountBN,
      },
    ];

    const rewardSolTokenArg = [
      {
        token: hex32ToNums(addressToBytes32Hex(evmUsdcAddress)),
        amount: amountBN,
      },
    ];

    const lightTransferCheckedSvmCall = wrapIxHeaderOnly(transferTokenIx);

    const calls = [
      {
        destination: Array.from(
          Buffer.from(lightTransferCheckedSvmCall.destination)
        ),
        calldata: Buffer.from(lightTransferCheckedSvmCall.calldata),
      },
    ];

    console.log("SVM passed calls: ", calls);

    const routeSolArg = {
      salt: Array.from(Buffer.from(saltHex.slice(2), "hex")),
      sourceDomainId: EVM_DOMAIN_ID,
      destinationDomainId: SOLANA_DOMAIN_ID,
      inbox: hex32ToNums(route.inbox),
      tokens: routeSolTokenArg,
      calls,
    };

    const rewardSolArg = {
      creator: new PublicKey(
        hex32ToBytes(addressToBytes32Hex(await creatorEvm.getAddress()))
      ),
      prover: hex32ToNums(addressToBytes32Hex(STORAGE_PROVER_ADDRESS_TESTNET)),
      tokens: rewardSolTokenArg,
      nativeAmount: new BN(0),
      deadline: new BN(1000000000000),
    };

    console.log("SVM passed call: ", calls);
    console.log("SVM route tokens: ", routeSolTokenArg);
    console.log("SVM reward tokens: ", rewardSolTokenArg);
    console.log("SVM route: ", routeSolArg);
    console.log("SVM reward: ", rewardSolArg);

    let remainingAccounts = [
      { pubkey: mockSvmUsdcMint, isSigner: false, isWritable: false },
      { pubkey: solverAta, isSigner: false, isWritable: true },
      { pubkey: executionAuthorityAta, isSigner: false, isWritable: true },
    ];

    transferTokenIx.keys.forEach((key) => {
      remainingAccounts.push({
        pubkey: key.pubkey,
        isSigner: key.pubkey === executionAuthority ? false : key.isSigner,
        isWritable: key.isWritable,
      });
    });

    remainingAccounts[remainingAccounts.length - 1].isSigner = false;

    const fulfillIx = await program.methods
      .fulfillIntent({
        intentHash: Array.from(intentHashBytes),
        route: routeSolArg,
        reward: rewardSolArg,
      })
      .accountsStrict({
        payer: solver.publicKey,
        solver: solver.publicKey,
        executionAuthority,
        dispatchAuthority,
        mailboxProgram: MAILBOX_ID_TESTNET,
        outboxPda,
        splNoopProgram: SPL_NOOP_ID,
        uniqueMessage: uniqueMessage.publicKey,
        intentFulfillmentMarker,
        dispatchedMessagePda,
        splTokenProgram: TOKEN_PROGRAM_ID,
        splToken2022Program: TOKEN_2022_PROGRAM_ID,
        systemProgram: SystemProgram.programId,
      })
      .remainingAccounts(remainingAccounts)
      .instruction();

    console.log("Fulfill tx: ", fulfillIx);

    try {
      let blockhash = await connection.getLatestBlockhash();
      const fulfillTx = new VersionedTransaction(
        new TransactionMessage({
          payerKey: solver.publicKey,
          recentBlockhash: blockhash.blockhash,
          instructions: [
            ComputeBudgetProgram.setComputeUnitLimit({
              units: 1_000_000,
            }),
            ComputeBudgetProgram.setComputeUnitPrice({
              microLamports: 300_000,
            }),
            fulfillIx,
          ],
        }).compileToV0Message()
      );

      fulfillTx.sign([solver, uniqueMessage]);
      const fulfillTxSignature = await connection.sendRawTransaction(
        fulfillTx.serialize()
      );

      await connection.confirmTransaction(
        { signature: fulfillTxSignature, ...blockhash },
        "confirmed"
      );

      console.log("fulfil tx sig :", fulfillTxSignature);
      console.log(
        "msg ID sent: ",
        Buffer.from(dispatchedMessagePda.toBytes()).toString("hex")
      );
    } catch (error) {
      console.log("ERROR: ", error);
    }

    // TODO: uncomment once we have an oracle on a destinatino domain
    // blockhash = await connection.getLatestBlockhash();
    // const payForGasIx = buildPayForGasIx(
    //   solver.publicKey,
    //   dispatchedMessagePda,
    //   uniqueMessage.publicKey
    // );

    // const payForGasTx = new VersionedTransaction(
    //   new TransactionMessage({
    //     payerKey: solver.publicKey,
    //     recentBlockhash: blockhash.blockhash,
    //     instructions: [
    //       ComputeBudgetProgram.setComputeUnitLimit({
    //         units: 200_000,
    //       }),
    //       ComputeBudgetProgram.setComputeUnitPrice({
    //         microLamports: 300_000,
    //       }),
    //       payForGasIx,
    //     ],
    //   }).compileToV0Message()
    // );

    // payForGasTx.sign([solver, uniqueMessage]);
    // const payForGasTxSignature = await connection.sendRawTransaction(
    //   payForGasTx.serialize()
    // );

    // await connection.confirmTransaction(
    //   { signature: payForGasTxSignature, ...blockhash },
    //   "confirmed"
    // );

    // console.log("pay for gas tx sig :", payForGasTxSignature);

    const accountInfo = await connection.getAccountInfo(
      intentFulfillmentMarker
    );
    expect(accountInfo?.data.length).to.be.greaterThan(0);
  });

  it.skip("proves and withdraws on EVM", async () => {
    console.log("Waiting for the message to land...");
    await new Promise((resolve) => setTimeout(resolve, 20_000));
    const l2Provider = new JsonRpcProvider(process.env.RPC_SEPOLIA);
    const solverEvmAddress = await solverEvm.getAddress();
    const intentSourceAddress = await intentSource.getAddress();

    expect(
      await intentSource[
        "isIntentFunded(((bytes32,uint256,uint256,bytes32,(bytes32,uint256)[],(bytes32,bytes,uint256)[]),(bytes32,bytes32,uint256,uint256,(bytes32,uint256)[])))"
      ]({ route, reward })
    ).to.be.true;

    // simulate prover writing the mapping
    let addProvenIntentCall = await testProver
      .connect(creatorEvm)
      .addProvenIntent(intentHashHex, solverEvmAddress);
    await addProvenIntentCall.wait();

    // let proveIntentCall = await testProver
    //   .connect(creatorEvm)
    //   .prove(
    //     solverEvmAddress,
    //     SOLANA_DOMAIN_ID,
    //     [intentHashHex],
    //     [solverEvmAddress],
    //     "0x",
    //     { value: BigInt(100000000) }
    //   );
    // await proveIntentCall.wait();

    console.log(
      "prover mapping :",
      await testProver.provenIntents(intentHashHex)
    );

    // vault address
    const vaultAddress = await intentSource[
      "intentVaultAddress(((bytes32,uint256,uint256,bytes32,(bytes32,uint256)[],(bytes32,bytes,uint256)[]),(bytes32,bytes32,uint256,uint256,(bytes32,uint256)[])))"
    ]({
      route,
      reward,
    });

    // withdraw
    await intentSource
      .connect(solverEvm)
      [
        "withdrawRewards(bytes32,(bytes32,bytes32,uint256,uint256,(bytes32,uint256)[]))"
      ](routeHashHex, reward);

    // after balances
    const solverBalanceAfter = await usdc.balanceOf(solverEvmAddress);
    expect(solverBalanceAfter).to.equal(evmUsdcAmount(5));

    // vault should be self-destructed, hence balance 0
    expect(await l2Provider.provider.getCode(vaultAddress)).to.equal("0x");
  });
});
