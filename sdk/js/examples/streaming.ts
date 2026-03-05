/* eslint-disable no-console */
/* eslint-disable @typescript-eslint/no-empty-function */
/* eslint-disable unicorn/prefer-top-level-await */

import type { JsonUpdate } from "../src/index.js";
import { PythLazerClient } from "../src/index.js";
import { refreshFeedDisplay, renderFeeds } from "./util.js";

// Ignore debug messages
console.debug = () => {};

// Store feed data for in-place updates
const feedData = new Map<
  string,
  {
    priceFeedId: number;
    price: number;
    confidence: number | undefined;
    emaPrice: number;
    emaConfidence: number | undefined;
    exponent: number;
    lastUpdate: Date;
  }
>();
const symbolsMap = new Map<number, string>();

const client = await PythLazerClient.create({
  logger: console, // Optionally log operations (to the console in this case.)
  token: "your-token-here", // Replace with your actual access token
  webSocketPoolConfig: {
    numConnections: 4, // Optionally specify number of parallel redundant connections to reduce the chance of dropped messages. The connections will round-robin across the provided URLs. Default is 4.
    onWebSocketError: (errorEvent) => {
      console.error("WebSocket error event:", errorEvent);
    },
    onWebSocketPoolError: (error) => {
      console.error("An error occurred within the WebSocketPool:", error);
    },
    // Optional configuration for resilient WebSocket connections
    rwsConfig: {
      heartbeatTimeoutDurationMs: 5000, // Optional heartbeat timeout duration in milliseconds
      logAfterRetryCount: 10, // Optional log after how many retries
      maxRetryDelayMs: 1000, // Optional maximum retry delay in milliseconds
    },
  },
});

// Fetch current map of price feeds
void client.getSymbols().then((symbols) => {
  for (const symbol of symbols) {
    symbolsMap.set(symbol.pyth_lazer_id, symbol.symbol);
  }
});

// Add a listener to read and display messages from the Lazer stream
client.addMessageListener((message) => {
  switch (message.type) {
    case "json": {
      if (message.value.type == "streamUpdated") {
        refreshFeedDisplay(message.value as JsonUpdate, feedData, symbolsMap);
      }
      break;
    }
    case "binary": {
      // Print out the binary hex messages if you want:
      // if ("solana" in message.value) {
      //   console.info("solana message:", message.value.solana?.toString("hex"));
      // }
      // if ("evm" in message.value) {
      //   console.info("evm message:", message.value.evm?.toString("hex"));
      // }
      break;
    }
  }
});

// Monitor for all connections in the pool being down simultaneously (e.g. if the internet goes down)
// The connections may still try to reconnect in the background. To shut down the client completely, call shutdown().
client.addAllConnectionsDownListener(() => {
  console.error("All connections are down!");
});

// Monitor for when connectivity is restored after all connections were down
client.addConnectionRestoredListener(() => {
  console.log("Connection restored after all connections were down.");
});

// Monitor individual connection timeouts (heartbeat timeout)
client.addConnectionTimeoutListener((connectionIndex, endpoint) => {
  console.warn(
    `Connection ${connectionIndex.toString()} to ${endpoint} timed out.`,
  );
});

// Monitor individual connection reconnections
client.addConnectionReconnectListener((connectionIndex, endpoint) => {
  console.log(
    `Connection ${connectionIndex.toString()} to ${endpoint} reconnected.`,
  );
});

renderFeeds(feedData, symbolsMap);

// Create and remove one or more subscriptions on the fly
client.subscribe({
  channel: "fixed_rate@200ms",
  deliveryFormat: "binary",
  formats: ["solana"],
  jsonBinaryEncoding: "base64",
  parsed: false,
  priceFeedIds: [1, 2],
  properties: ["price"],
  subscriptionId: 1,
  type: "subscribe",
});
client.subscribe({
  channel: "fixed_rate@50ms",
  deliveryFormat: "json",
  formats: ["evm"],
  jsonBinaryEncoding: "hex",
  parsed: true,
  priceFeedIds: [1, 2, 3, 4, 5],
  properties: [
    "price",
    "exponent",
    "publisherCount",
    "confidence",
    "emaPrice",
    "emaConfidence",
  ],
  subscriptionId: 2,
  type: "subscribe",
});
client.subscribe({
  channel: "real_time",
  deliveryFormat: "json",
  formats: ["solana"],
  jsonBinaryEncoding: "hex",
  parsed: true,
  priceFeedIds: [1],
  properties: ["price", "confidence"],
  subscriptionId: 3,
  type: "subscribe",
});

await new Promise((resolve) => setTimeout(resolve, 30_000));

client.unsubscribe(1);
client.unsubscribe(2);
client.unsubscribe(3);

// Clear screen and move cursor to top
process.stdout.write("\u001B[2J\u001B[H");
console.log("🛑 Shutting down Pyth Lazer demo after 30 seconds...");
console.log("👋 Goodbye!");

client.shutdown();
