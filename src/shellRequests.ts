import type { ShellRequest } from "./types";

export type FileDropAction = "ignore" | "add" | "open" | "extract" | "ask" | "create";

export function fileDropAction(
  paths: string[],
  creating: boolean,
  archiveCanModify: boolean,
  isArchive: (path: string) => boolean,
): FileDropAction {
  if (!paths.length) return "ignore";
  if (creating) return "create";
  if (paths.every(isArchive)) {
    // An archive dropped on an editable archive could mean either; the user decides.
    if (archiveCanModify) return "ask";
    return paths.length === 1 ? "open" : "extract";
  }
  return archiveCanModify ? "add" : "create";
}

export async function drainShellRequests(
  take: () => Promise<ShellRequest[]>,
  handle: (request: ShellRequest) => Promise<void>,
) {
  for (const request of await take()) await handle(request);
}
