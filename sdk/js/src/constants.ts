export const SOLANA_LAZER_PROGRAM_ID =
  "pytd2yyk641x7ak7mkaasSJVXh6YYZnC7wTmtgAyxPt";
export const SOLANA_LAZER_STORAGE_ID =
  "3rdJbqfnagQ4yx9HXJViD4zc4xpiSqmFsKpPuSCQVyQL";
export const DEFAULT_METADATA_SERVICE_URL = "https://pyth.dourolabs.app";
export const DEFAULT_PRICE_SERVICE_URL = "https://pyth-lazer-0.dourolabs.app";
/**
 * Default stream endpoints, round-robined across the pool's connections when
 * the caller does not supply their own `urls`. These are redundant router
 * instances.
 */
export const DEFAULT_STREAM_SERVICE_URLS = [
  "wss://pyth-lazer-0.dourolabs.app/v1/stream",
  "wss://pyth-lazer-1.dourolabs.app/v1/stream",
  "wss://pyth-lazer-2.dourolabs.app/v1/stream",
];

/**
 * WebSocket subprotocol marker that carries the auth token during the handshake
 * (used in every environment): clients send
 * `Sec-WebSocket-Protocol: pyth-lazer-auth, <token>`.
 */
export const AUTH_SUBPROTOCOL = "pyth-lazer-auth";
