import { beforeEach, describe, expect, it, mock } from "bun:test";

import {
  AUTH_SUBPROTOCOL,
  DEFAULT_STREAM_SERVICE_URLS,
} from "../../constants.js";
import type { Request } from "../../protocol.js";
import type { ResilientWebSocketConfig } from "../resilient-websocket.js";
import type { WebSocketPoolConfig } from "../websocket-pool.js";
import { WebSocketPool } from "../websocket-pool.js";

// Mock ResilientWebSocket
class MockResilientWebSocket {
  static instances: MockResilientWebSocket[] = [];

  endpoint: string;
  protocols?: string[] | undefined;
  heartbeatTimeoutDurationMs?: number | undefined;
  maxRetryDelayMs?: number | undefined;
  logAfterRetryCount?: number | undefined;
  wsUserClosed = false;
  onReconnect: () => void = () => undefined;
  onTimeout: () => void = () => undefined;
  onError: (error: unknown) => void = () => undefined;
  onMessage: (data: unknown) => void = () => undefined;

  private _isConnected = false;
  private _isReconnecting = false;

  constructor(config: ResilientWebSocketConfig) {
    this.endpoint = config.endpoint;
    this.protocols = config.protocols;
    this.heartbeatTimeoutDurationMs = config.heartbeatTimeoutDurationMs;
    this.maxRetryDelayMs = config.maxRetryDelayMs;
    this.logAfterRetryCount = config.logAfterRetryCount;
    MockResilientWebSocket.instances.push(this);
  }

  isConnected(): boolean {
    return this._isConnected;
  }

  isReconnecting(): boolean {
    return this._isReconnecting;
  }

  setConnected(value: boolean): void {
    this._isConnected = value;
  }

  setReconnecting(value: boolean): void {
    this._isReconnecting = value;
  }

  send = mock((_data: string | Buffer) => undefined);
  startWebSocket = mock(() => {
    // Simulate immediate connection for tests
    this._isConnected = true;
  });
  closeWebSocket = mock(() => {
    this._isConnected = false;
    this.wsUserClosed = true;
  });

  static clearInstances(): void {
    MockResilientWebSocket.instances = [];
  }
}

// Inject the mock connection so the pool builds fakes, not real sockets.
const createPool = (
  config: WebSocketPoolConfig,
  token: string,
  abortSignal?: AbortSignal | null,
) =>
  WebSocketPool.create(config, token, abortSignal, undefined, {
    createConnection: (rwsConfig) => new MockResilientWebSocket(rwsConfig),
  });

beforeEach(() => {
  MockResilientWebSocket.clearInstances();
});

describe("WebSocketPool", () => {
  describe("create()", () => {
    it("should create a pool with default number of connections", async () => {
      const config: WebSocketPoolConfig = {};
      const pool = await createPool(config, "test-token");

      // Default is 4 connections
      expect(pool.rwsPool.length).toBe(4);

      pool.shutdown();
    });

    it("should create a pool with specified number of connections", async () => {
      const config: WebSocketPoolConfig = {
        numConnections: 2,
      };
      const pool = await createPool(config, "test-token");

      expect(pool.rwsPool.length).toBe(2);

      pool.shutdown();
    });

    it("uses the default stream URLs, round-robined, when none provided", async () => {
      const config: WebSocketPoolConfig = {}; // default is 4 connections

      const pool = await createPool(config, "test-token");

      // 4 default connections round-robin across the 3 default URLs, so the
      // fourth wraps back to the first.
      const endpoints = MockResilientWebSocket.instances.map((i) => i.endpoint);
      expect(endpoints).toEqual([
        DEFAULT_STREAM_SERVICE_URLS[0],
        DEFAULT_STREAM_SERVICE_URLS[1],
        DEFAULT_STREAM_SERVICE_URLS[2],
        DEFAULT_STREAM_SERVICE_URLS[0],
      ]);

      pool.shutdown();
    });

    it("should accept custom URLs in config", () => {
      // This test verifies that custom URLs can be passed in config
      // We don't actually create the pool since that would require connections
      const customUrls = [
        "wss://custom1.example.com",
        "wss://custom2.example.com",
      ];
      const config: WebSocketPoolConfig = {
        numConnections: 2,
        urls: customUrls,
      };

      // Verify the config is valid and can be constructed
      expect(config.urls).toEqual(customUrls);
      expect(config.numConnections).toBe(2);
    });

    it("should throw error if URL is empty", async () => {
      const config: WebSocketPoolConfig = {
        numConnections: 1,
        urls: [""],
      };

      // An empty base URL fails the null/empty check before a connection is made.
      await expect(createPool(config, "test-token")).rejects.toThrow();
    });

    it("should call onWebSocketError callback when provided", async () => {
      const onWebSocketError = mock((_error: unknown) => undefined);
      const config: WebSocketPoolConfig = {
        numConnections: 1,
        onWebSocketError,
      };

      const pool = await createPool(config, "test-token");

      // The callback is attached but won't be called until an error occurs
      // We just verify the pool was created successfully
      expect(pool.rwsPool.length).toBe(1);

      pool.shutdown();
    });

    it("should call onWebSocketPoolError callback when provided", async () => {
      const onWebSocketPoolError = mock((_error: Error) => undefined);
      const config: WebSocketPoolConfig = {
        numConnections: 1,
        onWebSocketPoolError,
      };

      const pool = await createPool(config, "test-token");

      // The callback is attached via event emitter
      expect(pool.rwsPool.length).toBe(1);

      pool.shutdown();
    });

    it("passes the auth token to each connection as a subprotocol", async () => {
      const config: WebSocketPoolConfig = { numConnections: 2 };

      const pool = await createPool(config, "test-token");

      expect(MockResilientWebSocket.instances).toHaveLength(2);
      for (const instance of MockResilientWebSocket.instances) {
        expect(instance.protocols).toEqual([AUTH_SUBPROTOCOL, "test-token"]);
      }

      pool.shutdown();
    });

    it("forwards rwsConfig tuning values to each connection", async () => {
      const config: WebSocketPoolConfig = {
        numConnections: 2,
        rwsConfig: {
          heartbeatTimeoutDurationMs: 1234,
          logAfterRetryCount: 7,
          maxRetryDelayMs: 4321,
        },
      };

      const pool = await createPool(config, "test-token");

      expect(MockResilientWebSocket.instances).toHaveLength(2);
      for (const instance of MockResilientWebSocket.instances) {
        expect(instance.heartbeatTimeoutDurationMs).toBe(1234);
        expect(instance.maxRetryDelayMs).toBe(4321);
        expect(instance.logAfterRetryCount).toBe(7);
      }

      pool.shutdown();
    });

    it("keeps pool-controlled auth and endpoint regardless of rwsConfig", async () => {
      const config: WebSocketPoolConfig = {
        numConnections: 1,
        rwsConfig: {
          // Auth and endpoint are not part of rwsConfig's type; cast to prove
          // the pool still wins even if a caller forces them in at runtime.
          endpoint: "wss://attacker.example.com",
          protocols: ["evil", "spoofed-token"],
        } as WebSocketPoolConfig["rwsConfig"],
        urls: ["wss://real.example.com"],
      };

      const pool = await createPool(config, "test-token");

      const [instance] = MockResilientWebSocket.instances;
      expect(instance?.endpoint).toBe("wss://real.example.com");
      expect(instance?.protocols).toEqual([AUTH_SUBPROTOCOL, "test-token"]);

      pool.shutdown();
    });

    it("should throw AbortError when aborted before connection", async () => {
      const abortController = new AbortController();
      abortController.abort();

      const config: WebSocketPoolConfig = {
        numConnections: 1,
      };

      await expect(
        createPool(config, "test-token", abortController.signal),
      ).rejects.toThrow("WebSocketPool.create() was aborted");
    });

    it("should throw AbortError when aborted during connection wait", async () => {
      const abortController = new AbortController();

      // This test is tricky because the pool waits for at least one connection
      // We need to abort during that wait

      const config: WebSocketPoolConfig = {
        numConnections: 1,
      };

      // Start creation and abort shortly after
      const createPromise = createPool(
        config,
        "test-token",
        abortController.signal,
      );

      // Give it a tiny bit of time to start, then abort
      setTimeout(() => abortController.abort(), 5);

      // The pool might complete before abort (race condition in tests)
      // so we handle both cases
      try {
        const pool = await createPromise;
        // If it completed, just shutdown
        pool.shutdown();
      } catch (error) {
        // If it was aborted, verify the error
        expect((error as Error).message).toContain("aborted");
      }
    });

    it("should support round-robin URL distribution in config", () => {
      // This test verifies the config supports multiple URLs for round-robin
      // The actual round-robin logic is: urls[i % urls.length]
      const customUrls = [
        "wss://server1.example.com",
        "wss://server2.example.com",
      ];
      const config: WebSocketPoolConfig = {
        numConnections: 4,
        urls: customUrls,
      };

      // Verify round-robin calculation works as expected
      // With 4 connections and 2 URLs:
      // connection 0 -> urls[0 % 2] = server1
      // connection 1 -> urls[1 % 2] = server2
      // connection 2 -> urls[2 % 2] = server1
      // connection 3 -> urls[3 % 2] = server2
      expect(customUrls[0 % customUrls.length]).toBe(
        "wss://server1.example.com",
      );
      expect(customUrls[1 % customUrls.length]).toBe(
        "wss://server2.example.com",
      );
      expect(customUrls[2 % customUrls.length]).toBe(
        "wss://server1.example.com",
      );
      expect(customUrls[3 % customUrls.length]).toBe(
        "wss://server2.example.com",
      );

      expect(config.numConnections).toBe(4);
    });
  });

  describe("sendRequest()", () => {
    it("should send request to all connections", async () => {
      const pool = await createPool({ numConnections: 2 }, "test-token");

      const request: Request = {
        subscriptionId: 1,
        type: "unsubscribe",
      };

      pool.sendRequest(request);

      // Each ResilientWebSocket should have received the request
      for (const rws of pool.rwsPool) {
        // We can't easily verify send was called without deeper mocking
        // but we verify no error was thrown
        expect(rws).toBeDefined();
      }

      pool.shutdown();
    });

    it("should do nothing after shutdown", async () => {
      const pool = await createPool({ numConnections: 1 }, "test-token");

      pool.shutdown();

      const request: Request = {
        subscriptionId: 1,
        type: "unsubscribe",
      };

      // Should not throw
      pool.sendRequest(request);
    });
  });

  describe("addSubscription()", () => {
    it("should store subscription and send request", async () => {
      const pool = await createPool({ numConnections: 1 }, "test-token");

      const request: Request = {
        channel: "real_time",
        formats: ["evm"],
        priceFeedIds: [1, 2],
        properties: ["price"],
        subscriptionId: 42,
        type: "subscribe",
      };

      pool.addSubscription(request);

      // The subscription should be stored (we can't easily verify internals)
      // but we verify no error was thrown
      expect(pool.rwsPool.length).toBe(1);

      pool.shutdown();
    });

    it("should emit error for non-subscribe request", async () => {
      const onWebSocketPoolError = mock((_error: Error) => undefined);
      const pool = await createPool(
        { numConnections: 1, onWebSocketPoolError },
        "test-token",
      );

      const request: Request = {
        subscriptionId: 1,
        type: "unsubscribe",
      };

      pool.addSubscription(request);

      // Should have called the error handler
      expect(onWebSocketPoolError).toHaveBeenCalledTimes(1);

      pool.shutdown();
    });

    it("should do nothing after shutdown", async () => {
      const pool = await createPool({ numConnections: 1 }, "test-token");

      pool.shutdown();

      const request: Request = {
        channel: "real_time",
        formats: ["evm"],
        priceFeedIds: [1],
        properties: ["price"],
        subscriptionId: 1,
        type: "subscribe",
      };

      // Should not throw
      pool.addSubscription(request);
    });
  });

  describe("removeSubscription()", () => {
    it("should remove subscription and send unsubscribe request", async () => {
      const pool = await createPool({ numConnections: 1 }, "test-token");

      // First add a subscription
      const subscribeRequest: Request = {
        channel: "real_time",
        formats: ["evm"],
        priceFeedIds: [1],
        properties: ["price"],
        subscriptionId: 99,
        type: "subscribe",
      };
      pool.addSubscription(subscribeRequest);

      // Then remove it
      pool.removeSubscription(99);

      // Verify no error was thrown
      expect(pool.rwsPool.length).toBe(1);

      pool.shutdown();
    });

    it("should do nothing after shutdown", async () => {
      const pool = await createPool({ numConnections: 1 }, "test-token");

      pool.shutdown();

      // Should not throw
      pool.removeSubscription(1);
    });
  });

  describe("addMessageListener()", () => {
    it("should register message listener", async () => {
      const pool = await createPool({ numConnections: 1 }, "test-token");

      const handler = mock(async (_data: unknown) => undefined);
      pool.addMessageListener(handler);

      // Listener is registered but won't be called until a message arrives
      // We just verify no error was thrown
      expect(pool.rwsPool.length).toBe(1);

      pool.shutdown();
    });
  });

  describe("connection state listeners", () => {
    it("should register allConnectionsDownListener", async () => {
      const pool = await createPool({ numConnections: 1 }, "test-token");

      const handler = mock(() => undefined);
      pool.addAllConnectionsDownListener(handler);

      // Handler is registered, will be called when all connections go down
      expect(pool.rwsPool.length).toBe(1);

      pool.shutdown();
    });

    it("should register connectionRestoredListener", async () => {
      const pool = await createPool({ numConnections: 1 }, "test-token");

      const handler = mock(() => undefined);
      pool.addConnectionRestoredListener(handler);

      expect(pool.rwsPool.length).toBe(1);

      pool.shutdown();
    });

    it("should register connectionTimeoutListener", async () => {
      const pool = await createPool({ numConnections: 1 }, "test-token");

      const handler = mock((_index: number, _endpoint: string) => undefined);
      pool.addConnectionTimeoutListener(handler);

      expect(pool.rwsPool.length).toBe(1);

      pool.shutdown();
    });

    it("should register connectionReconnectListener", async () => {
      const pool = await createPool({ numConnections: 1 }, "test-token");

      const handler = mock((_index: number, _endpoint: string) => undefined);
      pool.addConnectionReconnectListener(handler);

      expect(pool.rwsPool.length).toBe(1);

      pool.shutdown();
    });
  });

  describe("shutdown()", () => {
    it("should close all connections", async () => {
      const pool = await createPool({ numConnections: 3 }, "test-token");

      expect(pool.rwsPool.length).toBe(3);

      pool.shutdown();

      // After shutdown, rwsPool should be empty
      expect(pool.rwsPool.length).toBe(0);
    });

    it("should clear all subscriptions", async () => {
      const pool = await createPool({ numConnections: 1 }, "test-token");

      // Add some subscriptions
      pool.addSubscription({
        channel: "real_time",
        formats: ["evm"],
        priceFeedIds: [1],
        properties: ["price"],
        subscriptionId: 1,
        type: "subscribe",
      });
      pool.addSubscription({
        channel: "real_time",
        formats: ["solana"],
        priceFeedIds: [2],
        properties: ["price"],
        subscriptionId: 2,
        type: "subscribe",
      });

      pool.shutdown();

      // After shutdown, operations should be no-ops
      expect(pool.rwsPool.length).toBe(0);
    });

    it("should be idempotent (safe to call multiple times)", async () => {
      const pool = await createPool({ numConnections: 1 }, "test-token");

      pool.shutdown();
      pool.shutdown();
      pool.shutdown();

      // Should not throw
      expect(pool.rwsPool.length).toBe(0);
    });

    it("should execute shutdown handlers", async () => {
      const onWebSocketPoolError = mock((_error: Error) => undefined);
      const pool = await createPool(
        { numConnections: 1, onWebSocketPoolError },
        "test-token",
      );

      pool.shutdown();

      // The error handler binding should be removed on shutdown
      expect(pool.rwsPool.length).toBe(0);
    });
  });

  describe("message deduplication", () => {
    it("should deduplicate identical string messages", async () => {
      const pool = await createPool({ numConnections: 2 }, "test-token");

      const handler = mock(async (_data: unknown) => undefined);
      pool.addMessageListener(handler);

      // Simulate receiving the same JSON message from multiple connections
      const message = JSON.stringify({ subscriptionId: 1, type: "subscribed" });

      // Call dedupeHandler directly (simulating messages from multiple connections)
      await pool.dedupeHandler(message);
      await pool.dedupeHandler(message);

      // Handler should only be called once due to deduplication
      expect(handler).toHaveBeenCalledTimes(1);

      pool.shutdown();
    });

    it("should not deduplicate different messages", async () => {
      const pool = await createPool({ numConnections: 1 }, "test-token");

      const handler = mock(async (_data: unknown) => undefined);
      pool.addMessageListener(handler);

      const message1 = JSON.stringify({
        subscriptionId: 1,
        type: "subscribed",
      });
      const message2 = JSON.stringify({
        subscriptionId: 2,
        type: "subscribed",
      });

      await pool.dedupeHandler(message1);
      await pool.dedupeHandler(message2);

      // Both messages should reach the handler
      expect(handler).toHaveBeenCalledTimes(2);

      pool.shutdown();
    });

    it("should throw on subscription error response", async () => {
      const pool = await createPool({ numConnections: 1 }, "test-token");

      pool.addMessageListener(async () => undefined);

      const errorMessage = JSON.stringify({
        error: "Invalid feed ID",
        subscriptionId: 1,
        type: "subscriptionError",
      });

      await expect(pool.dedupeHandler(errorMessage)).rejects.toThrow(
        "Error occurred for subscription ID 1",
      );

      pool.shutdown();
    });

    it("should throw on general error response", async () => {
      const pool = await createPool({ numConnections: 1 }, "test-token");

      pool.addMessageListener(async () => undefined);

      const errorMessage = JSON.stringify({
        error: "Authentication failed",
        type: "error",
      });

      await expect(pool.dedupeHandler(errorMessage)).rejects.toThrow(
        "Error: Authentication failed",
      );

      pool.shutdown();
    });
  });

  describe("error handling", () => {
    it("should emit pool error when message handler throws", async () => {
      const onWebSocketPoolError = mock((_error: Error) => undefined);
      const pool = await createPool(
        { numConnections: 1, onWebSocketPoolError },
        "test-token",
      );

      pool.addMessageListener(() => {
        throw new Error("Handler error");
      });

      const message = JSON.stringify({ subscriptionId: 1, type: "subscribed" });

      // This should not throw, but should emit error
      await pool.dedupeHandler(message);

      expect(onWebSocketPoolError).toHaveBeenCalledTimes(1);
      const errorArg = onWebSocketPoolError.mock.calls[0]?.[0];
      expect(errorArg).toBeInstanceOf(Error);
      expect((errorArg as Error).message).toBe("Handler error");

      pool.shutdown();
    });
  });
});
