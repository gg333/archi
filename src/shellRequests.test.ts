import { describe, expect, it, vi } from "vitest";
import type { ShellRequest } from "./types";
import { drainShellRequests, fileDropAction } from "./shellRequests";

describe("file drop routing", () => {
  const isArchive = (path: string) => path.endsWith(".zip") || path.endsWith(".7z");

  it("routes drops by the current workspace", () => {
    expect(fileDropAction([], false, false, isArchive)).toBe("ignore");
    expect(fileDropAction(["/tmp/report.pdf"], true, false, isArchive)).toBe("create");
    expect(fileDropAction(["/tmp/nested.zip"], false, true, isArchive)).toBe("add");
    expect(fileDropAction(["/tmp/one.zip", "/tmp/two.7z"], false, false, isArchive)).toBe("extract");
    expect(fileDropAction(["/tmp/one.zip", "/tmp/report.pdf"], false, false, isArchive)).toBe("create");
  });
});

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
