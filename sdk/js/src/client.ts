import type WebSocket from "isomorphic-ws";
import type { Logger } from "ts-log";
import { dummyLogger } from "ts-log";

import {
  DEFAULT_METADATA_SERVICE_URL,
  DEFAULT_PRICE_SERVICE_URL,
} from "./constants.js";
import type {
  JsonUpdate,
  LatestPriceRequest,
  ParsedPayload,
  PriceRequest,
  Request,
  Response,
  SymbolResponse,
  SymbolsQueryParams,
} from "./protocol.js";
import { BINARY_UPDATE_FORMAT_MAGIC_LE, FORMAT_MAGICS_LE } from "./protocol.js";
import type { WebSocketPoolConfig } from "./socket/websocket-pool.js";
import { WebSocketPool } from "./socket/websocket-pool.js";
import { bufferFromWebsocketData } from "./util/buffer-util.js";

export type BinaryResponse = {
  subscriptionId: number;
  evm?: Buffer | undefined;
  solana?: Buffer | undefined;
  parsed?: ParsedPayload | undefined;
  leEcdsa?: Buffer | undefined;
  leUnsigned?: Buffer | undefined;
};
export type JsonOrBinaryResponse =
  | {
      type: "json";
      value: Response;
    }
  | { type: "binary"; value: BinaryResponse };

const UINT16_NUM_BYTES = 2;
const UINT32_NUM_BYTES = 4;
const UINT64_NUM_BYTES = 8;

export type LazerClientConfig = {
  /**
   * if provided and the signal is detected as canceled,
   * all active listeners and connections will be unbound and killed.
   */
  abortSignal?: AbortSignal;
  token: string;
  metadataServiceUrl?: string;
  priceServiceUrl?: string;
  logger?: Logger;
  webSocketPoolConfig: WebSocketPoolConfig;
};

type PythLazerClientConstructorOpts = {
  abortSignal: AbortSignal | null | undefined;
  logger: Logger;
  metadataServiceUrl: string;
  priceServiceUrl: string;
  token: string;
  wsp: WebSocketPool;
};

export class PythLazerClient {
  private logger: PythLazerClientConstructorOpts["logger"];
  private metadataServiceUrl: PythLazerClientConstructorOpts["metadataServiceUrl"];
  private priceServiceUrl: PythLazerClientConstructorOpts["priceServiceUrl"];
  private token: PythLazerClientConstructorOpts["token"];
  private wsp: PythLazerClientConstructorOpts["wsp"];
  private abortSignal: AbortSignal | undefined | null;

  private constructor({
    abortSignal,
    logger,
    metadataServiceUrl,
    priceServiceUrl,
    token,
    wsp,
  }: PythLazerClientConstructorOpts) {
    this.abortSignal = abortSignal;
    this.logger = logger;
    this.metadataServiceUrl = metadataServiceUrl;
    this.priceServiceUrl = priceServiceUrl;
    this.token = token;
    this.wsp = wsp;

    this.bindHandlers();
  }

  /**
   * Creates a new PythLazerClient instance.
   * @param config - Configuration including token, metadata service URL, and price service URL, and WebSocket pool configuration
   */
  static async create(config: LazerClientConfig): Promise<PythLazerClient> {
    const token = config.token;

    // Collect and remove trailing slash from URLs
    const metadataServiceUrl = (
      config.metadataServiceUrl ?? DEFAULT_METADATA_SERVICE_URL
    ).replace(/\/+$/, "");
    const priceServiceUrl = (
      config.priceServiceUrl ?? DEFAULT_PRICE_SERVICE_URL
    ).replace(/\/+$/, "");
    const logger = config.logger ?? dummyLogger;

    // the prior API was mismatched, in that it marked a websocket pool as optional,
    // yet all internal code on the Pyth Pro client used it and threw if it didn't exist.
    // now, the typings indicate it's no longer optional and we don't sanity check
    // if it's set
    const wsp = await WebSocketPool.create(
      config.webSocketPoolConfig,
      token,
      config.abortSignal,
      logger,
    );

    const client = new PythLazerClient({
      abortSignal: config.abortSignal,
      logger,
      metadataServiceUrl,
      priceServiceUrl,
      token,
      wsp,
    });

    return client;
  }

  /**
   * Adds a message listener that receives either JSON or binary responses from the WebSocket connections.
   * The listener will be called for each message received, with deduplication across redundant connections.
   * @param handler - Callback function that receives the parsed message. The message can be either a JSON response
   * or a binary response containing EVM, Solana, or parsed payload data.
   */
  addMessageListener(handler: (event: JsonOrBinaryResponse) => void) {
    this.wsp.addMessageListener(async (data: WebSocket.Data) => {
      if (typeof data === "string") {
        handler({
          type: "json",
          value: JSON.parse(data) as Response,
        });
        return;
      }
      const buffData = await bufferFromWebsocketData(data);
      let pos = 0;
      const magic = buffData
        .subarray(pos, pos + UINT32_NUM_BYTES)
        .readUint32LE();
      pos += UINT32_NUM_BYTES;
      if (magic !== BINARY_UPDATE_FORMAT_MAGIC_LE) {
        throw new Error("binary update format magic mismatch");
      }
      // TODO: some uint64 values may not be representable as Number.
      const subscriptionId = Number(
        buffData.subarray(pos, pos + UINT64_NUM_BYTES).readBigInt64BE(),
      );
      pos += UINT64_NUM_BYTES;

      const value: BinaryResponse = { subscriptionId };
      while (pos < buffData.length) {
        const len = buffData
          .subarray(pos, pos + UINT16_NUM_BYTES)
          .readUint16BE();
        pos += UINT16_NUM_BYTES;
        const magic = buffData
          .subarray(pos, pos + UINT32_NUM_BYTES)
          .readUint32LE();
        if (magic === FORMAT_MAGICS_LE.EVM) {
          value.evm = buffData.subarray(pos, pos + len);
        } else if (magic === FORMAT_MAGICS_LE.SOLANA) {
          value.solana = buffData.subarray(pos, pos + len);
        } else if (magic === FORMAT_MAGICS_LE.LE_ECDSA) {
          value.leEcdsa = buffData.subarray(pos, pos + len);
        } else if (magic === FORMAT_MAGICS_LE.LE_UNSIGNED) {
          value.leUnsigned = buffData.subarray(pos, pos + len);
        } else if (magic === FORMAT_MAGICS_LE.JSON) {
          value.parsed = JSON.parse(
            buffData.subarray(pos + UINT32_NUM_BYTES, pos + len).toString(),
          ) as ParsedPayload;
        } else {
          throw new Error(`unknown magic:  ${magic.toString()}`);
        }
        pos += len;
      }
      handler({ type: "binary", value });
    });
  }

  /**
   * binds any internal event handlers
   */
  bindHandlers() {
    this.abortSignal?.addEventListener("abort", this.abortHandler);
  }

  subscribe(request: Request) {
    if (request.type !== "subscribe") {
      throw new Error("Request must be a subscribe request");
    }
    this.wsp.addSubscription(request);
  }

  unsubscribe(subscriptionId: number) {
    this.wsp.removeSubscription(subscriptionId);
  }

  send(request: Request) {
    this.wsp.sendRequest(request);
  }

  /**
   * Registers a handler function that will be called whenever all WebSocket connections are down or attempting to reconnect.
   * The connections may still try to reconnect in the background. To shut down the pool, call `shutdown()`.
   * @param handler - Function to be called when all connections are down
   */
  addAllConnectionsDownListener(handler: () => void): void {
    this.wsp.addAllConnectionsDownListener(handler);
  }

  /**
   * Registers a handler function that will be called when at least one connection is restored after all were down.
   * @param handler - Function to be called when connection is restored
   */
  addConnectionRestoredListener(handler: () => void): void {
    this.wsp.addConnectionRestoredListener(handler);
  }

  /**
   * Registers a handler function that will be called when an individual connection times out (heartbeat timeout).
   * @param handler - Function to be called with connection index and endpoint URL
   */
  addConnectionTimeoutListener(
    handler: (connectionIndex: number, endpoint: string) => void,
  ): void {
    this.wsp.addConnectionTimeoutListener(handler);
  }

  /**
   * Registers a handler function that will be called when an individual connection reconnects.
   * @param handler - Function to be called with connection index and endpoint URL
   */
  addConnectionReconnectListener(
    handler: (connectionIndex: number, endpoint: string) => void,
  ): void {
    this.wsp.addConnectionReconnectListener(handler);
  }

  /**
   * called if and only if a user provided an abort signal and it was aborted
   */
  protected abortHandler = (): void => {
    this.shutdown();
  };

  shutdown(): void {
    // Clean up abort signal listener to prevent memory leak
    this.abortSignal?.removeEventListener("abort", this.abortHandler);
    this.wsp.shutdown();
  }

  /**
   * Private helper method to make authenticated HTTP requests with Bearer token
   * @param url - The URL to fetch
   * @param options - Additional fetch options
   * @returns Promise resolving to the fetch Response
   */
  private authenticatedFetch(
    url: string,
    options: RequestInit = {},
  ): Promise<globalThis.Response> {
    // Handle all possible types of headers (Headers object, array, or plain object)
    const headers = new Headers(options.headers);
    headers.set(
      "Authorization",
      headers.get("authorization") ?? `Bearer ${this.token}`,
    );

    return fetch(url, {
      ...options,
      headers,
    });
  }

  /**
   * Queries the symbols endpoint to get available price feed symbols.
   * @param params - Optional query parameters to filter symbols
   * @returns Promise resolving to array of symbol information
   */
  async getSymbols(params?: SymbolsQueryParams): Promise<SymbolResponse[]> {
    const url = new URL(`${this.metadataServiceUrl}/v1/symbols`);

    if (params?.query) {
      url.searchParams.set("query", params.query);
    }
    if (params?.asset_type) {
      url.searchParams.set("asset_type", params.asset_type);
    }

    try {
      const response = await this.authenticatedFetch(url.toString());
      if (!response.ok) {
        throw new Error(
          `HTTP error! status: ${String(response.status)} - ${await response.text()}`,
        );
      }
      return (await response.json()) as SymbolResponse[];
    } catch (error) {
      throw new Error(
        `Failed to fetch symbols: ${error instanceof Error ? error.message : String(error)}`,
      );
    }
  }

  /**
   * Queries the latest price endpoint to get current price data.
   * @param params - Parameters for the latest price request
   * @returns Promise resolving to JsonUpdate with current price data
   */
  async getLatestPrice(params: LatestPriceRequest): Promise<JsonUpdate> {
    const url = `${this.priceServiceUrl}/v1/latest_price`;

    try {
      const body = JSON.stringify(params);
      this.logger.debug("getLatestPrice", { body, url });
      const response = await this.authenticatedFetch(url, {
        body: body,
        headers: {
          "Content-Type": "application/json",
        },
        method: "POST",
      });
      if (!response.ok) {
        throw new Error(
          `HTTP error! status: ${String(response.status)} - ${await response.text()}`,
        );
      }
      return (await response.json()) as JsonUpdate;
    } catch (error) {
      throw new Error(
        `Failed to fetch latest price: ${error instanceof Error ? error.message : String(error)}`,
      );
    }
  }

  /**
   * Queries the price endpoint to get historical price data at a specific timestamp.
   * @param params - Parameters for the price request including timestamp
   * @returns Promise resolving to JsonUpdate with price data at the specified time
   */
  async getPrice(params: PriceRequest): Promise<JsonUpdate> {
    const url = `${this.priceServiceUrl}/v1/price`;

    try {
      const body = JSON.stringify(params);
      this.logger.debug("getPrice", { body, url });
      const response = await this.authenticatedFetch(url, {
        body: body,
        headers: {
          "Content-Type": "application/json",
        },
        method: "POST",
      });
      if (!response.ok) {
        throw new Error(
          `HTTP error! status: ${String(response.status)} - ${await response.text()}`,
        );
      }
      return (await response.json()) as JsonUpdate;
    } catch (error) {
      throw new Error(
        `Failed to fetch price: ${error instanceof Error ? error.message : String(error)}`,
      );
    }
  }
}
