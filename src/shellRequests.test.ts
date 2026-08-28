import { describe, expect, it, vi } from "vitest";
import type { ShellRequest } from "./types";
import { drainShellRequests } from "./shellRequests";

describe("shell request queue", () => {
  it("handles queued requests once and in order", async () => {
    const requests: ShellRequest[] = [
      { version: 1, action: "open", paths: ["/tmp/one.zip"], createdAt: 1, nonce: "1" },
      { version: 1, action: "test_archive", paths: ["/tmp/two.zip"], createdAt: 2, nonce: "2" },
    ];
    let queue = requests;
    const take = async () => {
      const taken = queue;
      queue = [];
      return taken;
    };
    const handle = vi.fn(async (_request: ShellRequest) => undefined);
    await drainShellRequests(take, handle);
    await drainShellRequests(take, handle);
    expect(handle.mock.calls.map(([request]) => request.action)).toEqual(["open", "test_archive"]);
  });
});
