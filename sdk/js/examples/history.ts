/* eslint-disable no-console */

import { PythLazerClient } from "../src/index.js";
import { displayParsedPrices } from "./util.js";

const client = await PythLazerClient.create({
  logger: console,
  token: "your-token-here",
});

// Example 1: Get latest price for BTC using feed IDs
console.log("\n=== Example 1: Latest BTC price (requested with feed ID) ===");
const response1 = await client.getLatestPrice({
  channel: "fixed_rate@200ms",
  formats: [],
  jsonBinaryEncoding: "hex",
  parsed: true,
  priceFeedIds: [1],
  properties: ["price", "confidence", "exponent"],
});
displayParsedPrices(response1);

// Example 2: Get latest price using symbols
console.log("\n=== Example 2: Latest ETH price (requested with symbols) ===");
const response2 = await client.getLatestPrice({
  channel: "real_time",
  formats: [],
  parsed: true,
  priceFeedIds: [2],
  properties: ["price", "confidence", "exponent"],
});
displayParsedPrices(response2);

// Example 3: Get historical price at specific timestamp
console.log("\n=== Example 3: Historical BTC price at timestamp ===");
const timestamp = 1_754_348_458_565_000;
console.log(
  `Requesting price from timestamp: ${timestamp.toString()} (${new Date(timestamp / 1000).toISOString()})`,
);
const response3 = await client.getPrice({
  channel: "real_time",
  formats: [],
  parsed: true,
  priceFeedIds: [1],
  properties: ["price", "confidence", "exponent"],
  timestamp: timestamp,
});
displayParsedPrices(response3);
