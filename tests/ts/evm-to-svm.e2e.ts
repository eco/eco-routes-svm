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
import { AnchorProvider, BN, Program, utils as anchorUtils } from "@coral-xyz/anchor";
import * as anchor from "@coral-xyz/anchor";
import {
    createAssociatedTokenAccount,
    createTransferCheckedInstruction,
    getAssociatedTokenAddressSync,
    TOKEN_2022_PROGRAM_ID,
    TOKEN_PROGRAM_ID,
} from "@solana/spl-token";
import { expect } from "chai";
import { ethers, getBytes, JsonRpcProvider, keccak256, Signer } from "ethers";
import { IntentSource__factory, IntentSource, TestERC20, TestERC20__factory } from "./evm-types";
import { EcoRoutes } from "../../target/types/eco_routes";
import {
    MAINNET_RPC,
    MAILBOX_ID_MAINNET,
    SPL_NOOP_ID,
    SOLANA_DOMAIN_ID,
    EVM_DOMAIN_ID,
    INTENT_SOURCE_ADDRESS,
    HYPER_PROVER_ADDRESS,
    INBOX_ADDRESS,
    USDC_DECIMALS,
    USDC_MINT,
    DISPATCHED_MSG_PDA_HEADER_LEN,
    USDC_ADDRESS_MAINNET,
} from "./constants";
import { Route, Reward, addressToBytes32Hex, hex32ToBytes, hex32ToNums } from "./evmUtils";
import { buildPayForGasIx, loadKeypairFromFile, svmAddressToHex, usdcAmount, wrapIxFull, wrapIxHeaderOnly } from "./utils";
import ecoRoutesIdl from "../../target/idl/eco_routes.json";

const solver = loadKeypairFromFile("../../keys/program_auth_mainnet.json"); // SVM solver key
const connection = new Connection(MAINNET_RPC, "confirmed");
const provider = new AnchorProvider(connection, new anchor.Wallet(solver), {
    commitment: "confirmed",
});
const program = new Program(ecoRoutesIdl as anchor.Idl, provider) as Program<EcoRoutes>;

let intentHashBytes!: Uint8Array;
let route!: Route;
let reward!: Reward;

const salt = (() => {
    // Use timestamp as salt to avoid account conflicts while being reproducible
    const timestamp = Date.now().toString();
    const bytes = anchorUtils.bytes.utf8.encode(timestamp.padEnd(32, "\0"));
    return bytes.slice(0, 32);
})();

describe("EVM → SVM e2e", () => {
    const deadline = 211160000;
    const saltHex = "0x" + Buffer.from(salt).toString("hex");
    const routeTokenAmount = 1;
    const rewardTokenAmount = 1;
    const rewardNativeWei = ethers.parseEther("0.0001"); // 1 * 10^14 wei = 0.0001 ETH

    let intentSource: IntentSource;
    let usdc: TestERC20;
    let creatorEvm!: Signer;
    let solverEvm!: Signer;
    let intentHashHex!: string;
    let routeHashHex!: string;
    let l2Provider: ethers.JsonRpcProvider;
    let svmUsdcMint: PublicKey = USDC_MINT;
    let testReceiver: PublicKey = new PublicKey(process.env.SOLANA_TEST_RECEIVER!);
    let transferTokenIx: TransactionInstruction;

    before("Test setup", async () => {
        l2Provider = new JsonRpcProvider(process.env.EVM_RPC);

        creatorEvm = new ethers.Wallet(process.env.PK_CREATOR!, l2Provider);
        solverEvm = new ethers.Wallet(process.env.PK_SOLVER!, l2Provider);

        intentSource = IntentSource__factory.connect(INTENT_SOURCE_ADDRESS, creatorEvm);
        usdc = TestERC20__factory.connect(USDC_ADDRESS_MAINNET, solverEvm);

        const executionAuthority = PublicKey.findProgramAddressSync([Buffer.from("execution_authority"), salt], program.programId)[0];
        const executionAuthortiyAta = getAssociatedTokenAddressSync(svmUsdcMint, executionAuthority, true, TOKEN_2022_PROGRAM_ID);

        const testReceiverAta = getAssociatedTokenAddressSync(svmUsdcMint, testReceiver, true, TOKEN_2022_PROGRAM_ID);

        transferTokenIx = createTransferCheckedInstruction(
            executionAuthortiyAta,
            svmUsdcMint,
            testReceiverAta,
            executionAuthority,
            usdcAmount(routeTokenAmount),
            USDC_DECIMALS,
            undefined,
            TOKEN_2022_PROGRAM_ID
        );

        transferTokenIx.keys.forEach((k) => {
            if (k.pubkey.equals(executionAuthority)) {
                k.isSigner = true;
                k.isWritable = true; // must be writable – the program mutates it
            }
        });

        const transferCheckedSvmCall = wrapIxFull(transferTokenIx);
        transferTokenIx.keys.forEach((k) => {
            if (k.pubkey.equals(executionAuthority)) {
                // remove it for SVM ix so that we don't have to sign the tx with this pda
                k.isSigner = false;
            }
        });

        const routeTokens = [
            {
                token: svmAddressToHex(svmUsdcMint),
                amount: BigInt(usdcAmount(routeTokenAmount)),
            },
        ];

        const calls = [
            {
                target: "0x" + Buffer.from(transferCheckedSvmCall.destination).toString("hex"),
                data: "0x" + Buffer.from(transferCheckedSvmCall.calldata).toString("hex"),
                value: BigInt(0),
            },
        ];

        route = {
            salt: saltHex,
            source: EVM_DOMAIN_ID,
            destination: SOLANA_DOMAIN_ID,
            inbox: addressToBytes32Hex(INBOX_ADDRESS),
            tokens: routeTokens,
            calls,
        };

        reward = {
            creator: addressToBytes32Hex(await creatorEvm.getAddress()),
            prover: addressToBytes32Hex(HYPER_PROVER_ADDRESS),
            deadline: BigInt(deadline),
            nativeValue: rewardNativeWei,
            tokens: [
                {
                    token: addressToBytes32Hex(USDC_ADDRESS_MAINNET),
                    amount: BigInt(usdcAmount(rewardTokenAmount)),
                },
            ],
        };

        const { intentHash, routeHash } = await intentSource[
            "getIntentHash(((bytes32,uint256,uint256,bytes32,(bytes32,uint256)[],(bytes32,bytes,uint256)[]),(bytes32,bytes32,uint256,uint256,(bytes32,uint256)[])))"
        ]({
            route,
            reward,
        });

        intentHashHex = intentHash;
        routeHashHex = routeHash;
        intentHashBytes = ethers.getBytes(intentHash);

        console.log("Intent hash hex (EVM): ", intentHashHex);
        expect(intentHashBytes.length).equals(32);
    });

    it("publishes & funds an intent on EVM", async () => {
        const usdcApproveTx = await usdc.connect(creatorEvm).approve(INTENT_SOURCE_ADDRESS, usdcAmount(1));
        await usdcApproveTx.wait(5);

        const publishTx = await intentSource[
            "publishAndFund(((bytes32,uint256,uint256,bytes32,(bytes32,uint256)[],(bytes32,bytes,uint256)[]),(bytes32,bytes32,uint256,uint256,(bytes32,uint256)[])),bool)"
        ]({ route, reward }, false, { value: rewardNativeWei });
        const publishTxReceipt = await publishTx.wait(5);

        console.log("Publish Intent EVM transaction hash: ", publishTxReceipt.hash);

        console.log("Intent was sucessfully published.");
    });

    it("fulfills intent on Solana", async () => {
        const uniqueMessage = Keypair.generate();

        const executionAuthority = PublicKey.findProgramAddressSync([Buffer.from("execution_authority"), salt], program.programId)[0];
        const dispatchAuthority = PublicKey.findProgramAddressSync([Buffer.from("dispatch_authority")], program.programId)[0];
        const intentFulfillmentMarker = PublicKey.findProgramAddressSync([Buffer.from("intent_fulfillment_marker"), intentHashBytes], program.programId)[0];
        const outboxPda = PublicKey.findProgramAddressSync([Buffer.from("hyperlane"), Buffer.from("-"), Buffer.from("outbox")], MAILBOX_ID_MAINNET)[0];
        const dispatchedMessagePda = PublicKey.findProgramAddressSync(
            [Buffer.from("hyperlane"), Buffer.from("-"), Buffer.from("dispatched_message"), Buffer.from("-"), uniqueMessage.publicKey.toBuffer()],
            MAILBOX_ID_MAINNET
        )[0];

        const executionAuthorityAta = getAssociatedTokenAddressSync(svmUsdcMint, executionAuthority, true, TOKEN_2022_PROGRAM_ID);
        const executionAuthorityAtaData = await connection.getAccountInfo(executionAuthorityAta);
        if (!executionAuthorityAtaData) {
            await createAssociatedTokenAccount(
                connection,
                solver,
                svmUsdcMint,
                executionAuthority,
                { commitment: "confirmed" },
                TOKEN_2022_PROGRAM_ID,
                undefined,
                true
            );
        }

        const solverAta = getAssociatedTokenAddressSync(svmUsdcMint, solver.publicKey, true, TOKEN_2022_PROGRAM_ID);
        const solverAtaData = await connection.getAccountInfo(solverAta);
        if (!solverAtaData) {
            await createAssociatedTokenAccount(connection, solver, svmUsdcMint, solver.publicKey, { commitment: "confirmed" }, TOKEN_2022_PROGRAM_ID);
        }

        const destinationAta = getAssociatedTokenAddressSync(svmUsdcMint, testReceiver, true, TOKEN_2022_PROGRAM_ID);
        const destinationAtaData = await connection.getAccountInfo(destinationAta);
        if (!destinationAtaData) {
            await createAssociatedTokenAccount(connection, solver, svmUsdcMint, testReceiver, { commitment: "confirmed" }, TOKEN_2022_PROGRAM_ID);
        }

        const routeSolTokenArg = [
            {
                token: Array.from(svmUsdcMint.toBytes()),
                amount: new BN(usdcAmount(routeTokenAmount)),
            },
        ];
        const lightTransferCheckedSvmCall = wrapIxHeaderOnly(transferTokenIx);
        const calls = [
            {
                destination: Array.from(Buffer.from(lightTransferCheckedSvmCall.destination)),
                calldata: Buffer.from(lightTransferCheckedSvmCall.calldata),
            },
        ];

        const routeSolArg = {
            salt: Array.from(Buffer.from(saltHex.slice(2), "hex")),
            sourceDomainId: EVM_DOMAIN_ID,
            destinationDomainId: SOLANA_DOMAIN_ID,
            inbox: hex32ToNums(route.inbox),
            tokens: routeSolTokenArg,
            calls,
        };

        const rewardSolArg = {
            creator: new PublicKey(hex32ToBytes(addressToBytes32Hex(await creatorEvm.getAddress()))),
            prover: hex32ToNums(addressToBytes32Hex(HYPER_PROVER_ADDRESS)),
            tokens: [
                {
                    token: hex32ToNums(addressToBytes32Hex(USDC_ADDRESS_MAINNET)),
                    amount: new BN(usdcAmount(rewardTokenAmount)),
                },
            ],
            nativeAmount: new BN(rewardNativeWei.toString()),
            deadline: new BN(deadline),
        };

        let remainingAccounts = [
            { pubkey: svmUsdcMint, isSigner: false, isWritable: false },
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
                claimant: Array.from(getBytes(addressToBytes32Hex(await solverEvm.getAddress()))),
                route: routeSolArg,
                reward: rewardSolArg,
            })
            .accountsStrict({
                payer: solver.publicKey,
                solver: solver.publicKey,
                executionAuthority,
                dispatchAuthority,
                mailboxProgram: MAILBOX_ID_MAINNET,
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

        let blockhash = await connection.getLatestBlockhash();
        try {
            const fulfillTx = new VersionedTransaction(
                new TransactionMessage({
                    payerKey: solver.publicKey,
                    recentBlockhash: blockhash.blockhash,
                    instructions: [
                        ComputeBudgetProgram.setComputeUnitLimit({
                            units: 1_000_000,
                        }),
                        ComputeBudgetProgram.setComputeUnitPrice({
                            microLamports: 150_000,
                        }),
                        fulfillIx,
                    ],
                }).compileToV0Message()
            );

            fulfillTx.sign([solver, uniqueMessage]);
            const fulfillTxSignature = await connection.sendRawTransaction(fulfillTx.serialize());

            await connection.confirmTransaction({ signature: fulfillTxSignature, ...blockhash }, "confirmed");

            console.log("Fulfill SVM transaction signature:", fulfillTxSignature);
            console.log("Hyperlane Message ID: ", Buffer.from(dispatchedMessagePda.toBytes()).toString("hex"));
        } catch (error) {
            console.log("Error during fulfillment:", error);
            throw error;
        }

        blockhash = await connection.getLatestBlockhash();

        try {
            const dispatchedMsgAccountInfo = await connection.getAccountInfo(dispatchedMessagePda);
            if (dispatchedMsgAccountInfo.data.length === 0) {
                throw new Error("Dispatched Msg PDA account not found.");
            }

            const dispatchedMsgBytes = dispatchedMsgAccountInfo.data.slice(DISPATCHED_MSG_PDA_HEADER_LEN + 1);
            const messageIdHex = keccak256(dispatchedMsgBytes);
            console.log("Dispatched message ID (hex):", messageIdHex);

            const messageIdBytes = getBytes(messageIdHex);
            const payForGasIx = buildPayForGasIx(solver.publicKey, Buffer.from(messageIdBytes), uniqueMessage.publicKey);

            const payForGasTx = new VersionedTransaction(
                new TransactionMessage({
                    payerKey: solver.publicKey,
                    recentBlockhash: blockhash.blockhash,
                    instructions: [
                        ComputeBudgetProgram.setComputeUnitLimit({
                            units: 200_000,
                        }),
                        ComputeBudgetProgram.setComputeUnitPrice({
                            microLamports: 300_000,
                        }),
                        payForGasIx,
                    ],
                }).compileToV0Message()
            );

            payForGasTx.sign([solver, uniqueMessage]);

            const payForGasTxSignature = await connection.sendRawTransaction(payForGasTx.serialize());
            await connection.confirmTransaction({ signature: payForGasTxSignature, ...blockhash }, "confirmed");
            console.log("IGP gas payment SVM tx signature: ", payForGasTxSignature);
        } catch (error) {
            console.error("Error during gas payment:", error);
            throw error;
        }

        const accountInfo = await connection.getAccountInfo(intentFulfillmentMarker);
        expect(accountInfo?.data.length).to.be.greaterThan(0);
    });

    it("withdraws on EVM", async () => {
        console.log("Waiting for the message to be delivered...");
        await new Promise((resolve) => setTimeout(resolve, 35_000));
        const solverAddress = await solverEvm.getAddress();

        const logBalances = async (label: string) => {
            const eth = await l2Provider.getBalance(solverAddress);
            const usdcB = await usdc.balanceOf(solverAddress);
            console.log(`${label}  —  ${ethers.formatEther(eth)} ETH  |  ${ethers.formatUnits(usdcB, 6)} USDC`);
        };

        await logBalances("Before withdrawReward");

        const vaultAddress = await intentSource[
            "intentVaultAddress(((bytes32,uint256,uint256,bytes32,(bytes32,uint256)[],(bytes32,bytes,uint256)[]),(bytes32,bytes32,uint256,uint256,(bytes32,uint256)[])))"
        ]({
            route,
            reward,
        });

        console.log("Vault ETH balance:", ethers.formatEther(await l2Provider.getBalance(vaultAddress)));
        console.log("Vault USDC balance:", ethers.formatUnits(await usdc.balanceOf(vaultAddress), 6));

        const withdrawTx = await intentSource
            .connect(solverEvm)
            ["withdrawRewards(bytes32,(bytes32,bytes32,uint256,uint256,(bytes32,uint256)[]))"](routeHashHex, reward, { gasLimit: 600_000 });
        await withdrawTx.wait(5);

        await logBalances("After withdrawReward");

        console.log("Withdraw reward tx signature: ", withdrawTx.signature);

        // vault should be self-destructed, hence balance 0
        expect(await l2Provider.provider.getCode(vaultAddress)).to.equal("0x");

        console.log("The reward was successfully withdrawn.");
    });
});
