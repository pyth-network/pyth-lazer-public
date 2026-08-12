/** biome-ignore-all lint/suspicious/noConsole: e2e test script */

import assert from "node:assert/strict";
import process from "node:process";

import { PythLazerClient } from "@pythnetwork/pyth-lazer-sdk";
import {
  BASE_FEE,
  Contract,
  Keypair,
  Networks,
  nativeToScVal,
  scValToNative,
  TransactionBuilder,
} from "@stellar/stellar-sdk";
import { Api, Server } from "@stellar/stellar-sdk/rpc";
import yargs from "yargs";
import { hideBin } from "yargs/helpers";

const NETWORKS = {
  mainnet: {
    explorer: "public",
    // Inclusion bid, not the resource fee: `prepareTransaction` adds the Soroban resource fee on
    // top but leaves this untouched, and mainnet ledgers run close to full, so a BASE_FEE bid gets
    // evicted and the transaction expires without ever reaching a ledger.
    inclusionFee: "1000000",
    networkPassphrase: Networks.PUBLIC,
    rpcUrl: "https://mainnet.sorobanrpc.com",
  },
  testnet: {
    explorer: "testnet",
    inclusionFee: BASE_FEE,
    networkPassphrase: Networks.TESTNET,
    rpcUrl: "https://soroban-testnet.stellar.org",
  },
} as const;
const PAYLOAD_MAGIC = 0x93_c7_d3_75;
const BTC_USD_FEED_ID = 1;

const CHANNEL_NAMES: Record<number, string> = {
  1: "RealTime",
  2: "FixedRate50ms",
  3: "FixedRate200ms",
  4: "FixedRate1000ms",
};

const {
  network: networkName,
  secret,
  "contract-id": contractIdArg,
} = await yargs(hideBin(process.argv))
  .option("network", {
    choices: Object.keys(NETWORKS) as (keyof typeof NETWORKS)[],
    default: "testnet" as const,
    description: "Stellar network the contract is deployed on.",
  })
  .option("secret", {
    description:
      "Stellar secret key (S...). Required on mainnet; on testnet, a new keypair is generated and friendbot-funded when omitted.",
    type: "string",
  })
  .option("contract-id", {
    demandOption: true,
    description:
      "Lazer contract ID to test against. Deploy separately using deploy.sh first.",
    type: "string",
  })
  .help()
  .parseAsync();

const network = NETWORKS[networkName];

// biome-ignore lint/style/noProcessEnv: e2e reads Lazer auth token from the environment
const { PYTH_LAZER_TOKEN } = process.env;
if (!PYTH_LAZER_TOKEN) {
  throw new Error(
    "'PYTH_LAZER_TOKEN' environment variable must be set to your Lazer auth token.",
  );
}

function readLeU16(buf: Buffer, offset: number): number {
  return buf.readUInt16LE(offset);
}

function readLeU32(buf: Buffer, offset: number): number {
  return buf.readUInt32LE(offset);
}

function readLeI64(buf: Buffer, offset: number): bigint {
  return buf.readBigInt64LE(offset);
}

function readLeU64(buf: Buffer, offset: number): bigint {
  return buf.readBigUInt64LE(offset);
}

type ParsedFeed = { id: number; price?: bigint };
type ParsedPayload = { magic: number; channelId: number; feeds: ParsedFeed[] };

/** Parse the verified payload, printing feed data and returning structured fields. */
function parsePayload(buf: Buffer): ParsedPayload {
  let offset = 0;

  // Magic
  const magic = readLeU32(buf, offset);
  offset += 4;
  if (magic !== PAYLOAD_MAGIC) {
    throw new Error(
      `Invalid payload magic: 0x${magic.toString(16)} (expected 0x${PAYLOAD_MAGIC.toString(16)})`,
    );
  }
  console.log(`  Payload magic: 0x${magic.toString(16)} (valid)`);

  // Timestamp (microseconds since epoch)
  const timestampUs = readLeU64(buf, offset);
  offset += 8;
  const timestampMs = Number(timestampUs / 1000n);
  console.log(
    `  Timestamp: ${timestampUs} us (${new Date(timestampMs).toISOString()})`,
  );

  // Channel
  const channelId = buf.readUInt8(offset);
  offset += 1;
  console.log(
    `  Channel: ${channelId} (${CHANNEL_NAMES[channelId] ?? "Unknown"})`,
  );

  // Number of feeds
  const numFeeds = buf.readUInt8(offset);
  offset += 1;
  console.log(`  Number of feeds: ${numFeeds}`);

  const feeds: ParsedFeed[] = [];

  for (let f = 0; f < numFeeds; f++) {
    const feedId = readLeU32(buf, offset);
    offset += 4;
    const numProps = buf.readUInt8(offset);
    offset += 1;
    console.log(`\n  Feed #${f}: id=${feedId}, properties=${numProps}`);

    const feed: ParsedFeed = { id: feedId };

    for (let p = 0; p < numProps; p++) {
      const propId = buf.readUInt8(offset);
      offset += 1;

      switch (propId) {
        case 0: {
          // Price (i64)
          const price = readLeI64(buf, offset);
          offset += 8;
          feed.price = price;
          console.log(
            `    [${propId}] Price: ${price}${price === 0n ? " (absent)" : ""}`,
          );
          break;
        }
        case 1: {
          // BestBidPrice (i64)
          const val = readLeI64(buf, offset);
          offset += 8;
          console.log(`    [${propId}] BestBidPrice: ${val}`);
          break;
        }
        case 2: {
          // BestAskPrice (i64)
          const val = readLeI64(buf, offset);
          offset += 8;
          console.log(`    [${propId}] BestAskPrice: ${val}`);
          break;
        }
        case 3: {
          // PublisherCount (u16)
          const val = readLeU16(buf, offset);
          offset += 2;
          console.log(`    [${propId}] PublisherCount: ${val}`);
          break;
        }
        case 4: {
          // Exponent (i16)
          const val = buf.readInt16LE(offset);
          offset += 2;
          console.log(`    [${propId}] Exponent: ${val}`);
          break;
        }
        case 5: {
          // Confidence (u64)
          const val = readLeU64(buf, offset);
          offset += 8;
          console.log(`    [${propId}] Confidence: ${val}`);
          break;
        }
        case 6: {
          // FundingRate (bool + i64)
          const exists = buf.readUInt8(offset);
          offset += 1;
          if (exists) {
            const val = readLeI64(buf, offset);
            offset += 8;
            console.log(`    [${propId}] FundingRate: ${val}`);
          } else {
            console.log(`    [${propId}] FundingRate: (absent)`);
          }
          break;
        }
        case 7: {
          // FundingTimestamp (bool + u64)
          const exists = buf.readUInt8(offset);
          offset += 1;
          if (exists) {
            const val = readLeU64(buf, offset);
            offset += 8;
            console.log(`    [${propId}] FundingTimestamp: ${val}`);
          } else {
            console.log(`    [${propId}] FundingTimestamp: (absent)`);
          }
          break;
        }
        case 8: {
          // FundingRateInterval (bool + u64)
          const exists = buf.readUInt8(offset);
          offset += 1;
          if (exists) {
            const val = readLeU64(buf, offset);
            offset += 8;
            console.log(`    [${propId}] FundingRateInterval: ${val}`);
          } else {
            console.log(`    [${propId}] FundingRateInterval: (absent)`);
          }
          break;
        }
        case 9: {
          // MarketSession (u16)
          const val = readLeU16(buf, offset);
          offset += 2;
          const sessions = [
            "Regular",
            "PreMarket",
            "PostMarket",
            "OverNight",
            "Closed",
          ];
          console.log(`    [${propId}] MarketSession: ${sessions[val] ?? val}`);
          break;
        }
        case 10: {
          // EmaPrice (i64)
          const val = readLeI64(buf, offset);
          offset += 8;
          console.log(`    [${propId}] EmaPrice: ${val}`);
          break;
        }
        case 11: {
          // EmaConfidence (u64)
          const val = readLeU64(buf, offset);
          offset += 8;
          console.log(`    [${propId}] EmaConfidence: ${val}`);
          break;
        }
        case 12: {
          // FeedUpdateTimestamp (bool + u64)
          const exists = buf.readUInt8(offset);
          offset += 1;
          if (exists) {
            const val = readLeU64(buf, offset);
            offset += 8;
            console.log(`    [${propId}] FeedUpdateTimestamp: ${val}`);
          } else {
            console.log(`    [${propId}] FeedUpdateTimestamp: (absent)`);
          }
          break;
        }
        default:
          console.log(`    [${propId}] Unknown property`);
      }
    }

    feeds.push(feed);
  }

  return { channelId, feeds, magic };
}

// --- Step 1: Set up Stellar keypair ---
const server = new Server(network.rpcUrl);
let keypair: Keypair;
if (secret) {
  keypair = Keypair.fromSecret(secret);
  console.log("=== Using provided keypair ===");
  console.log(`  Account: ${keypair.publicKey()}`);
} else if (networkName === "mainnet") {
  throw new Error("--secret is required on mainnet (there is no friendbot).");
} else {
  keypair = Keypair.random();
  console.log("=== Generating Stellar test keypair ===");
  console.log(`  Account: ${keypair.publicKey()}`);

  console.log("\n=== Funding account via Stellar friendbot ===");
  await server.requestAirdrop(keypair.publicKey());
  console.log("  Account funded.");
}
const accountId = keypair.publicKey();

// --- Step 2: Use the provided contract ID ---
const contractId = contractIdArg;
console.log(`\n=== Using Lazer contract: ${contractId} on ${networkName} ===`);

// --- Step 3: Fetch real price update from Pyth Lazer ---
console.log("\n=== Fetching real price update from Pyth Lazer ===");
const lazer = await PythLazerClient.create({
  token: PYTH_LAZER_TOKEN,
  webSocketPoolConfig: {},
});
let update: Buffer;

try {
  const response = await lazer.getLatestPrice({
    channel: "fixed_rate@200ms",
    formats: ["leEcdsa"],
    jsonBinaryEncoding: "hex",
    priceFeedIds: [BTC_USD_FEED_ID],
    properties: ["price"],
  });

  const hex = response.leEcdsa?.data;
  if (!hex) {
    console.error("  Response:", JSON.stringify(response, null, 2));
    throw new Error("No leEcdsa data in response");
  }
  update = Buffer.from(hex, "hex");

  console.log(`  Update size: ${update.length} bytes`);
  console.log(`  Update hex (first 80 chars): ${hex.slice(0, 80)}...`);
} finally {
  lazer.shutdown();
}

// --- Step 4: Verify the real update on-chain ---
console.log("\n=== Calling verify_update with real Lazer payload ===");
const account = await server.getAccount(accountId);
const contract = new Contract(contractId);
const tx = new TransactionBuilder(account, {
  fee: network.inclusionFee,
  networkPassphrase: network.networkPassphrase,
})
  .addOperation(
    contract.call("verify_update", nativeToScVal(update, { type: "bytes" })),
  )
  .setTimeout(30)
  .build();

const prepared = await server.prepareTransaction(tx);
prepared.sign(keypair);

const sendResult = await server.sendTransaction(prepared);
console.log(`  Transaction hash: ${sendResult.hash}`);
console.log(
  `  Stellar Explorer: https://stellar.expert/explorer/${network.explorer}/tx/${sendResult.hash}`,
);
if (sendResult.status === "ERROR") {
  console.error("  Send error:", JSON.stringify(sendResult.errorResult));
  throw new Error(`Transaction submission failed: ${sendResult.status}`);
}

const txResult = await server.pollTransaction(sendResult.hash);
if (txResult.status === Api.GetTransactionStatus.NOT_FOUND) {
  throw new Error(
    `Transaction ${sendResult.hash} was dropped before reaching a ledger, so nothing executed and no fee was charged. This usually means the ledger was full and the inclusion fee of ${network.inclusionFee} stroops was outbid; retry, raising it if it keeps happening.`,
  );
}
if (txResult.status !== Api.GetTransactionStatus.SUCCESS) {
  throw new Error(`Transaction did not succeed: ${txResult.status}`);
}

const { returnValue } = txResult;
if (!returnValue) {
  throw new Error("verify_update returned no value");
}

// verify_update returns soroban_sdk::Bytes; decode the ScVal directly to a Buffer.
const payload = scValToNative(returnValue) as Buffer;
console.log(`  Verified payload: ${payload.length} bytes`);

// --- Step 5: Parse the verified payload ---
console.log("\n=== Parsing verified payload ===");
const parsed = parsePayload(payload);

// --- Step 6: Structured assertions ---
console.log("\n=== Asserting verified payload ===");
assert.equal(parsed.magic, PAYLOAD_MAGIC, "payload magic mismatch");
assert.ok(
  CHANNEL_NAMES[parsed.channelId] !== undefined,
  `unexpected channel id: ${parsed.channelId}`,
);
assert.ok(parsed.feeds.length > 0, "payload contains zero feeds");
assert.ok(
  parsed.feeds.some((feed) => feed.price !== undefined && feed.price !== 0n),
  "no feed reported a non-zero price",
);
assert.ok(
  parsed.feeds.some((feed) => feed.id === BTC_USD_FEED_ID),
  `BTC/USD feed (id ${BTC_USD_FEED_ID}) not present`,
);

// --- Done ---
console.log("\n=========================================");
console.log("=== END-TO-END TEST PASSED ===");
console.log("=========================================");
console.log(`\nLazer contract: ${contractId}`);
console.log(
  `\nThe real Pyth Lazer price update was successfully verified on Stellar ${networkName}!`,
);
