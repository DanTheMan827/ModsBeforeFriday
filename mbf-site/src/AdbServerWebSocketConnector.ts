import type { AdbIncomingSocketHandler, AdbServerClient } from "@yume-chan/adb";
import {
  MaybeConsumable,
  ReadableStream,
  ReadableWritablePair
} from "@yume-chan/stream-extra";

export const bridgeWebsocketAddress = "ws://127.0.0.1:25037/bridge";
export const bridgePingAddress = "http://127.0.0.1:25037/bridge/ping";

/**
 * Checks if the bridge is running
 * @returns Whether the bridge is running
 */
export async function checkForBridge(): Promise<boolean> {
  // Ping the bridge to see if the bridge is running
  try {
    const response = await fetch(bridgePingAddress, { method: "GET" });
    if (response.ok) {
      return true;
    }
  } catch (e) {
    return false;
  }

  return false;
}

/**
 * Returns a promise that resolves after a given delay.
 * @param ms - Number of milliseconds to delay.
 */
function delay(ms: number): Promise<void> {
  return new Promise((resolve) => setTimeout(resolve, ms));
}

/**
 * A generic Deferred class that encapsulates a Promise which can be resolved or rejected externally.
 */
class Deferred<T> {
  private promise: Promise<T>;
  private resolveFn!: (value: T | PromiseLike<T>) => void;
  private rejectFn!: (reason?: any) => void;
  private state: "running" | "resolved" | "rejected" = "running";

  constructor() {
    this.promise = new Promise<T>((resolve, reject) => {
      this.resolveFn = resolve;
      this.rejectFn = reject;
    });
  }

  public getPromise(): Promise<T> {
    return this.promise;
  }

  public getState(): "running" | "resolved" | "rejected" {
    return this.state;
  }

  public resolve(value: T): void {
    this.resolveFn(value);
    this.state = "resolved";
  }

  public reject(reason?: any): void {
    this.rejectFn(reason);
    this.state = "rejected";
  }
}

interface Socket extends ReadableWritablePair<Uint8Array, Uint8Array>{
  extensions: string;
  protocol: string;
}

/**
 * Wraps a WebSocket connection into Readable and Writable streams.
 */
class WebSocketConnection {
  public url: string;
  private socket: WebSocket;
  // Deferred that resolves when the connection opens.
  private opened: Deferred<Socket>;
  // Deferred that resolves when the socket closes.
  private closed: Deferred<{ closeCode: number; reason: string }>;

  public getOpened(): Promise<Socket> {
    return this.opened.getPromise();
  }

  public getClosed(): Promise<{ closeCode: number; reason: string }> {
    return this.closed.getPromise();
  }

  constructor(url: string, options?: { protocols?: string | string[] }) {
    this.url = url;
    this.socket = new WebSocket(url, options?.protocols);
    this.socket.binaryType = "arraybuffer";
    this.opened = new Deferred();
    this.closed = new Deferred();

    let hasOpened = false;

    // When the socket opens, resolve the deferred with connection details.
    this.socket.onopen = () => {
      hasOpened = true;
      this.opened.resolve({
        extensions: this.socket.extensions,
        protocol: this.socket.protocol,
        readable: new ReadableStream<Uint8Array<ArrayBufferLike>>({
          start: (controller) => {
            this.socket.onmessage = (event: MessageEvent<Uint8Array>) => {
              if (typeof event.data === "string") {
                controller.enqueue(event.data);
              } else {
                controller.enqueue(new Uint8Array(event.data));
              }
            };

            this.socket.onerror = () => {
              controller.error(new Error("WebSocket error"));
            };

            this.socket.onclose = (event) => {
              try {
                controller.close();
              } catch {
                // Ignore errors during close
              }
              this.closed.resolve({
                closeCode: event.code,
                reason: event.reason,
              });
            };
          },
        }),
        writable: new MaybeConsumable.WritableStream<Uint8Array>({
          write: async (chunk: Uint8Array) => {
            // Wait until bufferedAmount is low enough.
            //while (this.socket.bufferedAmount > 8388608) {
            //    await delay(10);
            //}
            this.socket.send(chunk);
          },
        }),
      });
    };

    // If an error occurs before the socket is opened, reject the opened deferred.
    this.socket.onerror = () => {
      if (!hasOpened) {
        this.opened.reject(new Error("WebSocket connection error"));
      }
    };
  }

  public close(closeInfo?: { closeCode?: number; reason?: string }): void {
    this.socket.close(closeInfo?.closeCode, closeInfo?.reason);
  }
}

/**
 * Creates a WebSocket connection to the ADB server bridge and returns an object
 * conforming to AdbServerClient.ServerConnection.
 *
 * @param onTimeout - Callback invoked if the connection times out.
 */
export async function createWebSocketBridge(
  onTimeout: () => void
): Promise<AdbServerClient.ServerConnection> {
  const connection = new WebSocketConnection(bridgeWebsocketAddress);

  let hasOpened = false;

  // Wait for the connection to open or timeout after 5000ms.
  const connectionResult = await Promise.race([
    connection.getOpened(),
    new Promise<never>((_, reject) =>
      setTimeout(() => {
        if (!hasOpened) onTimeout();
        reject(new Error("WebSocket connection timed out"));
      }, 5000)
    ),
  ]);
  hasOpened = true;

  // Get a writer from the writable stream.
  const writer = connectionResult.writable.getWriter();

  return {
    readable: connectionResult.readable,
    writable: new MaybeConsumable.WritableStream<Uint8Array>({
      write: (chunk) => writer.write(chunk),
      close: () => writer.close(),
    }),
    close: () => connection.close(),
    closed: connection.getClosed().then(() => { }),
  };
}

/**
 * An `AdbServerClient.ServerConnector` implementation for Node.js.
 */
export class AdbServerWebSocketConnector
  implements AdbServerClient.ServerConnector {

  constructor() { }

  async connect(): Promise<AdbServerClient.ServerConnection> {
    return createWebSocketBridge(console.error);
  }

  async addReverseTunnel(
    handler: AdbIncomingSocketHandler,
    address?: string,
  ): Promise<string> {
    throw new Error("Method not implemented.");
  }

  removeReverseTunnel(address: string) {
    throw new Error("Method not implemented.");
  }

  clearReverseTunnels() {
    throw new Error("Method not implemented.");
  }
}
