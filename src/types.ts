export interface ArchiveEntry {
  path: string;
  isDirectory: boolean;
  size: number | null;
  packedSize: number | null;
  modified: string | null;
  encrypted: boolean;
  method: string | null;
  isLink: boolean;
  linkTarget: string | null;
}

export interface ArchiveDocument {
  path: string;
  name: string;
  engineVersion: string;
  entryCount: number;
  totalBytes: number;
  encrypted: boolean;
  skippedLinks: number;
  comment: string | null;
  canModify: boolean;
  volumeCount: number;
}

export type SortKey = "name" | "size" | "packedSize" | "ratio" | "modified";

export interface EntryPage {
  folder: string;
  entries: ArchiveEntry[];
  page: number;
  pageSize: number;
  total: number;
  totalPages: number;
}

export interface ArchiveFolder {
  path: string;
  name: string;
  hasChildren: boolean;
}

export interface ExtractResult {
  destination: string;
  filesExtracted: number;
  filesSkipped: number;
  renamed: number;
  elapsedMs: number;
  warningCount: number;
}

export type ConflictPolicy = "ask" | "replace" | "skip" | "keepBoth";
export type ArchiveFormat =
  | "zip"
  | "sevenZip"
  | "tarGzip"
  | "tarXz"
  | "tarZstd"
  | "gzip"
  | "xz"
  | "zstd";
export type CompressionLevel = "store" | "fast" | "normal" | "maximum";
export type ExtractionDestination = "ask" | "sibling" | "custom";

export interface Settings {
  version: 1;
  defaultFormat: ArchiveFormat;
  defaultCompression: CompressionLevel;
  zipCompression: CompressionLevel;
  sevenZipCompression: CompressionLevel;
  extractionDestination: ExtractionDestination;
  customDestination: string | null;
  revealOnCompletion: boolean;
  notifications: boolean;
  showHiddenEntries: boolean;
  historyEnabled: boolean;
  maxExpandedBytes: number;
  maxPreviewBytes: number;
  maxConcurrentJobs: 1;
}

export interface TestResult {
  path: string;
  elapsedMs: number;
  warningCount: number;
}

export interface JobSnapshot {
  id: number;
  operation: "create" | "extract" | "test" | "modify";
  phase: "preparing" | "running" | "finishing" | "cancelling" | "done";
  percent: number;
  processedBytes: number;
  totalBytes: number;
  elapsedMs: number;
  bytesPerSecond: number;
  currentEntry: string | null;
  warningCount: number;
  cancellable: boolean;
}

export interface ArchiveError {
  code: string;
  message: string;
}

export type ShellAction =
  | "open"
  | "extract_here"
  | "extract_to_folder"
  | "test_archive"
  | "compress_zip"
  | "compress_options";

export interface ShellRequest {
  version: 1;
  action: ShellAction;
  paths: string[];
  createdAt: number;
  nonce: string;
}

export interface ShellIntegrationStatus {
  available: boolean;
  providerRegistered: boolean;
  documentExtensions: number;
  serviceActions: number;
}
