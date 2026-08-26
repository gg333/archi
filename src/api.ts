import { invoke } from "@tauri-apps/api/core";
import type {
  ArchiveDocument,
  ArchiveError,
  ArchiveFormat,
  CompressionLevel,
  ConflictPolicy,
  EntryPage,
  ExtractResult,
  JobSnapshot,
  Settings,
  ShellIntegrationStatus,
  ShellRequest,
  SortKey,
  TestResult,
} from "./types";

export function openArchive(path: string, password?: string): Promise<ArchiveDocument> {
  return invoke("open_archive", { path, password });
}

export function extractArchive(
  path: string,
  destination: string,
  conflictPolicy: ConflictPolicy,
  entries?: string[],
  password?: string,
  maxExpandedBytes = 10 * 1024 ** 3,
  allowUnbounded = false,
): Promise<ExtractResult> {
  return invoke("start_extract", {
    path,
    destination,
    conflictPolicy,
    entries,
    password,
    maxExpandedBytes,
    allowUnbounded,
  });
}

export function createArchive(
  inputs: string[],
  output: string,
  format: ArchiveFormat,
  compression: CompressionLevel,
  password?: string,
  passwordConfirmation?: string,
): Promise<ArchiveDocument> {
  return invoke("create_archive", {
    inputs,
    output,
    format,
    compression,
    password,
    passwordConfirmation,
  });
}

export function testArchive(path: string, password?: string): Promise<TestResult> {
  return invoke("test_archive", { path, password });
}

export function jobStatus(): Promise<JobSnapshot | null> {
  return invoke("job_status");
}

export function entryPage(
  path: string,
  folder: string,
  query: string,
  sort: SortKey,
  descending: boolean,
  page: number,
  pageSize: number,
  showHidden: boolean,
): Promise<EntryPage> {
  return invoke("entry_page", {
    path,
    folder,
    query,
    sort,
    descending,
    page,
    pageSize,
    showHidden,
  });
}

export function archiveChanged(path: string): Promise<boolean> {
  return invoke("archive_changed", { path });
}

export function entryIcons(keys: string[]): Promise<Record<string, string>> {
  return invoke("entry_icons", { keys });
}

export function getSettings(): Promise<Settings> {
  return invoke("get_settings");
}

export function saveSettings(settings: Settings): Promise<Settings> {
  return invoke("save_settings", { settings });
}

export function resetSettings(): Promise<Settings> {
  return invoke("reset_settings");
}

export function recordDiagnostic(event: string, code?: string): Promise<void> {
  return invoke("record_diagnostic", { event, code });
}

export function clearDiagnostics(): Promise<void> {
  return invoke("clear_diagnostics");
}

export function exportDiagnostics(destination: string): Promise<void> {
  return invoke("export_diagnostics", { destination });
}

export function openDestination(path: string): Promise<void> {
  return invoke("open_destination", { path });
}

export function cancelJob(): Promise<boolean> {
  return invoke("cancel_job");
}

export function takeShellRequests(): Promise<ShellRequest[]> {
  return invoke("take_shell_requests");
}

export function shellIntegrationStatus(): Promise<ShellIntegrationStatus> {
  return invoke("shell_integration_status");
}

export function defaultZipOutput(inputs: string[]): Promise<string> {
  return invoke("default_zip_output", { inputs });
}

export function archiveError(error: unknown): ArchiveError {
  if (
    typeof error === "object" &&
    error !== null &&
    "code" in error &&
    "message" in error
  ) {
    return error as ArchiveError;
  }
  return {
    code: "unexpected_error",
    message: error instanceof Error ? error.message : String(error),
  };
}
