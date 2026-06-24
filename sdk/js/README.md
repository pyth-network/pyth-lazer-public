# pyth-lazer-sdk - Readme

## Contributing & Development

See [contributing.md](docs/contributing/contributing.md) for information on how to develop or contribute to this project!

## How to use

```javascript
import { PythLazerClient } from "@pythnetwork/pyth-lazer-sdk";

const c = await PythLazerClient.create({
  token: "YOUR-AUTH-TOKEN-HERE",
  logger: console, // Optionally log operations (to the console in this case.)
  webSocketPoolConfig: {
    numConnections: 4, // Optionally specify number of parallel redundant connections to reduce the chance of dropped messages. The connections will round-robin across the provided URLs. Default is 4.
    onError: (error) => {
      console.error("⛔️ WebSocket error:", error.message);
    },
    // Optional configuration for resilient WebSocket connections
    rwsConfig: {
      heartbeatTimeoutDurationMs: 5000, // Optional heartbeat timeout duration in milliseconds
      maxRetryDelayMs: 1000, // Optional maximum retry delay in milliseconds
      logAfterRetryCount: 10, // Optional log after how many retries
    },
  },
});

c.addMessageListener((message) => {
  console.info("received the following from the Lazer stream:", message);
});

// Monitor for all connections in the pool being down simultaneously (e.g. if the internet goes down)
// The connections may still try to reconnect in the background. To shut down the client completely, call shutdown().
c.addAllConnectionsDownListener(() => {
  console.error("All connections are down!");
});

// Create and remove one or more subscriptions on the fly
c.subscribe({
  type: "subscribe",
  subscriptionId: 1,
  priceFeedIds: [1, 2],
  properties: ["price"],
  formats: ["solana"],
  deliveryFormat: "binary",
  channel: "fixed_rate@200ms",
  parsed: false,
  jsonBinaryEncoding: "base64",
});
c.subscribe({
  type: "subscribe",
  subscriptionId: 2,
  priceFeedIds: [1, 2, 3, 4, 5],
  properties: ["price", "exponent", "publisherCount", "confidence"],
  formats: ["evm"],
  deliveryFormat: "json",
  channel: "fixed_rate@200ms",
  parsed: true,
  jsonBinaryEncoding: "hex",
});
```

## Error callbacks and outage detection

The SDK exposes two error callbacks and an "all connections down" listener that
serve distinct purposes. Choose what to handle based on whether you want
visibility into per-socket churn or a signal that the price stream has actually
stopped.

### `onWebSocketError` (per-socket errors)

Passed via `webSocketPoolConfig.onWebSocketError`. Fires for individual
socket-level errors on a single underlying WebSocket — for example, a transient
`502` from one endpoint during a server upgrade or chaos test:

```json
{
  "at": "pyth#connect",
  "level": "ERROR",
  "error": "Unexpected server response: 502"
}
```

**In most cases you never need to handle `onWebSocketError` — it is purely
informational.** The pool manages each socket's lifecycle (reconnect, retry,
failover across the configured URLs), so these errors are expected churn when
one endpoint hiccups. When the SDK is configured with redundant connections
(`numConnections > 1`, multiple `urls`), a single socket failing does **not**
mean the stream is dark — the other connections continue delivering messages.
Treat it as diagnostic only; don't page or alert on it.

### `onWebSocketPoolError` (pool-level errors)

Passed via `webSocketPoolConfig.onWebSocketPoolError`. Fires for errors that
originate from the pool itself rather than a single socket — server-sent
`subscriptionError`/`error` messages, exceptions thrown by your message
listeners, send failures, and other internal errors that would otherwise
surface as unhandled rejections. Applications should listen to this and surface
it (log, alert, etc.).

```javascript
webSocketPoolConfig: {
  onWebSocketError: (error) => {
    // Individual socket error — usually safe to log at info/debug.
    console.warn("websocket error (single connection):", error.message);
  },
  onWebSocketPoolError: (error) => {
    // Pool-level error — surface this.
    console.error("websocket pool error:", error);
  },
},
```

### `addAllConnectionsDownListener` (stream is dark)

Registered on the client. Fires when **every** redundant connection is down or
reconnecting at the same time — i.e., the price stream really has stopped.
This is the right signal for outage detection and paging:

```javascript
c.addAllConnectionsDownListener(() => {
  console.error("All Lazer connections are down — stream is dark.");
});
```

A matching `addConnectionRestoredListener` fires when at least one connection
comes back after an all-down period.

### Recommendation

For detecting real outages, handle `onWebSocketPoolError` and
`addAllConnectionsDownListener`. Do not page or panic solely on
`onWebSocketError` when the SDK is configured with redundant endpoints and
prices are still flowing — individual sockets are expected to flap during
server upgrades and chaos tests.
