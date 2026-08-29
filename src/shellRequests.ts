import type { ShellRequest } from "./types";

export type FileDropAction = "ignore" | "add" | "extract" | "create";

export function fileDropAction(
  paths: string[],
  creating: boolean,
  archiveCanModify: boolean,
  isArchive: (path: string) => boolean,
): FileDropAction {
  if (!paths.length) return "ignore";
  if (creating) return "create";
  if (archiveCanModify) return "add";
  return paths.every(isArchive) ? "extract" : "create";
}

export async function drainShellRequests(
  take: () => Promise<ShellRequest[]>,
  handle: (request: ShellRequest) => Promise<void>,
) {
  for (const request of await take()) await handle(request);
}
