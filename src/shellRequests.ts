import type { ShellRequest } from "./types";

export async function drainShellRequests(
  take: () => Promise<ShellRequest[]>,
  handle: (request: ShellRequest) => Promise<void>,
) {
  for (const request of await take()) await handle(request);
}
