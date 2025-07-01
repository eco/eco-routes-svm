/*
 *     1. Publish + Fund intent on Solana
 *     2. Fulfil intent on EVM
 *     3. Claim on Solana
 */
import "dotenv/config";
import { AnchorProvider, BN, Program, utils as anchorUtils } from "@coral-xyz/anchor";
import * as anchor from "@coral-xyz/anchor";
import { Connection, PublicKey, SystemProgram, VersionedTransaction, TransactionMessage, LAMPORTS_PER_SOL } from "@solana/web3.js";
import { TOKEN_2022_PROGRAM_ID, TOKEN_PROGRAM_ID, createAssociatedTokenAccount, getAssociatedTokenAddressSync } from "@solana/spl-token";
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
import { HyperProver, HyperProver__factory, Inbox, Inbox__factory, IntentSource, IntentSource__factory, TestERC20, TestERC20__factory } from "./evm-types";
import { addressToBytes32Hex, encodeTransfer, hex32ToNums } from "./evmUtils";
import ecoRoutesIdl from "../../target/idl/eco_routes.json";
import { Reward, Route } from "./evmUtils";

const creatorSvm = loadKeypairFromFile("../../keys/program_auth_mainnet.json");
const connection = new Connection(MAINNET_RPC, "confirmed");
const provider = new AnchorProvider(connection, new anchor.Wallet(creatorSvm), {
    commitment: "confirmed",
});
const program = new Program(ecoRoutesIdl as anchor.Idl, provider) as Program<EcoRoutes>;

const salt = (() => {
    const bytes = anchorUtils.bytes.utf8.encode("svm-eveg55s7sasamsms".padEnd(32, "\0"));
    return bytes.slice(0, 32);
})();

describe("SVM -> EVM e2e", () => {
    const saltHex = "0x" + Buffer.from(salt).toString("hex");
    const deadline = 1796627873;

    let usdc: TestERC20;
    let inbox: Inbox;
    let hyperProver: HyperProver;
    let intentSource: IntentSource;
    let l2Provider: ethers.JsonRpcProvider;
    let solverEvm!: Signer;
    let userEvm!: Signer;
    let evmTransferCalldata!: string;
    let intentHashHex!: string;
    let intentHashBytes!: Uint8Array;
    let rewardHashHex!: string;
    let svmUsdcMint: PublicKey = USDC_MINT;
    let route!: Route;
    let reward!: Reward;
    let routeForHash!: any;

    before("Test setup", async () => {
        l2Provider = new JsonRpcProvider(process.env.EVM_RPC);
        userEvm = new ethers.Wallet(process.env.USER_WALLET_PK!, l2Provider);
        solverEvm = new ethers.Wallet(process.env.PK_SOLVER!, l2Provider);

        intentSource = IntentSource__factory.connect(INTENT_SOURCE_ADDRESS, solverEvm);
        usdc = TestERC20__factory.connect(USDC_ADDRESS_MAINNET, solverEvm);
        inbox = Inbox__factory.connect(INBOX_ADDRESS, solverEvm);
        hyperProver = HyperProver__factory.connect(HYPER_PROVER_ADDRESS, solverEvm);

        const evmCallTransferAmount = BigInt(usdcAmount(2));
        evmTransferCalldata = encodeTransfer(await userEvm.getAddress(), Number(evmCallTransferAmount));
        const transferUsdcEvmCall = {
            target: USDC_ADDRESS_MAINNET,
            data: evmTransferCalldata,
            value: BigInt(0),
        };

        const routeTokens = [
            {
                token: USDC_ADDRESS_MAINNET,
                amount: BigInt(usdcAmount(2)),
            },
        ];

        const rewardTokens = [
            {
                token: svmAddressToHex(svmUsdcMint),
                amount: BigInt(usdcAmount(2)),
            },
        ];

        const calls = [transferUsdcEvmCall];

        route = {
            salt: saltHex,
            source: SOLANA_DOMAIN_ID,
            destination: EVM_DOMAIN_ID,
            inbox: INBOX_ADDRESS,
            tokens: routeTokens,
            calls,
        };

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
            deadline: BigInt(deadline),
            nativeValue: BigInt(0.015 * LAMPORTS_PER_SOL),
            tokens: rewardTokens,
        };

        const { intentHash, rewardHash } = await intentSource[
            "getIntentHash(((bytes32,uint256,uint256,bytes32,(bytes32,uint256)[],(bytes32,bytes,uint256)[]),(bytes32,bytes32,uint256,uint256,(bytes32,uint256)[])))"
        ]({
            route: routeForHash,
            reward,
        });

        intentHashHex = intentHash;
        rewardHashHex = rewardHash;
        intentHashBytes = ethers.getBytes(intentHash);

        expect(intentHashBytes.length).equals(32);
    });

    it("Publish + Fund intent on Solana", async () => {
        const executionAuthority = PublicKey.findProgramAddressSync([Buffer.from("execution_authority"), salt], program.programId)[0];
        const intent = PublicKey.findProgramAddressSync([Buffer.from("intent"), intentHashBytes], program.programId)[0];
        const vault = PublicKey.findProgramAddressSync([Buffer.from("reward"), intentHashBytes, svmUsdcMint.toBytes()], program.programId)[0];

        const executionAuthorityAta = getAssociatedTokenAddressSync(svmUsdcMint, executionAuthority, true, TOKEN_2022_PROGRAM_ID);
        const executionAuthorityAtaData = await connection.getAccountInfo(executionAuthorityAta);
        if (!executionAuthorityAtaData) {
            await createAssociatedTokenAccount(
                connection,
                creatorSvm,
                svmUsdcMint,
                executionAuthority,
                { commitment: "confirmed" },
                TOKEN_2022_PROGRAM_ID,
                undefined,
                true
            );
        }
        const routeSolTokenArg = [
            {
                token: hex32ToNums(addressToBytes32Hex(USDC_ADDRESS_MAINNET)),
                amount: new BN(usdcAmount(2)),
            },
        ];

        const rewardSolTokenArg = [
            {
                token: Array.from(svmUsdcMint.toBytes()),
                amount: new BN(usdcAmount(2)),
            },
        ];

        const destinationSol = Array.from(Buffer.from(ethers.getBytes(addressToBytes32Hex(USDC_ADDRESS_MAINNET))));
        const calldataSol = Buffer.from(ethers.getBytes(evmTransferCalldata));
        const callsSol = [
            {
                destination: destinationSol,
                calldata: calldataSol,
            },
        ];

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
            nativeAmount: new BN(0.015 * LAMPORTS_PER_SOL),
            deadline: new BN(reward.deadline.toString()),
        };

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

        const publishIntentTxSignature = await connection.sendRawTransaction(publishIntentTx.serialize());
        await connection.confirmTransaction(
            {
                signature: publishIntentTxSignature,
                blockhash: blockhash.blockhash,
                lastValidBlockHeight: blockhash.lastValidBlockHeight,
            },
            "confirmed"
        );
        console.log("Publish Intent SVM tx signature: ", publishIntentTxSignature);

        const intentAccountInfo = await connection.getAccountInfo(intent);
        expect(intentAccountInfo.data.length).to.be.greaterThan(0);

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
                funderToken: getAssociatedTokenAddressSync(svmUsdcMint, creatorSvm.publicKey, true, TOKEN_2022_PROGRAM_ID),
                vault,
                mint: svmUsdcMint,
                tokenProgram: TOKEN_2022_PROGRAM_ID,
            })
            .instruction();

        const fundNativeIx = await program.methods
            .fundIntentNative({
                intentHash: Array.from(intentHashBytes),
            })
            .accountsStrict({
                intent,
                systemProgram: SystemProgram.programId,
                funder: creatorSvm.publicKey,
            })
            .instruction();

        blockhash = await connection.getLatestBlockhash();
        let fundIntentTx = new VersionedTransaction(
            new TransactionMessage({
                payerKey: creatorSvm.publicKey,
                recentBlockhash: blockhash.blockhash,
                instructions: [fundSplIx, fundNativeIx],
            }).compileToV0Message()
        );
        fundIntentTx.sign([creatorSvm]);

        const fundIntentTxSignature = await connection.sendRawTransaction(fundIntentTx.serialize());
        await connection.confirmTransaction(
            {
                signature: fundIntentTxSignature,
                blockhash: blockhash.blockhash,
                lastValidBlockHeight: blockhash.lastValidBlockHeight,
            },
            "confirmed"
        );
        console.log("Fund Intent SVM tx signature: ", fundIntentTxSignature);

        const intentAccount = await program.account.intent.fetch(intent);
        console.log("Intent account status:", intentAccount.status);
    });

    it("Fulfill intent on EVM", async () => {
        const usdcApproveTx = await usdc.connect(solverEvm).approve(INBOX_ADDRESS, usdcAmount(10));
        await usdcApproveTx.wait(10);

        const sourceChainProver = ethers.zeroPadValue(svmAddressToHex(ECO_ROUTES_ID_MAINNET), 32);
        const data = ethers.AbiCoder.defaultAbiCoder().encode(["bytes32", "bytes", "address"], [sourceChainProver, "0x", ethers.ZeroAddress]);

        const requiredFee = await hyperProver.fetchFee(SOLANA_DOMAIN_ID, [intentHashHex], [ethers.zeroPadValue(ethers.ZeroAddress, 32)], data);

        // add 5% to the fee (to be safe)
        const buffer = requiredFee / BigInt(20) > ethers.parseEther("0.0005") ? requiredFee / BigInt(20) : ethers.parseEther("0.0005");

        const fulfillTx = await inbox.fulfillAndProve(route, rewardHashHex, svmAddressToHex(creatorSvm.publicKey), intentHashHex, HYPER_PROVER_ADDRESS, data, {
            gasLimit: 900_000,
            value: requiredFee + buffer,
        });

        const fulfillTxReceipt = await fulfillTx.wait(5);
        console.log("Fulfill transaction hash:", fulfillTxReceipt.hash);

        const fulfilledMappingSlot = await inbox.fulfilled(intentHashHex);
        console.log("Fulfilled mapping result:", fulfilledMappingSlot);
    });

    it.skip("Claim intent on Solana", async () => {
        const intent = PublicKey.findProgramAddressSync([Buffer.from("intent"), intentHashBytes], program.programId)[0];
        const vault = PublicKey.findProgramAddressSync([Buffer.from("reward"), intentHashBytes, svmUsdcMint.toBytes()], program.programId)[0];

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
                claimerToken: getAssociatedTokenAddressSync(svmUsdcMint, creatorSvm.publicKey),
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

        const claimIntentTxSignature = await connection.sendRawTransaction(claimIntentTx.serialize());
        await connection.confirmTransaction(
            {
                signature: claimIntentTxSignature,
                blockhash: blockhash.blockhash,
                lastValidBlockHeight: blockhash.lastValidBlockHeight,
            },
            "confirmed"
        );

        const intentAccount = await program.account.intent.fetch(intent);
        const claimed = intentAccount.status.claimed[0];
        expect(claimed).to.be.true;
    });
});
