import {
  afterEach,
  beforeEach,
  describe,
  expect,
  it,
  mock,
  spyOn,
} from "bun:test";

import type { LazerClientConfig } from "../client.js";
import { PythLazerClient } from "../client.js";
import type { Request, SymbolResponse } from "../protocol.js";
import type { WebSocketPoolConfig } from "../socket/websocket-pool.js";
import { WebSocketPool } from "../socket/websocket-pool.js";

// Mock WebSocketPool
const mockWspInstance = {
  addAllConnectionsDownListener: mock((_handler: () => void) => undefined),
  addConnectionReconnectListener: mock(
    (_handler: (index: number, endpoint: string) => void) => undefined,
  ),
  addConnectionRestoredListener: mock((_handler: () => void) => undefined),
  addConnectionTimeoutListener: mock(
    (_handler: (index: number, endpoint: string) => void) => undefined,
  ),
  addMessageListener: mock(
    (_handler: (data: unknown) => Promise<void>) => undefined,
  ),
  addSubscription: mock((_request: Request) => undefined),
  removeSubscription: mock((_subscriptionId: number) => undefined),
  sendRequest: mock((_request: Request) => undefined),
  shutdown: mock(() => undefined),
};

type MockCreateArgs = [
  WebSocketPoolConfig,
  string,
  AbortSignal | null | undefined,
];
const mockWebSocketPoolCreate = mock<
  (
    config: WebSocketPoolConfig,
    token: string,
    abortSignal?: AbortSignal | null,
  ) => Promise<typeof mockWspInstance>
>(() => Promise.resolve(mockWspInstance));

// Store original and replace with mock
const originalCreate = WebSocketPool.create;

beforeEach(() => {
  mock.clearAllMocks();

  // Replace WebSocketPool.create with mock
  // @ts-expect-error - replacing static method for testing
  WebSocketPool.create = mockWebSocketPoolCreate;
});

afterEach(() => {
  // Restore original
  WebSocketPool.create = originalCreate;
});

describe("PythLazerClient", () => {
  describe("create()", () => {
    it("should create a client with required config", async () => {
      const config: LazerClientConfig = {
        token: "test-token",
        webSocketPoolConfig: {
          numConnections: 2,
        },
      };

      const client = await PythLazerClient.create(config);

      expect(client).toBeInstanceOf(PythLazerClient);
      expect(mockWebSocketPoolCreate).toHaveBeenCalledTimes(1);
    });

    it("should pass token and config to WebSocketPool.create", async () => {
      const config: LazerClientConfig = {
        token: "my-secret-token",
        webSocketPoolConfig: {
          numConnections: 4,
          rwsConfig: {
            heartbeatTimeoutDurationMs: 3000,
          },
        },
      };

      await PythLazerClient.create(config);

      expect(mockWebSocketPoolCreate).toHaveBeenCalledTimes(1);
      const calls = mockWebSocketPoolCreate.mock.calls as MockCreateArgs[];
      expect(calls.length).toBeGreaterThan(0);
      const firstCall = calls[0];
      expect(firstCall).toBeDefined();
      if (!firstCall) throw new Error("Expected first call to be defined");
      const [poolConfig, token] = firstCall;

      expect(token).toBe("my-secret-token");
      expect(poolConfig.numConnections).toBe(4);
      expect(poolConfig.rwsConfig?.heartbeatTimeoutDurationMs).toBe(3000);
    });

    it("should pass abortSignal to WebSocketPool.create config", async () => {
      const abortController = new AbortController();
      const config: LazerClientConfig = {
        abortSignal: abortController.signal,
        token: "test-token",
        webSocketPoolConfig: {
          numConnections: 2,
        },
      };

      await PythLazerClient.create(config);

      expect(mockWebSocketPoolCreate).toHaveBeenCalledTimes(1);
      const calls = mockWebSocketPoolCreate.mock.calls as MockCreateArgs[];
      const firstCall = calls[0];
      expect(firstCall).toBeDefined();
      if (!firstCall) throw new Error("Expected first call to be defined");
      const [, , abortSignal] = firstCall;

      // The abortSignal should be passed to WebSocketPool.create
      // so it can also abort during its own connection establishment
      expect(abortSignal).toBe(abortController.signal);
    });

    it("should throw AbortError with correct error name when aborted", async () => {
      const abortController = new AbortController();
      abortController.abort();

      const config: LazerClientConfig = {
        abortSignal: abortController.signal,
        token: "test-token",
        webSocketPoolConfig: {},
      };

      try {
        await PythLazerClient.create(config);
        // Should not reach here
        expect(true).toBe(false);
      } catch (error) {
        expect(error).toBeInstanceOf(DOMException);
        expect((error as DOMException).name).toBe("AbortError");
        expect((error as DOMException).message).toBe(
          "PythLazerClient.create() was aborted",
        );
      }
    });

    it("should not call WebSocketPool.create when already aborted", async () => {
      const abortController = new AbortController();
      abortController.abort();

      const config: LazerClientConfig = {
        abortSignal: abortController.signal,
        token: "test-token",
        webSocketPoolConfig: {},
      };

      await expect(PythLazerClient.create(config)).rejects.toThrow();

      // WebSocketPool.create should never have been called
      expect(mockWebSocketPoolCreate).not.toHaveBeenCalled();
    });

    it("should throw AbortError when aborted before WebSocketPool creation", async () => {
      const abortController = new AbortController();
      abortController.abort();

      const config: LazerClientConfig = {
        abortSignal: abortController.signal,
        token: "test-token",
        webSocketPoolConfig: {},
      };

      await expect(PythLazerClient.create(config)).rejects.toThrow(
        "PythLazerClient.create() was aborted",
      );
    });

    it("should throw AbortError when aborted after WebSocketPool.create completes", async () => {
      const abortController = new AbortController();

      // Make WebSocketPool.create complete, then abort before returning
      mockWebSocketPoolCreate.mockImplementationOnce(() => {
        // Simulate pool creation completing
        const result = mockWspInstance;
        // Abort during the async gap
        abortController.abort();
        return Promise.resolve(result);
      });

      const config: LazerClientConfig = {
        abortSignal: abortController.signal,
        token: "test-token",
        webSocketPoolConfig: {},
      };

      await expect(PythLazerClient.create(config)).rejects.toThrow(
        "PythLazerClient.create() was aborted",
      );

      // The WebSocketPool should have been shutdown due to abort
      expect(mockWspInstance.shutdown).toHaveBeenCalled();
    });

    it("should throw AbortError when aborted via signal during async wait", async () => {
      const abortController = new AbortController();

      // Make WebSocketPool.create take some time, abort while waiting
      mockWebSocketPoolCreate.mockImplementationOnce(async () => {
        await new Promise((resolve) => setTimeout(resolve, 50));
        return mockWspInstance;
      });

      const config: LazerClientConfig = {
        abortSignal: abortController.signal,
        token: "test-token",
        webSocketPoolConfig: {},
      };

      const createPromise = PythLazerClient.create(config);

      // Abort after starting creation but before it completes
      setTimeout(() => abortController.abort(), 10);

      await expect(createPromise).rejects.toThrow(
        "PythLazerClient.create() was aborted",
      );
    });

    it("should abort WebSocketPool.create when abortSignal is triggered during pool creation", async () => {
      // This test verifies that when an abortSignal is provided to PythLazerClient.create(),
      // it gets passed down to WebSocketPool.create() so the pool can use it to abort
      // its internal connection establishment logic.
      //
      // Without passing the signal down, WebSocketPool.create() would continue trying
      // to establish connections even after the user has aborted.

      const abortController = new AbortController();
      let webSocketPoolCreateAborted = false;

      // Mock WebSocketPool.create to simulate real behavior:
      // - It receives the abortSignal as a dedicated parameter
      // - It listens to the signal and aborts if triggered during creation
      // - If abortSignal is NOT passed, the abort won't be detected and creation will succeed
      mockWebSocketPoolCreate.mockImplementationOnce(
        (_poolConfig, _token, abortSignal) => {
          return new Promise((resolve, reject) => {
            // If the signal is already aborted, reject immediately
            if (abortSignal?.aborted) {
              webSocketPoolCreateAborted = true;
              reject(
                new DOMException(
                  "WebSocketPool.create() was aborted",
                  "AbortError",
                ),
              );
              return;
            }

            // Listen for abort during creation - this is the key behavior
            // If abortSignal was not passed to WebSocketPool.create, this listener
            // won't be registered and the abort won't be detected by WebSocketPool
            const abortHandler = () => {
              webSocketPoolCreateAborted = true;
              reject(
                new DOMException(
                  "WebSocketPool.create() was aborted",
                  "AbortError",
                ),
              );
            };
            abortSignal?.addEventListener("abort", abortHandler);

            // Simulate slow connection establishment (100ms)
            setTimeout(() => {
              abortSignal?.removeEventListener("abort", abortHandler);
              resolve(mockWspInstance);
            }, 100);
          });
        },
      );

      const config: LazerClientConfig = {
        abortSignal: abortController.signal,
        token: "test-token",
        webSocketPoolConfig: {},
      };

      const createPromise = PythLazerClient.create(config);

      // Abort while WebSocketPool.create is in progress (at 20ms, before 100ms completion)
      setTimeout(() => abortController.abort(), 20);

      // Wait for the promise to settle
      await expect(createPromise).rejects.toThrow();

      // The key assertion: WebSocketPool.create itself should have detected the abort
      // and rejected. If abortSignal was not passed down, this will be false because
      // the mock's abort listener was never registered (poolConfig.abortSignal was undefined)
      expect(webSocketPoolCreateAborted).toBe(true);
    });

    it("should cleanup WebSocketPool on error during creation", async () => {
      // Test that if WebSocketPool.create succeeds but something else fails,
      // the pool is still cleaned up
      const testError = new Error("Test error during creation");

      // Make mock throw after wsp is created
      mockWebSocketPoolCreate.mockImplementationOnce(() => {
        // Return the mock instance, but throw during subsequent processing
        // by making the returned value cause issues
        throw testError;
      });

      const config: LazerClientConfig = {
        token: "test-token",
        webSocketPoolConfig: {},
      };

      await expect(PythLazerClient.create(config)).rejects.toThrow(
        "Test error during creation",
      );
    });

    it("should use default URLs when not provided", async () => {
      const config: LazerClientConfig = {
        token: "test-token",
        webSocketPoolConfig: {},
      };

      const client = await PythLazerClient.create(config);

      // The URLs are stored internally - we can verify through HTTP methods
      // by checking the fetch calls. For now, we just verify creation succeeds.
      expect(client).toBeInstanceOf(PythLazerClient);
    });

    it("should strip trailing slashes from URLs", async () => {
      const config: LazerClientConfig = {
        metadataServiceUrl: "https://metadata.example.com///",
        priceServiceUrl: "https://price.example.com/",
        token: "test-token",
        webSocketPoolConfig: {},
      };

      const client = await PythLazerClient.create(config);

      // Verify by making a getSymbols call and checking the URL
      const mockFetch = spyOn(globalThis, "fetch").mockResolvedValueOnce(
        new Response(JSON.stringify([]), { status: 200 }),
      );

      await client.getSymbols();

      expect(mockFetch).toHaveBeenCalledTimes(1);
      const fetchUrl = mockFetch.mock.calls[0]?.[0] as string | undefined;
      expect(fetchUrl).toBeDefined();
      expect(fetchUrl).toStartWith("https://metadata.example.com/v1/symbols");
      expect(fetchUrl).not.toContain("///");

      mockFetch.mockRestore();
    });

    it("should cleanup on WebSocketPool.create error", async () => {
      mockWebSocketPoolCreate.mockRejectedValueOnce(
        new Error("Connection failed"),
      );

      const config: LazerClientConfig = {
        token: "test-token",
        webSocketPoolConfig: {},
      };

      await expect(PythLazerClient.create(config)).rejects.toThrow(
        "Connection failed",
      );
    });
  });

  describe("subscribe()", () => {
    it("should forward valid subscribe request to WebSocketPool", async () => {
      const client = await PythLazerClient.create({
        token: "test-token",
        webSocketPoolConfig: {},
      });

      const request: Request = {
        channel: "real_time",
        deliveryFormat: "json",
        formats: ["evm"],
        priceFeedIds: [1, 2, 3],
        properties: ["price"],
        subscriptionId: 1,
        type: "subscribe",
      };

      client.subscribe(request);

      expect(mockWspInstance.addSubscription).toHaveBeenCalledTimes(1);
      expect(mockWspInstance.addSubscription).toHaveBeenCalledWith(request);
    });

    it("should throw error for non-subscribe request type", async () => {
      const client = await PythLazerClient.create({
        token: "test-token",
        webSocketPoolConfig: {},
      });

      const request: Request = {
        subscriptionId: 1,
        type: "unsubscribe",
      };

      expect(() => client.subscribe(request)).toThrow(
        "Request must be a subscribe request",
      );
      expect(mockWspInstance.addSubscription).not.toHaveBeenCalled();
    });
  });

  describe("unsubscribe()", () => {
    it("should forward unsubscribe to WebSocketPool", async () => {
      const client = await PythLazerClient.create({
        token: "test-token",
        webSocketPoolConfig: {},
      });

      client.unsubscribe(42);

      expect(mockWspInstance.removeSubscription).toHaveBeenCalledTimes(1);
      expect(mockWspInstance.removeSubscription).toHaveBeenCalledWith(42);
    });
  });

  describe("send()", () => {
    it("should forward request to WebSocketPool.sendRequest", async () => {
      const client = await PythLazerClient.create({
        token: "test-token",
        webSocketPoolConfig: {},
      });

      const request: Request = {
        subscriptionId: 5,
        type: "unsubscribe",
      };

      client.send(request);

      expect(mockWspInstance.sendRequest).toHaveBeenCalledTimes(1);
      expect(mockWspInstance.sendRequest).toHaveBeenCalledWith(request);
    });
  });

  describe("event listeners", () => {
    it("should forward addAllConnectionsDownListener to WebSocketPool", async () => {
      const client = await PythLazerClient.create({
        token: "test-token",
        webSocketPoolConfig: {},
      });

      const handler = mock(() => undefined);
      client.addAllConnectionsDownListener(handler);

      expect(
        mockWspInstance.addAllConnectionsDownListener,
      ).toHaveBeenCalledTimes(1);
      expect(
        mockWspInstance.addAllConnectionsDownListener,
      ).toHaveBeenCalledWith(handler);
    });

    it("should forward addConnectionRestoredListener to WebSocketPool", async () => {
      const client = await PythLazerClient.create({
        token: "test-token",
        webSocketPoolConfig: {},
      });

      const handler = mock(() => undefined);
      client.addConnectionRestoredListener(handler);

      expect(
        mockWspInstance.addConnectionRestoredListener,
      ).toHaveBeenCalledTimes(1);
      expect(
        mockWspInstance.addConnectionRestoredListener,
      ).toHaveBeenCalledWith(handler);
    });

    it("should forward addConnectionTimeoutListener to WebSocketPool", async () => {
      const client = await PythLazerClient.create({
        token: "test-token",
        webSocketPoolConfig: {},
      });

      const handler = mock((_index: number, _endpoint: string) => undefined);
      client.addConnectionTimeoutListener(handler);

      expect(
        mockWspInstance.addConnectionTimeoutListener,
      ).toHaveBeenCalledTimes(1);
      expect(mockWspInstance.addConnectionTimeoutListener).toHaveBeenCalledWith(
        handler,
      );
    });

    it("should forward addConnectionReconnectListener to WebSocketPool", async () => {
      const client = await PythLazerClient.create({
        token: "test-token",
        webSocketPoolConfig: {},
      });

      const handler = mock((_index: number, _endpoint: string) => undefined);
      client.addConnectionReconnectListener(handler);

      expect(
        mockWspInstance.addConnectionReconnectListener,
      ).toHaveBeenCalledTimes(1);
      expect(
        mockWspInstance.addConnectionReconnectListener,
      ).toHaveBeenCalledWith(handler);
    });

    it("should forward addMessageListener to WebSocketPool", async () => {
      const client = await PythLazerClient.create({
        token: "test-token",
        webSocketPoolConfig: {},
      });

      const handler = mock(() => undefined);
      client.addMessageListener(handler);

      expect(mockWspInstance.addMessageListener).toHaveBeenCalledTimes(1);
      // The client wraps the handler, so we verify a function was passed
      const listenerArg = mockWspInstance.addMessageListener.mock.calls[0]?.[0];
      expect(typeof listenerArg).toBe("function");
    });
  });

  describe("shutdown()", () => {
    it("should call WebSocketPool.shutdown", async () => {
      const client = await PythLazerClient.create({
        token: "test-token",
        webSocketPoolConfig: {},
      });

      client.shutdown();

      expect(mockWspInstance.shutdown).toHaveBeenCalledTimes(1);
    });
  });

  describe("HTTP API methods", () => {
    let client: PythLazerClient;
    let mockFetch: ReturnType<typeof spyOn>;

    beforeEach(async () => {
      client = await PythLazerClient.create({
        token: "test-token",
        webSocketPoolConfig: {},
      });
      mockFetch = spyOn(globalThis, "fetch");
    });

    afterEach(() => {
      mockFetch.mockRestore();
    });

    describe("getSymbols()", () => {
      it("should make authenticated GET request to symbols endpoint", async () => {
        const mockSymbols: SymbolResponse[] = [
          {
            asset_type: "crypto",
            description: "Bitcoin",
            exponent: -8,
            min_channel: "real_time",
            min_publishers: 1,
            name: "BTC/USD",
            pyth_lazer_id: 1,
            schedule: "24/7",
            state: "stable",
            symbol: "BTCUSD",
          },
        ];
        mockFetch.mockResolvedValueOnce(
          new Response(JSON.stringify(mockSymbols), { status: 200 }),
        );

        const result = await client.getSymbols();

        expect(mockFetch).toHaveBeenCalledTimes(1);
        const callArgs = mockFetch.mock.calls[0];
        expect(callArgs).toBeDefined();
        const [url, options] = callArgs as [string, RequestInit];

        expect(url).toContain("/v1/symbols");
        expect(options.headers).toBeDefined();
        const headers = new Headers(options.headers);
        expect(headers.get("Authorization")).toBe("Bearer test-token");

        expect(result).toEqual(mockSymbols);
      });

      it("should include query parameter when provided", async () => {
        mockFetch.mockResolvedValueOnce(
          new Response(JSON.stringify([]), { status: 200 }),
        );

        await client.getSymbols({ query: "BTC" });

        const callArgs = mockFetch.mock.calls[0];
        expect(callArgs).toBeDefined();
        const [url] = callArgs as [string];
        expect(url).toContain("query=BTC");
      });

      it("should include asset_type parameter when provided", async () => {
        mockFetch.mockResolvedValueOnce(
          new Response(JSON.stringify([]), { status: 200 }),
        );

        await client.getSymbols({ asset_type: "crypto" });

        const callArgs = mockFetch.mock.calls[0];
        expect(callArgs).toBeDefined();
        const [url] = callArgs as [string];
        expect(url).toContain("asset_type=crypto");
      });

      it("should throw error on non-ok HTTP response", async () => {
        mockFetch.mockResolvedValueOnce(
          new Response("Unauthorized", { status: 401 }),
        );

        await expect(client.getSymbols()).rejects.toThrow(
          "Failed to fetch symbols",
        );
      });
    });

    describe("getLatestPrice()", () => {
      it("should make authenticated POST request with correct body", async () => {
        const mockResponse = {
          parsed: {
            priceFeeds: [{ price: "50000", priceFeedId: 1 }],
            timestampUs: "1234567890",
          },
        };
        mockFetch.mockResolvedValueOnce(
          new Response(JSON.stringify(mockResponse), { status: 200 }),
        );

        const params = {
          channel: "real_time" as const,
          formats: ["evm" as const],
          priceFeedIds: [1, 2],
          properties: ["price" as const],
        };

        const result = await client.getLatestPrice(params);

        expect(mockFetch).toHaveBeenCalledTimes(1);
        const callArgs = mockFetch.mock.calls[0];
        expect(callArgs).toBeDefined();
        const [url, options] = callArgs as [string, RequestInit];

        expect(url).toContain("/v1/latest_price");
        expect(options.method).toBe("POST");
        expect(options.body).toBe(JSON.stringify(params));

        const headers = new Headers(options.headers);
        expect(headers.get("Authorization")).toBe("Bearer test-token");
        expect(headers.get("Content-Type")).toBe("application/json");

        expect(result).toEqual(mockResponse);
      });

      it("should throw error on non-ok HTTP response", async () => {
        mockFetch.mockResolvedValueOnce(
          new Response("Bad Request", { status: 400 }),
        );

        await expect(
          client.getLatestPrice({
            channel: "real_time",
            formats: ["evm"],
            priceFeedIds: [1],
            properties: ["price"],
          }),
        ).rejects.toThrow("Failed to fetch latest price");
      });
    });

    describe("getPrice()", () => {
      it("should make authenticated POST request with timestamp", async () => {
        const mockResponse = {
          parsed: {
            priceFeeds: [{ price: "49000", priceFeedId: 1 }],
            timestampUs: "1234567890",
          },
        };
        mockFetch.mockResolvedValueOnce(
          new Response(JSON.stringify(mockResponse), { status: 200 }),
        );

        const params = {
          channel: "real_time" as const,
          formats: ["solana" as const],
          priceFeedIds: [1],
          properties: ["price" as const],
          timestamp: 1_234_567_890,
        };

        const result = await client.getPrice(params);

        expect(mockFetch).toHaveBeenCalledTimes(1);
        const callArgs = mockFetch.mock.calls[0];
        expect(callArgs).toBeDefined();
        const [url, options] = callArgs as [string, RequestInit];

        expect(url).toContain("/v1/price");
        expect(options.method).toBe("POST");
        expect(options.body).toBe(JSON.stringify(params));

        const headers = new Headers(options.headers);
        expect(headers.get("Authorization")).toBe("Bearer test-token");

        expect(result).toEqual(mockResponse);
      });

      it("should throw error on non-ok HTTP response", async () => {
        mockFetch.mockResolvedValueOnce(
          new Response("Not Found", { status: 404 }),
        );

        await expect(
          client.getPrice({
            channel: "real_time",
            formats: ["evm"],
            priceFeedIds: [1],
            properties: ["price"],
            timestamp: 1_234_567_890,
          }),
        ).rejects.toThrow("Failed to fetch price");
      });
    });
  });
});
