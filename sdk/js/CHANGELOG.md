# pyth-lazer-sdk - Changelog

## 7.0.0

Breaking changes to WebSocket streaming, aligning the SDK with the Lazer **api service** (the public read API that supersedes the router's public stream):

- **WebSocket auth moves to `Sec-WebSocket-Protocol`.** In every environment (browser and Node) the token is sent as the `pyth-lazer-auth` subprotocol (`Sec-WebSocket-Protocol: pyth-lazer-auth, <token>`) instead of the `?ACCESS_TOKEN=` query parameter, keeping it out of URLs and access logs.
- **Default stream URLs** move from the router (`wss://pyth-lazer-{0,1}.dourolabs.app/v1/stream`) to the redundant api service instances (`wss://pyth-{0,1,2}.dourolabs.app/v1/prices/stream`), round-robined across the pool's connections; `DEFAULT_STREAM_SERVICE_0_URL`/`DEFAULT_STREAM_SERVICE_1_URL` are replaced by a single `DEFAULT_STREAM_SERVICE_URLS` array.
- **Removed** the `?ACCESS_TOKEN=` helper and its `./util/url-util` export (`addAuthTokenToWebSocketUrl`).
- **Removed `webSocketPoolConfig.rwsConfig.wsOptions`.** Raw `ws` options (which previously carried the `Authorization` header) are no longer accepted, since the token now travels via the subprotocol. The `rwsConfig` tuning knobs (`heartbeatTimeoutDurationMs`, `maxRetryDelayMs`, `logAfterRetryCount`) are unchanged and still supported.

The `subscribe`/message wire protocol is unchanged.
