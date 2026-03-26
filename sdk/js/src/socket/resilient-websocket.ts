import type { ClientRequestArgs } from "node:http";

import type { ClientOptions, ErrorEvent } from "isomorphic-ws";
import WebSocket from "isomorphic-ws";
import type { Logger } from "ts-log";
import { dummyLogger } from "ts-log";

import { CustomSocketClosureCodes } from "../protocol.js";
import { envIsBrowserOrWorker } from "../util/env-util.js";

const DEFAULT_HEARTBEAT_TIMEOUT_DURATION_MS = 5000; // 5 seconds
const DEFAULT_MAX_RETRY_DELAY_MS = 1000; // 1 second'
const DEFAULT_LOG_AFTER_RETRY_COUNT = 10;

export type ResilientWebSocketConfig = {
  endpoint: string;
  wsOptions?: ClientOptions | ClientRequestArgs | undefined;
  logger?: Logger;
  heartbeatTimeoutDurationMs?: number;
  maxRetryDelayMs?: number;
  logAfterRetryCount?: number;
};

/**
 * the isomorphic-ws package ships with some slightly-erroneous typings.
 * namely, it returns a WebSocket with typings that indicate the "terminate()" function
 * is available on all platforms.
 * Given that, under the hood, it is using the globalThis.WebSocket class, if it's available,
 * and falling back to using the https://www.npmjs.com/package/ws package, this
 * means there are API differences between the native WebSocket (the one in a web browser)
 * and the server-side version from the "ws" package.
 *
 * This type creates a WebSocket type reference we use to indicate the unknown
 * nature of the env in which is code is run.
 */
type UnsafeWebSocket = Omit<WebSocket, "terminate"> &
  Partial<Pick<WebSocket, "terminate">>;

export class ResilientWebSocket {
  private endpoint: string;
  private wsOptions?: ClientOptions | ClientRequestArgs | undefined;
  private logger: Logger;
  private heartbeatTimeoutDurationMs: number;
  private maxRetryDelayMs: number;
  private logAfterRetryCount: number;

  wsClient: UnsafeWebSocket | undefined;
  wsUserClosed = false;
  private wsFailedAttempts: number;
  private heartbeatTimeout?: NodeJS.Timeout | undefined;
  private retryTimeout?: NodeJS.Timeout | undefined;
  private _isReconnecting = false;

  isReconnecting(): boolean {
    return this._isReconnecting;
  }

  isConnected(): this is this & { wsClient: WebSocket } {
    return this.wsClient?.readyState === WebSocket.OPEN;
  }

  private shouldLogRetry() {
    return this.wsFailedAttempts % this.logAfterRetryCount === 0;
  }

  onError: (error: ErrorEvent) => void;
  onMessage: (data: WebSocket.Data) => void;
  onReconnect: () => void;
  onTimeout: () => void;

  constructor(config: ResilientWebSocketConfig) {
    this.endpoint = config.endpoint;
    this.wsOptions = config.wsOptions;
    this.logger = config.logger ?? dummyLogger;
    this.heartbeatTimeoutDurationMs =
      config.heartbeatTimeoutDurationMs ??
      DEFAULT_HEARTBEAT_TIMEOUT_DURATION_MS;
    this.maxRetryDelayMs = config.maxRetryDelayMs ?? DEFAULT_MAX_RETRY_DELAY_MS;
    this.logAfterRetryCount =
      config.logAfterRetryCount ?? DEFAULT_LOG_AFTER_RETRY_COUNT;

    this.wsFailedAttempts = 0;
    this.onError = (error: ErrorEvent) => {
      void error;
    };
    this.onMessage = (data: WebSocket.Data): void => {
      void data;
    };
    this.onReconnect = (): void => {
      // Empty function, can be set by the user.
    };
    this.onTimeout = (): void => {
      // Empty function, can be set by the user.
    };
  }

  send(data: string | Buffer) {
    this.logger.debug(`Sending message`);

    if (this.isConnected()) {
      this.wsClient.send(data);
    } else {
      this.logger.warn(
        `WebSocket to ${this.endpoint} is not connected. Cannot send message.`,
      );
    }
  }

  startWebSocket() {
    if (this.wsUserClosed) {
      this.logger.error(
        "Connection was explicitly closed by user. Will not reconnect.",
      );
      return;
    }

    if (this.wsClient !== undefined) {
      this.logger.info("WebSocket client already started.");
      return;
    }

    if (this.wsFailedAttempts === 0) {
      this.logger.info(`Creating Web Socket client`);
    }

    if (this.retryTimeout !== undefined) {
      clearTimeout(this.retryTimeout);
      this.retryTimeout = undefined;
    }

    // we wrap the new WebSocket() construction in a try / catch because,
    // if one of our instances legitimately goes down or there's some network
    // error that impacts connectivity, the WebSocket constructor will throw
    // an uncaught DOMException in the browser (or equivalent in Node, Bun or Deno)
    // and potentially blow up the process (if running in Node, Bun or Deno)
    try {
      // browser constructor supports a different 2nd argument for the constructor,
      // so we need to ensure it's not included if we're running in that environment:
      // https://developer.mozilla.org/en-US/docs/Web/API/WebSocket/WebSocket#protocols
      this.wsClient = new WebSocket(
        this.endpoint,
        envIsBrowserOrWorker() ? undefined : this.wsOptions,
      );

      this.wsClient.addEventListener("open", () => {
        this.logger.info("WebSocket connection established");
        this.wsFailedAttempts = 0;
        this._isReconnecting = false;
        this.resetHeartbeat();
        try {
          this.onReconnect();
        } catch (error) {
          this.logger.error("Error in onReconnect callback:", error);
        }
      });

      this.wsClient.addEventListener("close", (e) => {
        if (this.wsUserClosed) {
          this.logger.info(
            `WebSocket connection to ${this.endpoint} closed by user`,
          );
        } else {
          if (this.shouldLogRetry()) {
            this.logger.warn(
              `WebSocket connection to ${this.endpoint} closed unexpectedly: Code: ${e.code.toString()}`,
            );
          }
          this.handleReconnect();
        }
      });

      this.wsClient.addEventListener("error", (event) => {
        try {
          this.onError(event);
        } catch (error) {
          this.logger.error("Error in onError callback:", error);
        }
      });

      this.wsClient.addEventListener("message", (event) => {
        this.resetHeartbeat();
        try {
          this.onMessage(event.data);
        } catch (error) {
          this.logger.error("Error in onMessage callback:", error);
        }
      });

      if (typeof this.wsClient?.on === "function") {
        this.wsClient.on("ping", () => {
          this.logger.debug("Ping received");
          this.resetHeartbeat();
        });
      }
    } catch (error) {
      this.logger.error("Failed to create WebSocket client:", error);
      this.handleReconnect();
    }
  }

  private resetHeartbeat(): void {
    if (this.heartbeatTimeout !== undefined) {
      clearTimeout(this.heartbeatTimeout);
    }

    this.heartbeatTimeout = setTimeout(() => {
      const warnMsg = "Connection timed out. Reconnecting...";
      this.logger.warn(warnMsg);
      try {
        this.onTimeout();
      } catch (error) {
        this.logger.error("Error in onTimeout callback:", error);
      }
      if (this.wsClient) {
        if (typeof this.wsClient.terminate === "function") {
          this.wsClient.terminate();
        } else {
          // terminate is an implementation detail of the node-friendly
          // https://www.npmjs.com/package/ws package, but is not a native WebSocket API,
          // so we have to use the close method
          this.wsClient.close(
            CustomSocketClosureCodes.CLIENT_TIMEOUT_BUT_RECONNECTING,
            warnMsg,
          );
        }
      }
      // No direct call to handleReconnect() here
      // terminate/close will trigger the 'close' event, which will call it.
    }, this.heartbeatTimeoutDurationMs);
  }

  private handleReconnect() {
    if (this.wsUserClosed) {
      this.logger.info(
        "WebSocket connection closed by user, not reconnecting.",
      );
      return;
    }

    if (this.heartbeatTimeout !== undefined) {
      clearTimeout(this.heartbeatTimeout);
    }

    if (this.retryTimeout !== undefined) {
      clearTimeout(this.retryTimeout);
    }

    this.wsFailedAttempts += 1;
    this.wsClient = undefined;

    this._isReconnecting = true;

    if (this.shouldLogRetry()) {
      this.logger.error(
        "Connection closed unexpectedly or because of timeout. Reconnecting after " +
          String(this.retryDelayMs()) +
          "ms.",
      );
    }

    this.retryTimeout = setTimeout(() => {
      this.startWebSocket();
    }, this.retryDelayMs());
  }

  closeWebSocket(): void {
    // immediately block duplicate calls to this function
    this.wsUserClosed = true;
    if (typeof this.wsClient?.close === "function") {
      this.wsClient.close();
      this.wsClient = undefined;
    }
  }

  /**
   * Calculates the delay in milliseconds for exponential backoff based on the number of failed attempts.
   *
   * The delay increases exponentially with each attempt, starting at 20ms for the first attempt,
   * and is capped at maxRetryDelayMs for attempts greater than or equal to 10.
   *
   * @returns The calculated delay in milliseconds before the next retry.
   */
  private retryDelayMs(): number {
    if (this.wsFailedAttempts >= 10) {
      return this.maxRetryDelayMs;
    }
    return Math.min(2 ** this.wsFailedAttempts * 10, this.maxRetryDelayMs);
  }
}
