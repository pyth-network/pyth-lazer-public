# pyth-lazer-sdk - Readme

## Contributing & Development

See [contributing.md](docs/contributing/contributing.md) for information on how to develop or contribute to this project!

## How to use

```javascript
import { PythLazerClient } from "@pythnetwork/pyth-lazer-sdk";

const client = await PythLazerClient.create({
  // Your Lazer access token.
  // The SDK sends it during the WebSocket handshake via
  // `Sec-WebSocket-Protocol`.
  // refer to the "Authentication" section, below.
  token: "YOUR-AUTH-TOKEN-HERE",
  logger: console, // Optional. Customize where connection logs are routed
  webSocketPoolConfig: {
    // Optional.
    // Parallel redundant connections, round-robined across `urls`, to
    // reduce dropped messages.
    // Default is 4.
    numConnections: 4,

    // Optional.
    // Override the stream endpoint(s). Round-robined across `numConnections`.
    // Defaults to the api service instances (pyth-0/1/2.dourolabs.app).
    // urls: ["wss://pyth-0.dourolabs.app/v1/prices/stream"],
    onWebSocketError: (error) => {
      console.error("⛔️ WebSocket error:", error.message);
    },

    // Optional.
    // Tune the underlying resilient connections. Auth, endpoint, and logger
    // are managed by the SDK and cannot be set here.
    rwsConfig: {
      heartbeatTimeoutDurationMs: 5000, // Reconnect if no message/ping within this window.
      maxRetryDelayMs: 1000, // Cap on exponential reconnect backoff.
      logAfterRetryCount: 10, // Log every Nth failed reconnect attempt.
    },
  },
});

client.addMessageListener((message) => {
  console.info("received the following from the Lazer stream:", message);
});

// Fires when every connection is down at once.
// Connections keep retrying in the background
client.addAllConnectionsDownListener(() => {
  console.error("All connections are down!");
});

// Subscribe on the fly.
client.subscribe({
  type: "subscribe",
  subscriptionId: 1,
  priceFeedIds: [1, 2, 3, 4, 5],
  properties: ["price", "exponent", "publisherCount", "confidence"],
  formats: [],
  deliveryFormat: "json",
  channel: "fixed_rate@200ms",
  parsed: true,
});
```

## Authentication

Pass your access token as `token` — that is the only auth step. The SDK sends it
to the server during the WebSocket handshake via the **`Sec-WebSocket-Protocol`**
header (`Sec-WebSocket-Protocol: pyth-lazer-auth, <token>`), the same way in the
browser and in Node. It is never placed in the URL as a query parameter, so the
token stays out of URLs, browser history, and proxy/server access logs — worth
knowing if you care about credential hygiene.

Connections default to the redundant Lazer api service stream instances
(`wss://pyth-{0,1,2}.dourolabs.app/v1/prices/stream`), round-robined across
`numConnections` (override via `webSocketPoolConfig.urls`). They serve parsed /
unsigned JSON updates; signed `evm` / `solana` update payloads are not served on
this stream.

> **v7 breaking change:** the token moved from `?ACCESS_TOKEN=` to
> `Sec-WebSocket-Protocol`, and the default stream moved from the router to the
> api service. See [CHANGELOG.md](CHANGELOG.md).

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
