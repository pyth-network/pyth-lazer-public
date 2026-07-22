export const SOLANA_LAZER_PROGRAM_ID =
  "pytd2yyk641x7ak7mkaasSJVXh6YYZnC7wTmtgAyxPt";
export const SOLANA_LAZER_STORAGE_ID =
  "3rdJbqfnagQ4yx9HXJViD4zc4xpiSqmFsKpPuSCQVyQL";
export const DEFAULT_METADATA_SERVICE_URL = "https://pyth.dourolabs.app";
export const DEFAULT_PRICE_SERVICE_URL = "https://pyth-lazer-0.dourolabs.app";
/**
 * Default stream endpoints, round-robined across the pool's connections when
 * the caller does not supply their own `urls`. These are redundant instances
 * of the Lazer api-service stream.
 */
export const DEFAULT_STREAM_SERVICE_URLS = [
  "wss://pyth-0.dourolabs.app/v1/prices/stream",
  "wss://pyth-1.dourolabs.app/v1/prices/stream",
  "wss://pyth-2.dourolabs.app/v1/prices/stream",
];

/**
 * WebSocket subprotocol marker that carries the auth token during the handshake
 * (used in every environment): clients send
 * `Sec-WebSocket-Protocol: pyth-lazer-auth, <token>`.
 */
export const AUTH_SUBPROTOCOL = "pyth-lazer-auth";
