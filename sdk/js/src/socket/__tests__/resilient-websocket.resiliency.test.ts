import { afterEach, beforeEach, describe, expect, it, mock } from "bun:test";
import { setTimeout } from "node:timers/promises";

import { ResilientWebSocket } from "../resilient-websocket.js";

let shouldThrowOnConstructor = false;

type Listener = (...args: unknown[]) => void;

class MockWebSocket {
  static OPEN = 1;
  readyState = 1;
  private listeners = new Map<string, Listener[]>();

  // biome-ignore lint/suspicious/noExplicitAny: matching WebSocket constructor signature
  constructor(_url: string, _protocols?: any) {
    if (shouldThrowOnConstructor) {
      throw new Error("Synchronous constructor error");
    }
    mockInstances.push(this);
  }

  addEventListener = mock((event: string, listener: Listener) => {
    const existing = this.listeners.get(event) ?? [];
    existing.push(listener);
    this.listeners.set(event, existing);
  });

  removeEventListener = mock(() => undefined);
  close = mock(() => undefined);
  terminate = mock(() => undefined);
  on = mock(() => undefined);
  send = mock(() => undefined);

  emit(event: string, data?: unknown) {
    for (const listener of this.listeners.get(event) ?? []) {
      listener(data);
    }
  }
}

const mockInstances: MockWebSocket[] = [];

mock.module("isomorphic-ws", () => ({
  default: MockWebSocket,
}));

function lastMock(): MockWebSocket {
  const instance = mockInstances[mockInstances.length - 1];
  if (!instance) throw new Error("No mock WebSocket instances");
  return instance;
}

describe("ResilientWebSocket Resiliency", () => {
  let rws: ResilientWebSocket;

  beforeEach(() => {
    shouldThrowOnConstructor = false;
    mockInstances.length = 0;
  });

  afterEach(() => {
    rws?.closeWebSocket();
  });

  it("should not crash when WebSocket constructor throws synchronously", async () => {
    shouldThrowOnConstructor = true;

    rws = new ResilientWebSocket({
      endpoint: "wss://example.com",
      maxRetryDelayMs: 10,
    });

    // This should not throw/crash the process
    rws.startWebSocket();

    await setTimeout(50);
    expect(rws.isReconnecting()).toBe(true);
  });

  it("should schedule reconnect after constructor error", async () => {
    shouldThrowOnConstructor = true;

    rws = new ResilientWebSocket({
      endpoint: "wss://example.com",
      maxRetryDelayMs: 10,
    });

    rws.startWebSocket();

    // After the retry delay, it should attempt to create a new WebSocket.
    // Allow the constructor to succeed on the retry.
    shouldThrowOnConstructor = false;
    await setTimeout(50);

    expect(mockInstances.length).toBeGreaterThanOrEqual(1);
  });

  it("should not crash when onReconnect callback throws", async () => {
    rws = new ResilientWebSocket({
      endpoint: "wss://example.com",
    });

    let onReconnectCalled = false;
    rws.onReconnect = () => {
      onReconnectCalled = true;
      throw new Error("User callback error");
    };

    rws.startWebSocket();
    lastMock().emit("open");

    await setTimeout(10);
    expect(onReconnectCalled).toBe(true);
  });

  it("should not crash when onError callback throws", async () => {
    rws = new ResilientWebSocket({
      endpoint: "wss://example.com",
    });

    let onErrorCalled = false;
    rws.onError = () => {
      onErrorCalled = true;
      throw new Error("User error callback error");
    };

    rws.startWebSocket();
    lastMock().emit("error", { message: "test error" });

    await setTimeout(10);
    expect(onErrorCalled).toBe(true);
  });

  it("should not crash when onMessage callback throws", async () => {
    rws = new ResilientWebSocket({
      endpoint: "wss://example.com",
    });

    let onMessageCalled = false;
    rws.onMessage = () => {
      onMessageCalled = true;
      throw new Error("User message callback error");
    };

    rws.startWebSocket();
    lastMock().emit("open");
    lastMock().emit("message", { data: "test" });

    await setTimeout(10);
    expect(onMessageCalled).toBe(true);
  });

  it("should not crash when onTimeout callback throws", async () => {
    rws = new ResilientWebSocket({
      endpoint: "wss://example.com",
      heartbeatTimeoutDurationMs: 10,
    });

    let onTimeoutCalled = false;
    rws.onTimeout = () => {
      onTimeoutCalled = true;
      throw new Error("User timeout callback error");
    };

    rws.startWebSocket();
    lastMock().emit("open");

    // Wait for heartbeat timeout to fire
    await setTimeout(50);
    expect(onTimeoutCalled).toBe(true);
  });

  it("should call terminate/close (not handleReconnect directly) on heartbeat timeout", async () => {
    rws = new ResilientWebSocket({
      endpoint: "wss://example.com",
      heartbeatTimeoutDurationMs: 10,
    });

    rws.startWebSocket();

    const ws = lastMock();
    ws.emit("open");

    // Wait for heartbeat timeout
    await setTimeout(50);

    // terminate should have been called on the websocket
    expect(ws.terminate).toHaveBeenCalledTimes(1);

    // Now simulate the close event that terminate would trigger.
    // The reconnect should happen exactly once (from the close handler).
    ws.emit("close", { code: 1006 });

    await setTimeout(50);

    // A new WebSocket should have been created for the reconnect
    expect(mockInstances.length).toBe(2);
  });
});
