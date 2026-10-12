export type Format =
  | "evm"
  | "solana"
  | "leEcdsa"
  | "leUnsigned"
  | "packedEvmEcdsa";
export type DeliveryFormat = "json" | "binary";
export type JsonBinaryEncoding = "base64" | "hex";
export type PriceFeedProperty =
  | "price"
  | "bestBidPrice"
  | "bestAskPrice"
  | "exponent"
  | "publisherCount"
  | "confidence"
  | "fundingRate"
  | "fundingTimestamp"
  | "fundingRateInterval"
  | "marketSession"
  | "emaPrice"
  | "emaConfidence"
  | "feedUpdateTimestamp";
export type Channel =
  | "real_time"
  | "fixed_rate@50ms"
  | "fixed_rate@200ms"
  | "fixed_rate@1000ms";

export type Request =
  | {
      type: "subscribe";
      subscriptionId: number;
      priceFeedIds?: number[] | undefined;
      symbols?: string[] | undefined;
      /** Required unless `formats` is `["packedEvmEcdsa"]`, which fixes the properties itself and accepts none. */
      properties?: PriceFeedProperty[] | undefined;
      formats: Format[];
      deliveryFormat?: DeliveryFormat | undefined;
      jsonBinaryEncoding?: JsonBinaryEncoding | undefined;
      parsed?: boolean | undefined;
      ignoreInvalidFeedIds?: boolean | undefined;
      channel: Channel;
    }
  | {
      type: "unsubscribe";
      subscriptionId: number;
    };

export type ParsedFeedPayload = {
  priceFeedId: number;
  price?: string | undefined;
  bestBidPrice?: string | undefined;
  bestAskPrice?: string | undefined;
  publisherCount?: number | undefined;
  exponent?: number | undefined;
  confidence?: number | undefined;
  fundingRate?: number | undefined;
  fundingTimestamp?: number | undefined;
  fundingRateInterval?: number | undefined;
  marketSession?: string | undefined;
  emaPrice?: string | undefined;
  emaConfidence?: number | undefined;
  feedUpdateTimestamp?: number | undefined;
};

export type ParsedPayload = {
  timestampUs: string;
  priceFeeds: ParsedFeedPayload[];
};

export type JsonBinaryData = {
  encoding: JsonBinaryEncoding;
  data: string;
};

export type InvalidFeedSubscriptionDetails = {
  unknownIds: number[];
  unknownSymbols: string[];
  unsupportedChannels: number[];
  unstable: number[];
};

export type Response =
  | {
      type: "error";
      error: string;
    }
  | {
      type: "subscribed";
      subscriptionId: number;
    }
  | {
      type: "subscribedWithInvalidFeedIdsIgnored";
      subscriptionId: number;
      subscribedFeedIds: number[];
      ignoredInvalidFeedIds: InvalidFeedSubscriptionDetails;
    }
  | {
      type: "unsubscribed";
      subscriptionId: number;
    }
  | {
      type: "subscriptionError";
      subscriptionId: number;
      error: string;
    }
  | {
      type: "streamUpdated";
      subscriptionId: number;
      parsed?: ParsedPayload | undefined;
      evm?: JsonBinaryData | undefined;
      solana?: JsonBinaryData | undefined;
      leEcdsa?: JsonBinaryData | undefined;
      leUnsigned?: JsonBinaryData | undefined;
      packedEvmEcdsa?: JsonBinaryData | undefined;
    };

export const BINARY_UPDATE_FORMAT_MAGIC_LE = 461_928_307;

export const FORMAT_MAGICS_LE = {
  EVM: 2_593_727_018,
  JSON: 3_302_625_434,
  LE_ECDSA: 1_296_547_300,
  LE_UNSIGNED: 1_499_680_012,
  SOLANA: 2_182_742_457,
};

/** The magic at the start of a standard payload serialized as big-endian, read as a big-endian u32. */
export const PAYLOAD_FORMAT_MAGIC_BE = 2_479_346_549;

/** The magic at the start of a `packedEvmEcdsa` payload: `PEVM`, read as a big-endian u32. */
export const PACKED_EVM_PAYLOAD_FORMAT_MAGIC_BE = 1_346_721_357;

export type AssetType =
  | "crypto"
  | "fx"
  | "equity"
  | "metal"
  | "rates"
  | "nav"
  | "commodity"
  | "funding-rate"
  | "eco"
  | "kalshi";

export type SymbolsQueryParams = {
  query?: string;
  asset_type?: AssetType;
};

export type SymbolResponse = {
  pyth_lazer_id: number;
  name: string;
  symbol: string;
  description: string;
  asset_type: string;
  exponent: number;
  min_publishers: number;
  min_channel: string;
  state: string;
  schedule: string;
  cmc_id?: number | null;
  hermes_id?: string | null;
  interval?: string | null;
};

export type LatestPriceRequest = {
  priceFeedIds?: number[];
  symbols?: string[];
  /** Required unless `formats` is `["packedEvmEcdsa"]`, which fixes the properties itself and accepts none. */
  properties?: PriceFeedProperty[];
  formats: Format[];
  jsonBinaryEncoding?: JsonBinaryEncoding;
  parsed?: boolean;
  channel: Channel;
};

export type PriceRequest = {
  timestamp: number;
  priceFeedIds?: number[];
  symbols?: string[];
  /** Required unless `formats` is `["packedEvmEcdsa"]`, which fixes the properties itself and accepts none. */
  properties?: PriceFeedProperty[];
  formats: Format[];
  jsonBinaryEncoding?: JsonBinaryEncoding;
  parsed?: boolean;
  channel: Channel;
};

export type JsonUpdate = {
  parsed?: ParsedPayload;
  evm?: JsonBinaryData;
  solana?: JsonBinaryData;
  leEcdsa?: JsonBinaryData;
  leUnsigned?: JsonBinaryData;
  packedEvmEcdsa?: JsonBinaryData;
};

export enum CustomSocketClosureCodes {
  CLIENT_TIMEOUT_BUT_RECONNECTING = 4000,
}
