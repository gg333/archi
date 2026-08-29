import type { ArchiveDocument, ArchiveEntry, EntryPage, JobSnapshot, Settings } from "./types";

export const archiveEntryContract = {
  path: "folder/file.txt",
  isDirectory: false,
  size: 1,
  packedSize: 1,
  modified: null,
  encrypted: false,
  method: null,
  isLink: false,
  linkTarget: null,
} satisfies ArchiveEntry;

export const archiveDocumentContract = {
  path: "/tmp/archive.zip",
  name: "archive.zip",
  engineVersion: "7-Zip test",
  entryCount: 1,
  totalBytes: 1,
  encrypted: false,
  skippedLinks: 0,
  comment: null,
  canModify: true,
  volumeCount: 1,
} satisfies ArchiveDocument;

export const entryPageContract = {
  folder: "",
  entries: [archiveEntryContract],
  fileTypes: ["txt"],
  page: 1,
  pageSize: 200,
  total: 1,
  totalPages: 1,
} satisfies EntryPage;

export const jobContract = {
  id: 1,
  operation: "extract",
  phase: "running",
  percent: 50,
  processedBytes: 1,
  totalBytes: 2,
  elapsedMs: 1,
  bytesPerSecond: 1,
  currentEntry: null,
  warningCount: 0,
  cancellable: true,
} satisfies JobSnapshot;

export const settingsContract = {
  version: 1,
  defaultFormat: "zip",
  defaultCompression: "normal",
  zipCompression: "normal",
  sevenZipCompression: "normal",
  extractionDestination: "ask",
  customDestination: null,
  revealOnCompletion: true,
  notifications: false,
  showHiddenEntries: false,
  historyEnabled: true,
  maxExpandedBytes: 10 * 1024 ** 3,
  maxPreviewBytes: 100 * 1024 ** 2,
  maxConcurrentJobs: 1,
} satisfies Settings;
