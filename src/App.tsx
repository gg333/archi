import { useEffect, useMemo, useRef, useState } from "react";
import { getVersion } from "@tauri-apps/api/app";
import { listen } from "@tauri-apps/api/event";
import { getCurrentWebview } from "@tauri-apps/api/webview";
import { getCurrentWindow } from "@tauri-apps/api/window";
import { confirm, open, save } from "@tauri-apps/plugin-dialog";
import { isPermissionGranted, requestPermission, sendNotification } from "@tauri-apps/plugin-notification";
import {
  addToArchive, archiveChanged, archiveError, archiveFolders, cancelJob, clearDiagnostics, clearRecentArchives,
  createArchive, defaultZipOutput, deleteArchiveEntries, entryIcons, entryPage, exportDiagnostics,
  extractArchive, getSettings, jobStatus, openArchive, openArchiveEntry, openDestination, recentArchives,
  recordDiagnostic, renameArchiveEntry, resetSettings, saveSettings, setArchiveComment,
  shellIntegrationStatus, takeShellRequests, testArchive,
} from "./api";
import type {
  ArchiveDocument, ArchiveEntry, ArchiveError, ArchiveFolder, ArchiveFormat, CompressionLevel,
  ConflictPolicy, EntryPage, JobSnapshot, Settings, ShellIntegrationStatus,
  ShellRequest, SortKey,
} from "./types";
import "./App.css";
import { Modal } from "./components/Modal";
import { ExtractDialog as ExtractPanel, JobShelf, PasswordDialog } from "./components/OperationPanels";
import { PopupMenu } from "./components/PopupMenu";
import { drainShellRequests } from "./shellRequests";
import appIcon from "../src-tauri/icons/128x128@2x.png";

const PAGE_SIZE = 200;
const archiveFilters = [
  "zip", "7z", "rar", "tar", "gz", "tgz", "bz2", "tbz2", "xz", "txz",
  "zst", "cab", "iso", "lzh", "lha", "ar", "cpio", "001",
];
const createFormats: { value: ArchiveFormat; label: string; extension: string }[] = [
  { value: "zip", label: "ZIP", extension: "zip" },
  { value: "sevenZip", label: "7z", extension: "7z" },
  { value: "tarGzip", label: "TAR.GZ", extension: "tar.gz" },
  { value: "tarXz", label: "TAR.XZ", extension: "tar.xz" },
  { value: "tarZstd", label: "TAR.ZST", extension: "tar.zst" },
  { value: "gzip", label: "GZIP stream", extension: "gz" },
  { value: "xz", label: "XZ stream", extension: "xz" },
  { value: "zstd", label: "Zstandard stream", extension: "zst" },
];
const defaultSettings: Settings = {
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
};

type Operation = "opening" | "extracting" | "creating" | "testing" | "modifying" | null;
type PasswordAction =
  | { kind: "open"; path: string }
  | { kind: "entry"; path: string; entry: string; quickLook: boolean }
  | { kind: "extract"; path: string; destination: string; entries: string[]; reveal: boolean; conflictPolicy?: ConflictPolicy }
  | { kind: "test"; path: string }
  | { kind: "add"; path: string; inputs: string[]; compression: CompressionLevel }
  | { kind: "delete"; path: string; entries: string[] }
  | { kind: "rename"; path: string; entry: string; newName: string }
  | { kind: "comment"; path: string; comment: string };
interface EntryMenu { entry: ArchiveEntry; x: number; y: number }
interface ExtractDialog { entries: string[]; destination: string }
interface RenameDialog { entry: ArchiveEntry; name: string }

function App() {
  const [archive, setArchive] = useState<ArchiveDocument | null>(null);
  const [entries, setEntries] = useState<EntryPage | null>(null);
  const [folder, setFolder] = useState("");
  const [folderHistory, setFolderHistory] = useState([""]);
  const [folderHistoryIndex, setFolderHistoryIndex] = useState(0);
  const [folderChildren, setFolderChildren] = useState<Record<string, ArchiveFolder[]>>({});
  const [sidebarVisible, setSidebarVisible] = useState(true);
  const [expandedFolders, setExpandedFolders] = useState<Set<string>>(new Set());
  const [addMenuOpen, setAddMenuOpen] = useState(false);
  const [extractMenuOpen, setExtractMenuOpen] = useState(false);
  const [moreOpen, setMoreOpen] = useState(false);
  const [query, setQuery] = useState("");
  const [sort, setSort] = useState<SortKey>("name");
  const [descending, setDescending] = useState(false);
  const [pageNumber, setPageNumber] = useState(1);
  const [selected, setSelected] = useState<Set<string>>(new Set());
  const [lastSelected, setLastSelected] = useState<string | null>(null);
  const [archiveNeedsPassword, setArchiveNeedsPassword] = useState(false);
  const [archiveOutdated, setArchiveOutdated] = useState(false);
  const [monitorChanges, setMonitorChanges] = useState(true);
  const [error, setError] = useState<ArchiveError | null>(null);
  const [status, setStatus] = useState("Choose an archive to inspect its contents.");
  const [operation, setOperation] = useState<Operation>(null);
  const [job, setJob] = useState<JobSnapshot | null>(null);
  const [conflictPolicy, setConflictPolicy] = useState<ConflictPolicy>("ask");
  const [passwordAction, setPasswordAction] = useState<PasswordAction | null>(null);
  const [password, setPassword] = useState("");
  const [passwordError, setPasswordError] = useState<string | null>(null);
  const [showPassword, setShowPassword] = useState(false);
  const [extractDialog, setExtractDialog] = useState<ExtractDialog | null>(null);
  const [conflictMessage, setConflictMessage] = useState<string | null>(null);
  const [revealExtraction, setRevealExtraction] = useState(true);
  const [createOpen, setCreateOpen] = useState(false);
  const [createInputs, setCreateInputs] = useState<string[]>([]);
  const [createOutput, setCreateOutput] = useState("");
  const [createFormat, setCreateFormat] = useState<ArchiveFormat>("zip");
  const [compression, setCompression] = useState<CompressionLevel>("normal");
  const [volumeSize, setVolumeSize] = useState<number | null>(null);
  const [encrypt, setEncrypt] = useState(false);
  const [createPassword, setCreatePassword] = useState("");
  const [createConfirmation, setCreateConfirmation] = useState("");
  const [showCreatePassword, setShowCreatePassword] = useState(false);
  const [entryMenu, setEntryMenu] = useState<EntryMenu | null>(null);
  const [propertiesEntry, setPropertiesEntry] = useState<ArchiveEntry | null>(null);
  const [settings, setSettings] = useState<Settings>(defaultSettings);
  const [settingsDraft, setSettingsDraft] = useState<Settings>(defaultSettings);
  const [settingsOpen, setSettingsOpen] = useState(false);
  const [recentOpen, setRecentOpen] = useState(false);
  const [integration, setIntegration] = useState<ShellIntegrationStatus | null>(null);
  const [dragActive, setDragActive] = useState(false);
  const [nativeIcons, setNativeIcons] = useState<Record<string, string>>({});
  const [recents, setRecents] = useState<string[]>([]);
  const [renameDialog, setRenameDialog] = useState<RenameDialog | null>(null);
  const [commentOpen, setCommentOpen] = useState(false);
  const [commentDraft, setCommentDraft] = useState("");
  const [aboutOpen, setAboutOpen] = useState(false);
  const [appVersion, setAppVersion] = useState<string | null>(null);
  const searchRef = useRef<HTMLInputElement>(null);
  const shellBusy = useRef(false);
  const requestedIcons = useRef(new Set<string>());
  const archivePathRef = useRef<string | null>(null);
  archivePathRef.current = archive?.path ?? null;
  const busy = operation !== null;
  const selectedEntries = useMemo(
    () => entries?.entries.filter((entry) => selected.has(entry.path)) ?? [],
    [entries, selected],
  );
  const selectedSize = selectedEntries.reduce((total, entry) => total + (entry.size ?? 0), 0);

  useEffect(() => {
    void getVersion().then(setAppVersion).catch(() => undefined);
    void getSettings().then((value) => {
      setSettings(value);
      setSettingsDraft(value);
      setCreateFormat(value.defaultFormat);
      setCompression(compressionFor(value.defaultFormat, value));
      setRevealExtraction(value.revealOnCompletion);
    }).catch((caught) => setError(archiveError(caught)));
    void refreshRecents();
  }, []);

  useEffect(() => {
    let disposed = false;
    let unlisten: (() => void) | undefined;
    void listen<string>("menu-action", ({ payload }) => {
      if (payload === "about") setAboutOpen(true);
      else if (!busy && payload === "new-archive") openCreateDialog();
      else if (!busy && payload === "open-archive") void chooseArchive();
      else if (!busy && payload === "open-recent") { void refreshRecents(); setRecentOpen(true); }
      else if (!busy && payload === "close-archive") closeArchive();
      else if (!busy && payload === "settings") openSettings();
    }).then((remove) => {
      if (disposed) remove(); else unlisten = remove;
    });
    return () => { disposed = true; unlisten?.(); };
  }, [busy, settings]);

  useEffect(() => {
    void getCurrentWindow().setTitle(archive ? archive.name : "Archi");
  }, [archive]);

  useEffect(() => {
    if (busy) return;
    let active = true;
    const poll = async () => {
      if (!active || shellBusy.current) return;
      shellBusy.current = true;
      try {
        await drainShellRequests(takeShellRequests, async (request) => {
          if (active) await handleShellRequest(request);
        });
      } catch (caught) {
        if (active) setError(archiveError(caught));
      } finally { shellBusy.current = false; }
    };
    void poll();
    const timer = window.setInterval(poll, 400);
    return () => { active = false; window.clearInterval(timer); };
  }, [busy, conflictPolicy, settings]);

  useEffect(() => {
    if (!archive) { setEntries(null); return; }
    let active = true;
    const timer = window.setTimeout(() => {
      void entryPage(
        archive.path, folder, query, sort, descending, pageNumber, PAGE_SIZE,
        settings.showHiddenEntries,
      ).then((value) => {
        if (!active) return;
        setEntries(value);
        if (value.page !== pageNumber) setPageNumber(value.page);
      }).catch((caught) => active && setError(archiveError(caught)));
    }, query ? 150 : 0);
    return () => { active = false; window.clearTimeout(timer); };
  }, [archive, folder, query, sort, descending, pageNumber, settings.showHiddenEntries]);

  useEffect(() => {
    if (!archive) {
      setFolderChildren({});
      setExpandedFolders(new Set());
      return;
    }
    let active = true;
    setFolderChildren({});
    setExpandedFolders(new Set());
    void archiveFolders(archive.path, "", settings.showHiddenEntries)
      .then((value) => {
        if (!active) return;
        setFolderChildren({ "": value });
      })
      .catch((caught) => active && setError(archiveError(caught)));
    return () => { active = false; };
  }, [archive, settings.showHiddenEntries]);

  useEffect(() => {
    if (!settings.showHiddenEntries && isHiddenArchivePath(folder)) navigateFolder("");
  }, [folder, settings.showHiddenEntries]);

  useEffect(() => {
    if (!entries) return;
    const keys = [...new Set(entries.entries.map(entryIconKey))]
      .filter((key) => key !== "__link__" && !requestedIcons.current.has(key));
    if (!keys.length) return;
    keys.forEach((key) => requestedIcons.current.add(key));
    void entryIcons(keys)
      .then((icons) => setNativeIcons((current) => ({ ...current, ...icons })))
      .catch(() => undefined);
  }, [entries]);

  useEffect(() => {
    if (!operation || operation === "opening") { setJob(null); return; }
    let active = true;
    const poll = async () => {
      try { const current = await jobStatus(); if (active) setJob(current); } catch { /* result owns error */ }
    };
    void poll();
    const timer = window.setInterval(poll, 250);
    return () => { active = false; window.clearInterval(timer); };
  }, [operation]);

  useEffect(() => {
    if (!archive || busy || archiveOutdated || !monitorChanges) return;
    let active = true;
    const check = async () => {
      try { if (active && await archiveChanged(archive.path)) setArchiveOutdated(true); }
      catch (caught) { if (active) { setArchiveOutdated(true); setError(archiveError(caught)); } }
    };
    const timer = window.setInterval(check, 3000);
    return () => { active = false; window.clearInterval(timer); };
  }, [archive, busy, archiveOutdated, monitorChanges]);

  useEffect(() => {
    if (error) void recordDiagnostic("error", error.code).catch(() => undefined);
  }, [error]);

  useEffect(() => {
    if (!entryMenu) return;
    const close = () => setEntryMenu(null);
    window.addEventListener("click", close);
    return () => window.removeEventListener("click", close);
  }, [entryMenu]);

  useEffect(() => {
    let disposed = false;
    let unlisten: (() => void) | undefined;
    void getCurrentWebview().onDragDropEvent((event) => {
      if (event.payload.type === "enter" || event.payload.type === "over") setDragActive(true);
      else if (event.payload.type === "leave") setDragActive(false);
      else if (event.payload.type === "drop") {
        setDragActive(false);
        const paths = event.payload.paths;
        if (!paths.length || busy) return;
        if (!createOpen && paths.length === 1 && isArchivePath(paths[0])) void loadArchive(paths[0]);
        else { addCreateInputs(paths); setCreateOpen(true); }
      }
    }).then((remove) => { if (disposed) remove(); else unlisten = remove; });
    return () => { disposed = true; unlisten?.(); };
  }, [busy, createOpen]);

  useEffect(() => {
    const shortcut = (event: KeyboardEvent) => {
      if (event.defaultPrevented) return;
      const modifier = event.metaKey || event.ctrlKey;
      if (event.key === "Escape") {
        if (entryMenu) setEntryMenu(null);
        else if (aboutOpen) setAboutOpen(false);
        else if (propertiesEntry) setPropertiesEntry(null);
        else if (renameDialog) setRenameDialog(null);
        else if (commentOpen) setCommentOpen(false);
        else if (extractDialog) setExtractDialog(null);
        else if (settingsOpen) setSettingsOpen(false);
        else if (recentOpen) setRecentOpen(false);
        else if (createOpen) setCreateOpen(false);
        else if (passwordAction) clearPasswordPrompt();
        else if (job?.cancellable) void requestCancellation();
        return;
      }
      const dialogOpen = aboutOpen || propertiesEntry || passwordAction || createOpen || settingsOpen || recentOpen || extractDialog || renameDialog || commentOpen;
      if (event.key === " " && !modifier && archive && !busy && !dialogOpen && selectedEntries.length === 1 && !selectedEntries[0].isDirectory && !isTypingTarget(event.target)) {
        event.preventDefault();
        void requestEntryOpen(selectedEntries[0], true);
        return;
      }
      if (!modifier || dialogOpen) return;
      const key = event.key.toLowerCase();
      if (key === "o") { event.preventDefault(); void chooseArchive(); }
      else if (key === "n") { event.preventDefault(); openCreateDialog(); }
      else if (key === "e" && archive) { event.preventDefault(); void requestExtraction(selected.size ? [...selected] : []); }
      else if (key === "t" && archive) { event.preventDefault(); void requestTest(); }
      else if (key === "f" && archive) { event.preventDefault(); searchRef.current?.focus(); }
      else if (key === "a" && event.shiftKey) { event.preventDefault(); openCreateDialog(); void addFiles(); }
      else if (key === "a" && entries) { event.preventDefault(); setSelected(new Set(entries.entries.map((entry) => entry.path))); }
      else if (key === "i" && selectedEntries.length === 1) { event.preventDefault(); setPropertiesEntry(selectedEntries[0]); }
      else if (key === "c" && selected.size) {
        event.preventDefault();
        void copyPaths([...selected]);
      }
    };
    window.addEventListener("keydown", shortcut);
    return () => window.removeEventListener("keydown", shortcut);
  }, [aboutOpen, archive, busy, commentOpen, createOpen, entries, entryMenu, extractDialog, job, passwordAction, propertiesEntry, recentOpen, renameDialog, selected, selectedEntries, settingsOpen]);

  async function chooseArchive() {
    const path = await open({ multiple: false, directory: false, title: "Open Archive", filters: [{ name: "Archives", extensions: archiveFilters }] });
    if (path) await loadArchive(path);
  }

  async function handleShellRequest(request: ShellRequest) {
    if (!request.paths.length) return;
    if (request.action === "open") {
      await loadArchive(request.paths[0]);
      return;
    }
    if (request.action === "compress_options") {
      setCreateInputs(request.paths);
      setCreateFormat(settings.defaultFormat);
      setCompression(settings.defaultCompression);
      setCreateOutput("");
      setCreateOpen(true);
      setStatus(`${request.paths.length} Finder ${request.paths.length === 1 ? "item" : "items"} ready to compress.`);
      return;
    }
    if (request.action === "compress_zip") {
      try {
        const output = await defaultZipOutput(request.paths);
        await runCreation(request.paths, output, "zip", settings.defaultCompression);
      } catch (caught) { setError(archiveError(caught)); setStatus("Finder compression failed."); }
      return;
    }
    if (request.action === "extract_here" || request.action === "extract_to_folder") {
      const destination = request.action === "extract_to_folder"
        ? await open({ multiple: false, directory: true, title: "Extract Finder Selection" })
        : null;
      if (request.action === "extract_to_folder" && !destination) return;
      await runShellExtractions(request.paths, destination);
      return;
    }
    if (request.action === "test_archive") await runShellTests(request.paths);
  }

  async function runShellExtractions(paths: string[], sharedDestination: string | null) {
    let firstDestination: string | null = null;
    for (const path of paths) {
      let document: ArchiveDocument;
      try {
        setOperation("opening"); setStatus(`Reading ${leafName(path)}…`);
        document = await openArchive(path);
        installDocument(document, false);
      } catch (caught) {
        const failure = archiveError(caught);
        const destination = sharedDestination ?? parentPath(path);
        if (isPasswordError(failure)) {
          setPasswordError(null);
          setPasswordAction({ kind: "extract", path, destination, entries: [], reveal: settings.revealOnCompletion, conflictPolicy: "keepBoth" });
          setStatus("Password required to extract the Finder selection.");
        } else { setError(failure); setStatus("Finder extraction failed."); }
        return;
      } finally { setOperation(null); }
      const destination = sharedDestination ?? parentPath(path);
      firstDestination ??= destination;
      if (!await runExtraction(path, destination, [], false, undefined, false, "keepBoth")) return;
    }
    if (settings.revealOnCompletion && firstDestination) {
      try { await openDestination(firstDestination); } catch (caught) { setError(archiveError(caught)); }
    }
  }

  async function runShellTests(paths: string[]) {
    for (const path of paths) {
      try {
        setOperation("opening"); setStatus(`Reading ${leafName(path)}…`);
        const document = await openArchive(path);
        installDocument(document, false);
      } catch (caught) {
        const failure = archiveError(caught);
        if (isPasswordError(failure)) {
          setPasswordError(null);
          setPasswordAction({ kind: "test", path });
          setStatus("Password required to test the Finder selection.");
        } else { setError(failure); setStatus("Finder integrity test failed."); }
        return;
      } finally { setOperation(null); }
      if (!await runTest(path)) return;
    }
  }

  async function loadArchive(path: string, suppliedPassword?: string): Promise<boolean> {
    setOperation("opening"); setError(null); setStatus("Reading archive…");
    try {
      const document = await openArchive(path, suppliedPassword);
      installDocument(document, suppliedPassword !== undefined);
      setStatus(`${document.entryCount.toLocaleString()} entries`);
      void refreshRecents();
      clearPasswordPrompt();
      void recordDiagnostic("open").catch(() => undefined);
      return true;
    } catch (caught) {
      const failure = archiveError(caught);
      if (isPasswordError(failure)) {
        setPasswordError(suppliedPassword === undefined ? null : failure.message);
        setPasswordAction({ kind: "open", path }); setStatus(suppliedPassword === undefined ? "Password required to open archive." : "The password was not accepted. Try again.");
      } else { setError(failure); setStatus("Could not open archive."); }
      return false;
    } finally { setOperation(null); }
  }

  function installDocument(document: ArchiveDocument, passwordProvided: boolean) {
    setArchive(document);
    setCommentDraft(document.comment ?? "");
    setArchiveNeedsPassword(passwordProvided || document.encrypted);
    resetBrowser(); setArchiveOutdated(false); setMonitorChanges(true);
  }

  function closeArchive() {
    setArchive(null); setEntries(null); setCommentDraft(""); setArchiveNeedsPassword(false);
    setArchiveOutdated(false); setMonitorChanges(false); setEntryMenu(null); setPropertiesEntry(null);
    setRenameDialog(null); setCommentOpen(false); setExtractDialog(null); clearPasswordPrompt();
    resetBrowser(); setStatus("Choose an archive to inspect its contents.");
  }

  function resetBrowser() {
    setFolder(""); setFolderHistory([""]); setFolderHistoryIndex(0); setQuery("");
    setPageNumber(1); setSelected(new Set()); setLastSelected(null);
  }

  function navigateFolder(next: string) {
    setFolder(next); setQuery(""); setPageNumber(1); setSelected(new Set());
    setFolderHistory((current) => {
      const history = [...current.slice(0, folderHistoryIndex + 1), next];
      setFolderHistoryIndex(history.length - 1);
      return history;
    });
  }

  async function toggleFolder(node: ArchiveFolder) {
    if (!node.hasChildren) return;
    if (expandedFolders.has(node.path)) {
      setExpandedFolders((current) => { const next = new Set(current); next.delete(node.path); return next; });
      return;
    }
    setExpandedFolders((current) => new Set(current).add(node.path));
    if (folderChildren[node.path] || !archive) return;
    const archivePath = archive.path;
    try {
      const children = await archiveFolders(archivePath, node.path, settings.showHiddenEntries);
      if (archivePathRef.current === archivePath) {
        setFolderChildren((current) => ({ ...current, [node.path]: children }));
      }
    } catch (caught) {
      setExpandedFolders((current) => { const next = new Set(current); next.delete(node.path); return next; });
      setError(archiveError(caught));
    }
  }

  function moveHistory(nextIndex: number) {
    const next = folderHistory[nextIndex];
    if (next === undefined) return;
    setFolderHistoryIndex(nextIndex); setFolder(next); setQuery(""); setPageNumber(1); setSelected(new Set());
  }

  function goUp() {
    if (folder) navigateFolder(folder.includes("/") ? folder.slice(0, folder.lastIndexOf("/")) : "");
  }

  async function requestExtraction(paths: string[] = []) {
    if (!archive) return;
    setConflictMessage(null);
    let destination: string | null = null;
    if (settings.extractionDestination === "custom") destination = settings.customDestination;
    else if (settings.extractionDestination === "sibling") destination = siblingExtractionPath(archive.path, archive.name);
    else destination = await chooseExtractFolder(paths);
    if (!destination) return;
    setRevealExtraction(settings.revealOnCompletion);
    setExtractDialog({ entries: paths, destination });
  }

  async function requestEntryOpen(entry: ArchiveEntry, quickLook: boolean, suppliedPassword?: string): Promise<boolean> {
    if (!archive || busy) return false;
    if (entry.isDirectory) {
      navigateFolder(entry.path);
      return true;
    }
    setOperation("opening"); setError(null);
    setStatus(quickLook ? `Preparing Quick Look for ${leafName(entry.path)}…` : `Opening ${leafName(entry.path)}…`);
    try {
      await openArchiveEntry(archive.path, entry.path, quickLook, suppliedPassword);
      setStatus(quickLook ? `Previewing ${leafName(entry.path)}.` : `Opened a temporary read-only copy of ${leafName(entry.path)}.`);
      clearPasswordPrompt(); void recordDiagnostic("open").catch(() => undefined);
      return true;
    } catch (caught) {
      const failure = archiveError(caught);
      if (isPasswordError(failure)) {
        setPasswordError(suppliedPassword === undefined ? null : failure.message);
        setPasswordAction({ kind: "entry", path: archive.path, entry: entry.path, quickLook });
        setStatus(suppliedPassword === undefined ? "Enter the archive password and try again." : "The password was not accepted. Try again.");
      } else {
        setError(failure);
        setStatus(quickLook ? "Quick Look failed." : "Could not open the archive entry.");
        clearPasswordPrompt();
      }
      return false;
    } finally { setOperation(null); }
  }

  async function chooseExtractFolder(paths: string[]) {
    if (!archive) return null;
    return open({ multiple: false, directory: true, title: paths.length === 1 ? `Extract ${leafName(paths[0])}` : `Extract ${archive.name}` });
  }

  async function changeExtractFolder() {
    if (!extractDialog) return;
    const destination = await chooseExtractFolder(extractDialog.entries);
    if (destination) setExtractDialog({ ...extractDialog, destination });
  }

  async function submitExtraction(event: React.FormEvent) {
    event.preventDefault();
    if (!archive || !extractDialog) return;
    const request = extractDialog;
    setConflictMessage(null);
    setExtractDialog(null);
    if (archiveNeedsPassword) {
      setPasswordError(null);
      setPasswordAction({ kind: "extract", path: archive.path, destination: request.destination, entries: request.entries, reveal: revealExtraction });
    } else await runExtraction(archive.path, request.destination, request.entries, revealExtraction);
  }

  async function runExtraction(archivePath: string, destination: string, paths: string[], reveal: boolean, suppliedPassword?: string, allowUnbounded = false, policy: ConflictPolicy = conflictPolicy): Promise<boolean> {
    setOperation("extracting"); setError(null); setStatus(paths.length ? "Extracting selected entries…" : "Extracting archive…");
    try {
      const result = await extractArchive(archivePath, destination, policy, paths.length ? paths : undefined, suppliedPassword, settings.maxExpandedBytes, allowUnbounded);
      const changes = [
        `${result.filesExtracted.toLocaleString()} ${result.filesExtracted === 1 ? "file" : "files"} extracted`,
        result.filesSkipped ? `${result.filesSkipped.toLocaleString()} skipped` : "",
        result.renamed ? `${result.renamed.toLocaleString()} renamed` : "",
        result.warningCount ? `${result.warningCount.toLocaleString()} warnings` : "",
      ].filter(Boolean);
      setStatus(`${changes.join(", ")} in ${formatDuration(result.elapsedMs)}`);
      clearPasswordPrompt(); void recordDiagnostic("extract").catch(() => undefined);
      void notifyCompletion("Extraction complete", changes.join(", "));
      if (reveal) { try { await openDestination(result.destination); } catch (caught) { setError(archiveError(caught)); } }
      return true;
    } catch (caught) {
      const failure = archiveError(caught);
      if (failure.code === "cancelled") { setStatus("Extraction cancelled. No destination files were changed."); clearPasswordPrompt(); }
      else if (!allowUnbounded && (failure.code === "expansion_limit_exceeded" || failure.code === "expansion_size_unknown")) {
        if (await confirm(`${failure.message}\n\nContinue this extraction anyway?`, { title: "Extraction size warning", kind: "warning" })) {
          return await runExtraction(archivePath, destination, paths, reveal, suppliedPassword, true, policy);
        }
        setStatus("Extraction cancelled.");
      }
      else if (isPasswordError(failure)) {
        setPasswordError(suppliedPassword === undefined ? null : failure.message);
        setPasswordAction({ kind: "extract", path: archivePath, destination, entries: paths, reveal, conflictPolicy: policy });
        setStatus(suppliedPassword === undefined ? "Enter the archive password and try again." : "The password was not accepted. Try again.");
      } else if (failure.code === "conflict" && policy === "ask") {
        setConflictMessage(failure.message); setConflictPolicy("keepBoth"); setRevealExtraction(reveal);
        setExtractDialog({ entries: paths, destination }); setStatus("Choose how to handle the existing files.");
      } else { setError(failure); setStatus("Extraction failed."); clearPasswordPrompt(); }
      return false;
    } finally { setOperation(null); setJob(null); }
  }

  async function runTest(archivePath: string, suppliedPassword?: string): Promise<boolean> {
    setOperation("testing"); setError(null); setStatus("Testing archive integrity…");
    try {
      const result = await testArchive(archivePath, suppliedPassword);
      setStatus(result.warningCount ? `Integrity test passed with ${result.warningCount} warnings in ${formatDuration(result.elapsedMs)}.` : `Integrity test passed in ${formatDuration(result.elapsedMs)}.`);
      clearPasswordPrompt(); void recordDiagnostic("test").catch(() => undefined); return true;
    } catch (caught) {
      const failure = archiveError(caught);
      if (failure.code === "cancelled") { setStatus("Integrity test cancelled."); clearPasswordPrompt(); }
      else if (isPasswordError(failure)) { setPasswordError(suppliedPassword === undefined ? null : failure.message); setPasswordAction({ kind: "test", path: archivePath }); setStatus(suppliedPassword === undefined ? "Enter the archive password and try again." : "The password was not accepted. Try again."); }
      else { setError(failure); setStatus("Integrity test failed."); clearPasswordPrompt(); }
      return false;
    } finally { setOperation(null); setJob(null); }
  }

  async function requestTest() {
    if (!archive) return;
    if (archiveNeedsPassword) { setPasswordError(null); setPasswordAction({ kind: "test", path: archive.path }); } else await runTest(archive.path);
  }

  async function chooseArchiveAdditions(directory: boolean) {
    if (!archive?.canModify) return;
    const paths = await open({
      multiple: true,
      directory,
      title: directory ? "Add Folders to Archive" : "Add Files to Archive",
    });
    if (!paths) return;
    const inputs = Array.isArray(paths) ? paths : [paths];
    const level = compressionFor(archive.path.toLowerCase().endsWith(".zip") ? "zip" : "sevenZip", settings);
    if (archiveNeedsPassword) {
      setPasswordError(null);
      setPasswordAction({ kind: "add", path: archive.path, inputs, compression: level });
    } else await runAdditions(archive.path, inputs, level);
  }

  async function runAdditions(path: string, inputs: string[], level: CompressionLevel, suppliedPassword?: string) {
    setOperation("modifying"); setError(null); setStatus("Adding items to a recoverable archive copy…");
    try {
      const document = await addToArchive(path, inputs, level, suppliedPassword);
      installDocument(document, archiveNeedsPassword || suppliedPassword !== undefined);
      setStatus(`Added ${inputs.length.toLocaleString()} ${inputs.length === 1 ? "item" : "items"}.`);
      clearPasswordPrompt(); void recordDiagnostic("modify").catch(() => undefined); return true;
    } catch (caught) {
      return handleModificationError(caught, suppliedPassword, { kind: "add", path, inputs, compression: level }, "Add failed.");
    } finally { setOperation(null); setJob(null); }
  }

  async function requestDelete(paths: string[]) {
    if (!archive?.canModify || !paths.length) return;
    if (!await confirm(`Delete ${paths.length} selected ${paths.length === 1 ? "entry" : "entries"} from ${archive.name}?`, { title: "Delete archive entries", kind: "warning" })) return;
    if (archiveNeedsPassword) {
      setPasswordError(null); setPasswordAction({ kind: "delete", path: archive.path, entries: paths });
    } else await runDelete(archive.path, paths);
  }

  async function runDelete(path: string, paths: string[], suppliedPassword?: string) {
    setOperation("modifying"); setError(null); setStatus("Deleting from a recoverable archive copy…");
    try {
      const document = await deleteArchiveEntries(path, paths, suppliedPassword);
      installDocument(document, archiveNeedsPassword || suppliedPassword !== undefined);
      setStatus(`Deleted ${paths.length.toLocaleString()} selected ${paths.length === 1 ? "entry" : "entries"}.`);
      clearPasswordPrompt(); void recordDiagnostic("modify").catch(() => undefined); return true;
    } catch (caught) {
      return handleModificationError(caught, suppliedPassword, { kind: "delete", path, entries: paths }, "Delete failed.");
    } finally { setOperation(null); setJob(null); }
  }

  function requestRename(entry: ArchiveEntry) {
    if (archive?.canModify) setRenameDialog({ entry, name: leafName(entry.path) });
  }

  async function submitRename(event: React.FormEvent) {
    event.preventDefault();
    if (!archive || !renameDialog) return;
    const request = { entry: renameDialog.entry.path, newName: renameDialog.name };
    setRenameDialog(null);
    if (archiveNeedsPassword) {
      setPasswordError(null); setPasswordAction({ kind: "rename", path: archive.path, ...request });
    } else await runRename(archive.path, request.entry, request.newName);
  }

  async function runRename(path: string, entry: string, newName: string, suppliedPassword?: string) {
    setOperation("modifying"); setError(null); setStatus("Renaming in a recoverable archive copy…");
    try {
      const document = await renameArchiveEntry(path, entry, newName, suppliedPassword);
      installDocument(document, archiveNeedsPassword || suppliedPassword !== undefined);
      setStatus(`Renamed ${leafName(entry)} to ${newName}.`);
      clearPasswordPrompt(); void recordDiagnostic("modify").catch(() => undefined); return true;
    } catch (caught) {
      return handleModificationError(caught, suppliedPassword, { kind: "rename", path, entry, newName }, "Rename failed.");
    } finally { setOperation(null); setJob(null); }
  }

  function openCommentEditor() {
    if (!archive?.canModify || !archive.path.toLowerCase().endsWith(".zip")) return;
    setCommentDraft(archive.comment ?? ""); setCommentOpen(true);
  }

  async function submitComment(event: React.FormEvent) {
    event.preventDefault();
    if (!archive) return;
    setCommentOpen(false);
    if (archiveNeedsPassword) {
      setPasswordError(null); setPasswordAction({ kind: "comment", path: archive.path, comment: commentDraft });
    } else await runComment(archive.path, commentDraft);
  }

  async function runComment(path: string, comment: string, suppliedPassword?: string) {
    setOperation("modifying"); setError(null); setStatus("Saving ZIP comment safely…");
    try {
      const document = await setArchiveComment(path, comment, suppliedPassword);
      installDocument(document, archiveNeedsPassword || suppliedPassword !== undefined);
      setStatus(comment ? "Archive comment saved." : "Archive comment removed.");
      clearPasswordPrompt(); void recordDiagnostic("modify").catch(() => undefined); return true;
    } catch (caught) {
      return handleModificationError(caught, suppliedPassword, { kind: "comment", path, comment }, "Comment update failed.");
    } finally { setOperation(null); setJob(null); }
  }

  function handleModificationError(caught: unknown, suppliedPassword: string | undefined, action: PasswordAction, fallback: string) {
    const failure = archiveError(caught);
    if (failure.code === "cancelled") { setStatus("Archive modification cancelled. The original archive was preserved."); clearPasswordPrompt(); }
    else if (isPasswordError(failure)) {
      setPasswordError(suppliedPassword === undefined ? null : failure.message);
      setPasswordAction(action); setStatus(suppliedPassword === undefined ? "Enter the archive password and try again." : "The password was not accepted. Try again.");
    } else { setError(failure); setStatus(fallback); clearPasswordPrompt(); }
    return false;
  }

  async function submitPassword(event: React.FormEvent) {
    event.preventDefault();
    if (!passwordAction || !password) return;
    if (passwordAction.kind === "open") await loadArchive(passwordAction.path, password);
    else if (passwordAction.kind === "entry") {
      const entry = entries?.entries.find((candidate) => candidate.path === passwordAction.entry);
      if (entry) await requestEntryOpen(entry, passwordAction.quickLook, password);
      else {
        setError({ code: "entry_not_found", message: "The archive entry is no longer visible." });
        clearPasswordPrompt();
      }
    }
    else if (passwordAction.kind === "extract") await runExtraction(passwordAction.path, passwordAction.destination, passwordAction.entries, passwordAction.reveal, password, false, passwordAction.conflictPolicy ?? conflictPolicy);
    else if (passwordAction.kind === "test") await runTest(passwordAction.path, password);
    else if (passwordAction.kind === "add") await runAdditions(passwordAction.path, passwordAction.inputs, passwordAction.compression, password);
    else if (passwordAction.kind === "delete") await runDelete(passwordAction.path, passwordAction.entries, password);
    else if (passwordAction.kind === "rename") await runRename(passwordAction.path, passwordAction.entry, passwordAction.newName, password);
    else await runComment(passwordAction.path, passwordAction.comment, password);
  }

  function openCreateDialog() {
    setCreateFormat(settings.defaultFormat); setCompression(compressionFor(settings.defaultFormat, settings)); setCreateOpen(true);
  }
  async function addFiles() { addCreateInputs(await open({ multiple: true, directory: false, title: "Add Files" })); }
  async function addFolder() { addCreateInputs(await open({ multiple: true, directory: true, title: "Add Folders" })); }
  function addCreateInputs(paths: string | string[] | null) {
    if (!paths) return;
    const values = Array.isArray(paths) ? paths : [paths];
    setCreateInputs((current) => [...new Set([...current, ...values])]);
  }
  async function chooseCreateOutput() {
    const option = createFormats.find(({ value }) => value === createFormat)!;
    const path = await save({ title: "Save Archive", defaultPath: `Archive.${option.extension}`, filters: [{ name: option.label, extensions: [option.extension.split(".").pop()!] }] });
    if (path) setCreateOutput(path);
  }

  async function submitCreate(event: React.FormEvent) {
    event.preventDefault();
    if (!createInputs.length || !createOutput) { setError({ code: "missing_create_fields", message: "Choose input items and an output archive." }); return; }
    if (isStreamFormat(createFormat) && createInputs.length !== 1) { setError({ code: "stream_requires_file", message: "GZIP, XZ, and Zstandard streams require exactly one regular file." }); return; }
    if (encrypt && (!createPassword || createPassword !== createConfirmation)) { setError({ code: "password_mismatch", message: "Enter matching non-empty passwords." }); return; }
    const passwordForJob = encrypt ? createPassword : undefined;
    const confirmationForJob = encrypt ? createConfirmation : undefined;
    setCreatePassword(""); setCreateConfirmation(""); setCreateOpen(false);
    const created = await runCreation(createInputs, createOutput, createFormat, compression, passwordForJob, confirmationForJob, volumeSize ?? undefined);
    if (!created) setCreateOpen(true);
  }

  async function runCreation(inputs: string[], output: string, format: ArchiveFormat, level: CompressionLevel, passwordForJob?: string, confirmationForJob?: string, splitSize?: number): Promise<boolean> {
    setOperation("creating"); setError(null); setStatus("Creating archive…");
    try {
      const document = await createArchive(inputs, output, format, level, passwordForJob, confirmationForJob, splitSize);
      setArchive(document); setArchiveNeedsPassword(passwordForJob !== undefined); resetBrowser(); setArchiveOutdated(false); setMonitorChanges(true);
      setStatus(`Created ${document.name} with ${document.entryCount.toLocaleString()} ${document.entryCount === 1 ? "entry" : "entries"}${document.volumeCount > 1 ? ` across ${document.volumeCount} volumes` : ""}.${document.skippedLinks ? ` Skipped ${document.skippedLinks.toLocaleString()} symbolic ${document.skippedLinks === 1 ? "link" : "links"}.` : ""}`);
      setCreateInputs([]); setCreateOutput(""); setVolumeSize(null); void refreshRecents(); void recordDiagnostic("create").catch(() => undefined);
      void notifyCompletion("Archive created", document.name);
      return true;
    } catch (caught) {
      const failure = archiveError(caught);
      if (failure.code === "cancelled") setStatus("Archive creation cancelled. No output archive was created.");
      else { setError(failure); setStatus("Archive creation failed."); }
      return false;
    } finally { setOperation(null); setJob(null); }
  }

  function openSettings() {
    setSettingsDraft({ ...settings }); setSettingsOpen(true); void refreshIntegration();
  }
  async function refreshIntegration() {
    try { setIntegration(await shellIntegrationStatus()); }
    catch (caught) { setError(archiveError(caught)); }
  }
  async function chooseCustomDestination() {
    const destination = await open({ multiple: false, directory: true, title: "Default Extraction Folder" });
    if (destination) setSettingsDraft({ ...settingsDraft, customDestination: destination });
  }
  async function submitSettings(event: React.FormEvent) {
    event.preventDefault();
    try {
      if (settingsDraft.notifications && !await notificationPermission()) {
        setError({ code: "notifications_denied", message: "Notifications are disabled in system settings." });
        return;
      }
      const saved = await saveSettings({ ...settingsDraft, defaultCompression: compressionFor(settingsDraft.defaultFormat, settingsDraft) });
      setSettings(saved); setSettingsDraft(saved); setCreateFormat(saved.defaultFormat); setCompression(compressionFor(saved.defaultFormat, saved)); setSettingsOpen(false); setStatus("Settings saved.");
      if (saved.historyEnabled) void refreshRecents(); else setRecents([]);
    } catch (caught) { setError(archiveError(caught)); }
  }
  async function restoreSettings() {
    try {
      const restored = await resetSettings();
      setSettings(restored); setSettingsDraft(restored); setCreateFormat(restored.defaultFormat); setCompression(compressionFor(restored.defaultFormat, restored)); setStatus("Settings reset to defaults.");
      void refreshRecents();
    } catch (caught) { setError(archiveError(caught)); }
  }
  async function exportLocalDiagnostics() {
    const destination = await save({ title: "Export Diagnostics", defaultPath: "Archi Diagnostics.log", filters: [{ name: "Log", extensions: ["log"] }] });
    if (!destination) return;
    try { await exportDiagnostics(destination); setStatus("Diagnostics exported."); } catch (caught) { setError(archiveError(caught)); }
  }
  async function clearLocalDiagnostics() {
    if (!await confirm("Clear local diagnostic logs? This cannot be undone.", { title: "Clear diagnostic logs", kind: "warning" })) return;
    try { await clearDiagnostics(); setStatus("Diagnostic logs cleared."); } catch (caught) { setError(archiveError(caught)); }
  }
  async function refreshRecents() {
    try { setRecents(await recentArchives()); } catch { /* archive opening remains available */ }
  }
  async function clearHistory() {
    if (!await confirm("Clear recent archive history?", { title: "Clear recent archives", kind: "warning" })) return;
    try { await clearRecentArchives(); setRecents([]); setStatus("Recent archive history cleared."); }
    catch (caught) { setError(archiveError(caught)); }
  }
  async function requestCancellation() {
    if (await confirm("Cancel the active archive operation?", { title: "Cancel operation", kind: "warning" })) await cancelOperation();
  }
  async function cancelOperation() { if (await cancelJob()) setStatus("Cancelling archive operation…"); }
  async function notifyCompletion(title: string, body: string) {
    if (settings.notifications && await notificationPermission()) sendNotification({ title, body });
  }
  async function copyPaths(paths: string[]) {
    try {
      await navigator.clipboard.writeText(paths.join("\n"));
      setStatus(`${paths.length} ${paths.length === 1 ? "path" : "paths"} copied.`);
    } catch (caught) {
      setError({ code: "clipboard_failed", message: caught instanceof Error ? caught.message : "Could not copy archive paths." });
      setStatus("Copy failed.");
    }
  }
  function clearPasswordPrompt() { setPasswordAction(null); setPassword(""); setPasswordError(null); setShowPassword(false); }
  function changeSort(next: SortKey) {
    if (sort === next) setDescending(!descending); else { setSort(next); setDescending(false); }
    setPageNumber(1);
  }

  function selectEntry(event: React.MouseEvent, entry: ArchiveEntry) {
    if (event.shiftKey && lastSelected && entries) {
      const start = entries.entries.findIndex((value) => value.path === lastSelected);
      const end = entries.entries.findIndex((value) => value.path === entry.path);
      if (start >= 0 && end >= 0) setSelected(new Set(entries.entries.slice(Math.min(start, end), Math.max(start, end) + 1).map((value) => value.path)));
    } else if (event.metaKey || event.ctrlKey) {
      setSelected((current) => { const next = new Set(current); if (next.has(entry.path)) next.delete(entry.path); else next.add(entry.path); return next; });
    } else setSelected(new Set([entry.path]));
    setLastSelected(entry.path);
  }

  function handleRowKey(event: React.KeyboardEvent<HTMLTableRowElement>, index: number, entry: ArchiveEntry) {
    if (event.key === "Enter") { event.preventDefault(); if (entry.isDirectory) navigateFolder(entry.path); else void requestEntryOpen(entry, false); }
    else if (event.key === "Backspace") { event.preventDefault(); goUp(); }
    else if (event.key === " ") {
      event.preventDefault(); event.stopPropagation(); setSelected(new Set([entry.path])); if (!entry.isDirectory) void requestEntryOpen(entry, true);
    } else if (event.key === "ArrowDown" || event.key === "ArrowUp") {
      event.preventDefault();
      const rows = [...document.querySelectorAll<HTMLTableRowElement>("tbody tr[data-entry]")];
      const next = event.key === "ArrowDown" ? index + 1 : index - 1;
      rows[next]?.focus();
      if (rows[next]?.dataset.entry) setSelected(new Set([rows[next].dataset.entry!]));
    }
  }

  const crumbs = folder ? folder.split("/") : [];
  return (
    <main className={`app-shell${dragActive ? " drag-active" : ""}`}>
      {!archive && <header className="welcome-toolbar" data-tauri-drag-region>
        <div className="welcome-title" data-tauri-drag-region><strong>Archi</strong></div>
      </header>}
      {dragActive && <div className="drop-overlay" role="status">Drop an archive to open it, or files and folders to create one</div>}
      {error && <section className="error-banner" role="alert"><div><strong>{error.code.replace(/_/g, " ")}</strong><p>{error.message}</p></div><button className="icon-button" onClick={() => setError(null)} aria-label="Dismiss error"><Icon name="close" /></button></section>}
      {archiveOutdated && archive && <section className="change-banner" role="alert"><div><strong>Archive changed on disk</strong><p>Reload to browse the current file. No archive contents have been edited by this app.</p></div><div className="header-actions"><button onClick={() => { setArchiveOutdated(false); setMonitorChanges(false); }}>Keep browsing</button><button className="primary-button" onClick={() => void loadArchive(archive.path)}>Reload</button></div></section>}
      {job && <JobShelf job={job} onCancel={() => void requestCancellation()} />}

      {archive ? <section className="archive-panel" aria-busy={busy}>
        <div className="mac-toolbar" data-tauri-drag-region>
          <div className="toolbar-nav">
            <button className="toolbar-icon-button" aria-label="Back" title="Back" onClick={() => moveHistory(folderHistoryIndex - 1)} disabled={folderHistoryIndex === 0}><Icon name="back" /></button>
            <button className="toolbar-icon-button" aria-label="Forward" title="Forward" onClick={() => moveHistory(folderHistoryIndex + 1)} disabled={folderHistoryIndex >= folderHistory.length - 1}><Icon name="forward" /></button>
            <button className="toolbar-icon-button" aria-label="Up one folder" title="Up one folder" onClick={goUp} disabled={!folder}><Icon name="up" /></button>
            <button className={`toolbar-icon-button${sidebarVisible ? " active" : ""}`} aria-label="Toggle sidebar" title="Toggle sidebar" onClick={() => setSidebarVisible((value) => !value)}><Icon name="sidebar" /></button>
          </div>
          <div className="toolbar-document" data-tauri-drag-region><strong>{archive.name}</strong>{archive.volumeCount > 1 && <span>{archive.volumeCount} volumes</span>}</div>
          <div className="toolbar-actions">
            <label className="toolbar-search">
              <Icon name="search" className="search-icon" />
              <span className="sr-only">Search entry names</span>
              <input ref={searchRef} type="search" value={query} onChange={(event) => { setQuery(event.target.value); setPageNumber(1); setSelected(new Set()); }} placeholder="Search" />
              {query && <button type="button" className="search-clear" aria-label="Clear search" onClick={() => { setQuery(""); setPageNumber(1); setSelected(new Set()); searchRef.current?.focus(); }}><Icon name="close" /></button>}
            </label>
            {archive.canModify && <PopupMenu label="Add to archive" open={addMenuOpen} onOpenChange={setAddMenuOpen} disabled={busy} triggerClassName="toolbar-label-button" trigger={<><Icon name="add" /><span>Add</span><Icon name="disclosureDown" /></>}>
              <button role="menuitem" onClick={() => { setAddMenuOpen(false); void chooseArchiveAdditions(false); }}>Add Files…</button>
              <button role="menuitem" onClick={() => { setAddMenuOpen(false); void chooseArchiveAdditions(true); }}>Add Folder…</button>
            </PopupMenu>}
            <button className="toolbar-label-button" onClick={requestTest} disabled={busy}><Icon name="test" /><span>Test</span></button>
            <PopupMenu label="Extract options" open={extractMenuOpen} onOpenChange={setExtractMenuOpen} disabled={busy} triggerClassName="primary-button toolbar-label-button" trigger={<><Icon name="extract" /><span>Extract</span><Icon name="disclosureDown" /></>}>
              {selected.size > 0 && <button role="menuitem" onClick={() => { setExtractMenuOpen(false); void requestExtraction([...selected]); }}>Extract Selected ({selected.size})…</button>}
              <button role="menuitem" onClick={() => { setExtractMenuOpen(false); void requestExtraction([]); }}>Extract All…</button>
            </PopupMenu>
            <PopupMenu label="More actions" open={moreOpen} onOpenChange={setMoreOpen} trigger={<Icon name="more" />}>
                {archive.canModify && selectedEntries.length === 1 && <button role="menuitem" onClick={() => { setMoreOpen(false); requestRename(selectedEntries[0]); }}>Rename…</button>}
                {archive.canModify && selected.size > 0 && <button className="danger-button" role="menuitem" onClick={() => { setMoreOpen(false); void requestDelete([...selected]); }}>Delete</button>}
                {archive.path.toLowerCase().endsWith(".zip") && <button role="menuitem" onClick={() => { setMoreOpen(false); openCommentEditor(); }}>Comment…</button>}
                <button role="menuitem" onClick={() => { setMoreOpen(false); openSettings(); }}>Settings…</button>
            </PopupMenu>
          </div>
        </div>
        <div className="browser-bar">
          <nav className="breadcrumbs" aria-label="Archive folder"><button onClick={() => navigateFolder("")} aria-current={!folder ? "page" : undefined}>{archive.name}</button>{crumbs.map((crumb, index) => { const path = crumbs.slice(0, index + 1).join("/"); return <span key={path}><span aria-hidden="true">/</span><button onClick={() => navigateFolder(path)} aria-current={path === folder ? "page" : undefined}>{crumb}</button></span>; })}</nav>
        </div>
        <div className={`browser-content${sidebarVisible ? " with-sidebar" : ""}`}>
          {sidebarVisible && <aside className="folder-sidebar" aria-label="Archive folders">
            <button className={`sidebar-root${folder === "" ? " active" : ""}`} onClick={() => navigateFolder("")}><Icon name="archive" /><span>{archive.name}</span></button>
            <FolderTree nodes={folderChildren[""] ?? []} childrenByFolder={folderChildren} activePath={folder} expanded={expandedFolders} onToggle={(node) => void toggleFolder(node)} onNavigate={navigateFolder} />
          </aside>}
          <div className="table-wrap"><table><thead><tr><SortableHeader label="Name" value="name" current={sort} descending={descending} onSort={changeSort} /><th>Type</th><SortableHeader label="Size" value="size" current={sort} descending={descending} onSort={changeSort} /><SortableHeader label="Compressed" value="packedSize" current={sort} descending={descending} onSort={changeSort} /><SortableHeader label="Ratio" value="ratio" current={sort} descending={descending} onSort={changeSort} /><SortableHeader label="Modified" value="modified" current={sort} descending={descending} onSort={changeSort} /><th>Encrypted</th></tr></thead>
          <tbody>{entries?.entries.map((entry, index) => <tr key={entry.path} data-entry={entry.path} tabIndex={index === 0 || selected.has(entry.path) ? 0 : -1} aria-selected={selected.has(entry.path)} aria-label={entryLabel(entry)} className={selected.has(entry.path) ? "selected" : undefined} onClick={(event) => selectEntry(event, entry)} onKeyDown={(event) => handleRowKey(event, index, entry)} onDoubleClick={() => entry.isDirectory ? navigateFolder(entry.path) : void requestEntryOpen(entry, false)} onContextMenu={(event) => { event.preventDefault(); setSelected(new Set([entry.path])); setEntryMenu(contextMenuPosition(event, entry)); }} title={entry.isDirectory ? "Double-click to open; right-click for actions" : "Double-click to open; press Spacebar for Quick Look"}>
            <td className="entry-name" title={entry.path}><EntryIcon entry={entry} source={nativeIcons[entryIconKey(entry)]} />{leafName(entry.path)}{query && parentEntryPath(entry.path) && <small>{parentEntryPath(entry.path)}</small>}</td><td>{entry.isLink ? "Link" : entry.isDirectory ? "Folder" : entry.method ?? "File"}</td><td>{entry.size === null ? "—" : formatBytes(entry.size)}</td><td>{entry.packedSize === null ? "—" : formatBytes(entry.packedSize)}</td><td>{formatRatio(entry)}</td><td>{entry.modified ?? "—"}</td><td>{entry.encrypted ? "Yes" : "No"}</td>
          </tr>)}{entries?.total === 0 && <tr><td colSpan={7} className="no-results">No entries match this view.</td></tr>}</tbody></table></div>
        </div>
        <footer className="status-bar"><span aria-live="polite">{status}</span><div className="page-controls"><span>{selected.size ? `${selected.size.toLocaleString()} selected · ${formatBytes(selectedSize)} · ` : ""}{entries ? `${entries.total.toLocaleString()} items · Page ${entries.page} of ${entries.totalPages}` : "Loading…"}</span><button aria-label="Previous page" onClick={() => setPageNumber((value) => Math.max(1, value - 1))} disabled={!entries || entries.page <= 1}><Icon name="back" /></button><button aria-label="Next page" onClick={() => setPageNumber((value) => value + 1)} disabled={!entries || entries.page >= entries.totalPages}><Icon name="forward" /></button></div></footer>
      </section> : <section className="empty-state">
        <div className="empty-hero">
          <img className="empty-app-icon" src={appIcon} alt="" aria-hidden="true" />
          <h2>Open an Archive</h2>
          <p>Browse, preview or safely extract files entirely on your Mac.</p>
          <div className="empty-actions">
            <button className="primary-button" onClick={chooseArchive} disabled={busy}><Icon name="open" /><span>Open Archive…</span></button>
            <button onClick={openCreateDialog} disabled={busy}><Icon name="add" /><span>New Archive…</span></button>
          </div>
          <p className="drop-hint">Drop an archive anywhere</p>
        </div>
        <span className="empty-status" role="status">{status}</span>
      </section>}

      {entryMenu && <div className="context-menu" role="menu" style={{ left: entryMenu.x, top: entryMenu.y }} onClick={(event) => event.stopPropagation()}>{entryMenu.entry.isDirectory ? <button role="menuitem" onClick={() => { const path = entryMenu.entry.path; setEntryMenu(null); navigateFolder(path); }}>Open Folder</button> : <><button role="menuitem" onClick={() => { const entry = entryMenu.entry; setEntryMenu(null); void requestEntryOpen(entry, false); }}>Open</button><button role="menuitem" onClick={() => { const entry = entryMenu.entry; setEntryMenu(null); void requestEntryOpen(entry, true); }}><Icon name="quickLook" />Quick Look</button></>}<button role="menuitem" onClick={() => { const path = entryMenu.entry.path; setEntryMenu(null); void requestExtraction([path]); }}><Icon name="extract" />Extract {entryMenu.entry.isDirectory ? "Folder" : "File"}…</button>{archive?.canModify && <><button role="menuitem" onClick={() => { const entry = entryMenu.entry; setEntryMenu(null); requestRename(entry); }}>Rename…</button><button className="danger-button" role="menuitem" onClick={() => { const path = entryMenu.entry.path; setEntryMenu(null); void requestDelete([path]); }}>Delete</button></>}<button role="menuitem" onClick={() => { const path = entryMenu.entry.path; setEntryMenu(null); void copyPaths([path]); }}>Copy Path</button><button role="menuitem" onClick={() => { setPropertiesEntry(entryMenu.entry); setEntryMenu(null); }}>Properties</button></div>}
      {aboutOpen && <Modal className="about-dialog" labelledBy="about-title" onClose={() => setAboutOpen(false)}><div className="about-heading"><img src={appIcon} alt="" /><div><h2 id="about-title">Archi</h2><p>Version {appVersion ?? "0.4.0"}</p></div></div><p>Fast, private archive management for macOS.</p><dl><dt>Archive engine</dt><dd>7-Zip 26.02</dd></dl><p className="about-legal">7-Zip is licensed separately under the GNU LGPL with the unRAR restriction. Full notices and corresponding source are included with Archi.</p><p>© 2026 Nitivar</p><div className="modal-actions"><button autoFocus onClick={() => setAboutOpen(false)}>Close</button></div></Modal>}
      {recentOpen && <Modal className="recent-dialog" labelledBy="recent-title" onClose={() => setRecentOpen(false)}><h2 id="recent-title">Open Recent</h2>{settings.historyEnabled && recents.length ? <div className="recent-list">{recents.map((path) => <button key={path} onClick={() => { setRecentOpen(false); void loadArchive(path); }} title={path}><Icon name="archive" /><span className="recent-copy"><strong>{leafName(path)}</strong><small>{parentPath(path)}</small></span></button>)}</div> : <p>{settings.historyEnabled ? "No recent archives." : "Recent archive history is disabled in Settings."}</p>}<div className="modal-actions"><button autoFocus onClick={() => setRecentOpen(false)}>Close</button></div></Modal>}
      {propertiesEntry && <PropertiesDialog entry={propertiesEntry} onClose={() => setPropertiesEntry(null)} />}
      {renameDialog && <Modal className="password-dialog" labelledBy="rename-title" onClose={() => setRenameDialog(null)}><form className="modal-form" onSubmit={submitRename}><h2 id="rename-title">Rename archive entry</h2><p>{renameDialog.entry.path}</p><label>New name<input autoFocus value={renameDialog.name} onChange={(event) => setRenameDialog({ ...renameDialog, name: event.target.value })} /></label><div className="modal-actions"><button type="button" onClick={() => setRenameDialog(null)}>Cancel</button><button className="primary-button" type="submit">Rename</button></div></form></Modal>}
      {commentOpen && <Modal className="comment-dialog" labelledBy="comment-title" onClose={() => setCommentOpen(false)}><form className="modal-form" onSubmit={submitComment}><h2 id="comment-title">ZIP archive comment</h2><label>Comment<textarea autoFocus rows={6} maxLength={65535} value={commentDraft} onChange={(event) => setCommentDraft(event.target.value)} /></label><p>{new TextEncoder().encode(commentDraft).length.toLocaleString()} of 65,535 UTF-8 bytes</p><div className="modal-actions"><button type="button" onClick={() => setCommentOpen(false)}>Cancel</button><button className="primary-button" type="submit">Save Comment</button></div></form></Modal>}
      {extractDialog && <ExtractPanel selectedCount={extractDialog.entries.length} destination={extractDialog.destination} error={conflictMessage} policy={conflictPolicy} reveal={revealExtraction} onChooseDestination={() => void changeExtractFolder()} onPolicyChange={setConflictPolicy} onRevealChange={setRevealExtraction} onClose={() => setExtractDialog(null)} onSubmit={submitExtraction} />}
      {passwordAction && <PasswordDialog archiveName={leafName(passwordAction.path)} error={passwordError} password={password} showPassword={showPassword} busy={busy} onPasswordChange={setPassword} onShowPasswordChange={setShowPassword} onClose={clearPasswordPrompt} onSubmit={submitPassword} />}
      {createOpen && <Modal className="create-dialog" labelledBy="create-title" onClose={() => setCreateOpen(false)}><form className="modal-form" onSubmit={submitCreate}><h2 id="create-title">New Archive</h2><div className="source-picker"><div><strong>Items</strong><span>{createInputs.length ? `${createInputs.length} selected` : "Drop items here or choose them"}</span></div><div className="header-actions"><button type="button" onClick={addFiles}>Add Files…</button><button type="button" onClick={addFolder}>Add Folder…</button></div></div>{createInputs.length > 0 && <ul className="source-list">{createInputs.map((path) => <li key={path} title={path}><span>{leafName(path)}</span><button type="button" aria-label={`Remove ${leafName(path)}`} onClick={() => setCreateInputs((current) => current.filter((value) => value !== path))}><Icon name="close" /></button></li>)}</ul>}<label>Output<div className="output-picker"><input readOnly value={createOutput} placeholder="Choose where to save the archive" /><button type="button" onClick={chooseCreateOutput}>Choose…</button></div></label><div className="form-grid"><label>Format<select value={createFormat} onChange={(event) => { const format = event.target.value as ArchiveFormat; setCreateFormat(format); setCompression(compressionFor(format, settings)); setCreateOutput(""); setEncrypt(false); setVolumeSize(null); }}>{createFormats.map((option) => <option key={option.value} value={option.value}>{option.label}</option>)}</select></label><label>Compression<select value={compression} onChange={(event) => setCompression(event.target.value as CompressionLevel)}><option value="store">Store</option><option value="fast">Fast</option><option value="normal">Normal</option><option value="maximum">Maximum</option></select></label>{supportsArchiveOptions(createFormat) && <label>Split into volumes<select value={volumeSize ?? ""} onChange={(event) => setVolumeSize(event.target.value ? Number(event.target.value) : null)}><option value="">Single archive</option><option value={10 * 1024 ** 2}>10 MiB</option><option value={100 * 1024 ** 2}>100 MiB</option><option value={1024 ** 3}>1 GiB</option><option value={4 * 1024 ** 3}>4 GiB</option></select></label>}</div>{isStreamFormat(createFormat) && <p className="compatibility-note">Compressed streams accept one regular file. Use a TAR format for folders or multiple items.</p>}{supportsArchiveOptions(createFormat) && <label className="checkbox-label"><input type="checkbox" checked={encrypt} onChange={(event) => setEncrypt(event.target.checked)} />Encrypt archive</label>}{encrypt && supportsArchiveOptions(createFormat) && <div className="form-grid"><label>Password<input type={showCreatePassword ? "text" : "password"} value={createPassword} onChange={(event) => setCreatePassword(event.target.value)} autoComplete="new-password" /></label><label>Confirm password<input type={showCreatePassword ? "text" : "password"} value={createConfirmation} onChange={(event) => setCreateConfirmation(event.target.value)} autoComplete="new-password" /></label><label className="checkbox-label"><input type="checkbox" checked={showCreatePassword} onChange={(event) => setShowCreatePassword(event.target.checked)} />Show passwords</label></div>}{encrypt && createFormat === "zip" && <p className="compatibility-note">ZIP encryption uses AES-256; some older unzip tools may not support it.</p>}<div className="modal-actions"><button type="button" onClick={() => setCreateOpen(false)}>Cancel</button><button className="primary-button" type="submit">Create {createFormats.find(({ value }) => value === createFormat)?.label}</button></div></form></Modal>}
      {settingsOpen && <Modal className="settings-dialog" labelledBy="settings-title" onClose={() => setSettingsOpen(false)}><form className="modal-form" onSubmit={submitSettings}><h2 id="settings-title">Settings</h2><fieldset><legend>Defaults</legend><div className="form-grid"><label>Archive format<select value={settingsDraft.defaultFormat} onChange={(event) => setSettingsDraft({ ...settingsDraft, defaultFormat: event.target.value as ArchiveFormat })}>{createFormats.map((option) => <option key={option.value} value={option.value}>{option.label}</option>)}</select></label><label>ZIP compression<select value={settingsDraft.zipCompression} onChange={(event) => setSettingsDraft({ ...settingsDraft, zipCompression: event.target.value as CompressionLevel })}><option value="store">Store</option><option value="fast">Fast</option><option value="normal">Normal</option><option value="maximum">Maximum</option></select></label><label>7z compression<select value={settingsDraft.sevenZipCompression} onChange={(event) => setSettingsDraft({ ...settingsDraft, sevenZipCompression: event.target.value as CompressionLevel })}><option value="store">Store</option><option value="fast">Fast</option><option value="normal">Normal</option><option value="maximum">Maximum</option></select></label></div><label>Extraction destination<select value={settingsDraft.extractionDestination} onChange={(event) => setSettingsDraft({ ...settingsDraft, extractionDestination: event.target.value as Settings["extractionDestination"] })}><option value="ask">Ask every time</option><option value="sibling">Folder beside archive</option><option value="custom">Custom folder</option></select></label>{settingsDraft.extractionDestination === "custom" && <div className="output-picker"><input readOnly value={settingsDraft.customDestination ?? ""} placeholder="Choose a folder" /><button type="button" onClick={chooseCustomDestination}>Choose…</button></div>}</fieldset><fieldset><legend>Completion and browsing</legend><label className="checkbox-label"><input type="checkbox" checked={settingsDraft.revealOnCompletion} onChange={(event) => setSettingsDraft({ ...settingsDraft, revealOnCompletion: event.target.checked })} />Open destination after extraction</label><label className="checkbox-label"><input type="checkbox" checked={settingsDraft.notifications} onChange={(event) => setSettingsDraft({ ...settingsDraft, notifications: event.target.checked })} />Completion notifications</label><label className="checkbox-label"><input type="checkbox" checked={settingsDraft.showHiddenEntries} onChange={(event) => setSettingsDraft({ ...settingsDraft, showHiddenEntries: event.target.checked })} />Show hidden archive entries</label><label>Maximum declared extraction size<select value={settingsDraft.maxExpandedBytes} onChange={(event) => setSettingsDraft({ ...settingsDraft, maxExpandedBytes: Number(event.target.value) })}><option value={1024 ** 3}>1 GiB</option><option value={10 * 1024 ** 3}>10 GiB</option><option value={100 * 1024 ** 3}>100 GiB</option><option value={1024 ** 4}>1 TiB</option></select></label><label>Maximum temporary preview size<select value={settingsDraft.maxPreviewBytes} onChange={(event) => setSettingsDraft({ ...settingsDraft, maxPreviewBytes: Number(event.target.value) })}><option value={10 * 1024 ** 2}>10 MiB</option><option value={100 * 1024 ** 2}>100 MiB</option><option value={500 * 1024 ** 2}>500 MiB</option><option value={1024 ** 3}>1 GiB</option></select></label></fieldset><fieldset><legend>Finder integration</legend><p>{integration?.available && integration.providerRegistered ? `${integration.documentExtensions} archive extensions and ${integration.serviceActions} Finder Services are installed.` : integration ? "Finder integration is unavailable in this build." : "Checking Finder integration…"}</p><p>Service visibility can be enabled or disabled in macOS System Settings → Keyboard → Keyboard Shortcuts → Services.</p><button type="button" onClick={refreshIntegration}>Refresh Status</button></fieldset><fieldset><legend>Privacy and diagnostics</legend><label className="checkbox-label"><input type="checkbox" checked={settingsDraft.historyEnabled} onChange={(event) => setSettingsDraft({ ...settingsDraft, historyEnabled: event.target.checked })} />Remember recent archives on this Mac</label><p>Recent archive paths stay in this user account and are deleted when history is disabled.</p><div className="header-actions"><button type="button" onClick={clearHistory} disabled={!recents.length}>Clear History…</button><button type="button" onClick={exportLocalDiagnostics}>Export Diagnostics…</button><button type="button" className="danger-button" onClick={clearLocalDiagnostics}>Clear Logs…</button></div><p>Local diagnostics contain app, OS, architecture, engine version, operation, and error codes—never passwords, file contents, or entry lists.</p></fieldset><div className="modal-actions"><button type="button" onClick={restoreSettings}>Reset Defaults</button><span className="modal-spacer" /><button type="button" onClick={() => setSettingsOpen(false)}>Cancel</button><button className="primary-button" type="submit">Save Settings</button></div></form></Modal>}
    </main>
  );
}


type IconName =
  | "back" | "forward" | "up" | "sidebar" | "search" | "close"
  | "add" | "test" | "extract" | "more" | "folder" | "archive"
  | "open" | "quickLook" | "disclosureRight" | "disclosureDown"
  | "sortUp" | "sortDown";

function Icon({ name, className = "" }: { name: IconName; className?: string }) {
  let body;
  switch (name) {
    case "back": body = <path d="M12.5 4.5 7 10l5.5 5.5" />; break;
    case "forward": body = <path d="m7.5 4.5 5.5 5.5-5.5 5.5" />; break;
    case "up": body = <><path d="M5 9.5 10 4.5l5 5" /><path d="M10 5v10.5" /></>; break;
    case "sidebar": body = <><rect x="2.75" y="3.5" width="14.5" height="13" rx="2" /><path d="M7.25 3.75v12.5" /></>; break;
    case "search": body = <><circle cx="8.5" cy="8.5" r="4.25" /><path d="m11.7 11.7 4.05 4.05" /></>; break;
    case "close": body = <><path d="m6 6 8 8" /><path d="m14 6-8 8" /></>; break;
    case "add": body = <><path d="M10 4v12" /><path d="M4 10h12" /></>; break;
    case "test": body = <path d="m4.5 10.3 3.5 3.5 7.5-8" />; break;
    case "extract": body = <><path d="M10 3.5v8.5" /><path d="m6.5 8.8 3.5 3.5 3.5-3.5" /><path d="M4 14.5v1.75h12V14.5" /></>; break;
    case "more": body = <><circle cx="5" cy="10" r="1.15" fill="currentColor" stroke="none" /><circle cx="10" cy="10" r="1.15" fill="currentColor" stroke="none" /><circle cx="15" cy="10" r="1.15" fill="currentColor" stroke="none" /></>; break;
    case "folder": body = <path d="M2.75 6h5.5l1.4 1.75h7.6v7.75H2.75z" />; break;
    case "archive": body = <><rect x="3" y="3.5" width="14" height="13" rx="2" /><path d="M3.5 7h13" /><path d="M8 10h4" /></>; break;
    case "open": body = <><path d="M2.75 6h5.5l1.4 1.75h7.6v7.75H2.75z" /><path d="m10.5 12 2-2 2 2" /></>; break;
    case "quickLook": body = <><path d="M2.5 10s2.6-4.5 7.5-4.5 7.5 4.5 7.5 4.5-2.6 4.5-7.5 4.5S2.5 10 2.5 10Z" /><circle cx="10" cy="10" r="2.2" /></>; break;
    case "disclosureRight": body = <path d="m8 5.5 4.5 4.5L8 14.5" />; break;
    case "disclosureDown": body = <path d="m5.5 8 4.5 4.5L14.5 8" />; break;
    case "sortUp": body = <path d="m6.5 12.5 3.5-4 3.5 4" />; break;
    case "sortDown": body = <path d="m6.5 8 3.5 4 3.5-4" />; break;
  }
  return <svg className={`ui-icon ${className}`.trim()} viewBox="0 0 20 20" aria-hidden="true" focusable="false">{body}</svg>;
}

function FolderTree({ nodes, childrenByFolder, activePath, expanded, onToggle, onNavigate, depth = 0 }: { nodes: ArchiveFolder[]; childrenByFolder: Record<string, ArchiveFolder[]>; activePath: string; expanded: Set<string>; onToggle: (node: ArchiveFolder) => void; onNavigate: (path: string) => void; depth?: number }) {
  return <>{nodes.map((node) => {
    const isExpanded = expanded.has(node.path);
    return <div key={node.path}>
      <div className={`sidebar-row${activePath === node.path ? " active" : ""}`} style={{ paddingLeft: `${10 + depth * 14}px` }}>
        <button className="disclosure" aria-label={`${isExpanded ? "Collapse" : "Expand"} ${node.name}`} aria-expanded={node.hasChildren ? isExpanded : undefined} onClick={() => onToggle(node)} disabled={!node.hasChildren}>{node.hasChildren ? <Icon name={isExpanded ? "disclosureDown" : "disclosureRight"} /> : null}</button>
        <button className="sidebar-folder" onClick={() => onNavigate(node.path)}><Icon name="folder" /><span>{node.name}</span></button>
      </div>
      {isExpanded && <FolderTree nodes={childrenByFolder[node.path] ?? []} childrenByFolder={childrenByFolder} activePath={activePath} expanded={expanded} onToggle={onToggle} onNavigate={onNavigate} depth={depth + 1} />}
    </div>;
  })}</>;
}

function SortableHeader({ label, value, current, descending, onSort }: { label: string; value: SortKey; current: SortKey; descending: boolean; onSort: (value: SortKey) => void }) {
  const active = current === value;
  return <th aria-sort={active ? (descending ? "descending" : "ascending") : "none"}><button className="sort-button" onClick={() => onSort(value)}>{label}{active && <Icon name={descending ? "sortDown" : "sortUp"} />}</button></th>;
}
function PropertiesDialog({ entry, onClose }: { entry: ArchiveEntry; onClose: () => void }) {
  return <Modal className="properties-dialog" labelledBy="properties-title" onClose={onClose}><h2 id="properties-title">Entry Properties</h2><dl><dt>Path</dt><dd>{entry.path}</dd><dt>Type</dt><dd>{entry.isLink ? "Link" : entry.isDirectory ? "Folder" : "File"}</dd><dt>Size</dt><dd>{entry.size === null ? "Unknown" : formatBytes(entry.size)}</dd><dt>Compressed</dt><dd>{entry.packedSize === null ? "Unknown" : formatBytes(entry.packedSize)}</dd><dt>Ratio</dt><dd>{formatRatio(entry)}</dd><dt>Modified</dt><dd>{entry.modified ?? "Unknown"}</dd><dt>Method</dt><dd>{entry.method ?? "Unknown"}</dd><dt>Encrypted</dt><dd>{entry.encrypted ? "Yes" : "No"}</dd></dl><div className="modal-actions"><button autoFocus onClick={onClose}>Close</button></div></Modal>;
}
function isPasswordError(error: ArchiveError) { return error.code === "password_required" || error.code === "wrong_password"; }
async function notificationPermission() {
  if (await isPermissionGranted()) return true;
  return await requestPermission() === "granted";
}
function compressionFor(format: ArchiveFormat, settings: Settings) {
  if (format === "zip") return settings.zipCompression;
  if (format === "sevenZip") return settings.sevenZipCompression;
  return settings.defaultCompression;
}
function supportsArchiveOptions(format: ArchiveFormat) { return format === "zip" || format === "sevenZip"; }
function isStreamFormat(format: ArchiveFormat) { return format === "gzip" || format === "xz" || format === "zstd"; }
function isArchivePath(path: string) {
  const lower = path.toLowerCase();
  const extension = lower.split(".").pop();
  return extension === "001" ? lower.endsWith(".zip.001") || lower.endsWith(".7z.001") : extension !== undefined && archiveFilters.includes(extension);
}
function entryIconKey(entry: ArchiveEntry) {
  if (entry.isDirectory) return "__folder__";
  if (entry.isLink) return "__link__";
  const extension = entry.path.toLowerCase().split(".").pop() ?? "";
  return /^[a-z0-9]{1,16}$/.test(extension) ? extension : "__file__";
}
function EntryIcon({ entry, source }: { entry: ArchiveEntry; source?: string }) {
  if (source) return <img className="entry-icon" src={source} alt="" aria-hidden="true" />;
  const path = entry.isDirectory
    ? "M2.5 5.5h6l1.5 2h7.5v8.5h-15z"
    : entry.isLink
      ? "M7.4 12.6l5.2-5.2m-7.2 7.2l-1 1a2.1 2.1 0 003 3l2.4-2.4a2.1 2.1 0 000-3m4.8-6.8l1-1a2.1 2.1 0 013 3l-2.4 2.4a2.1 2.1 0 01-3 0"
      : "M4 2.5h7l5 5V17.5H4zM11 2.5v5h5";
  return <svg className="entry-icon fallback-icon" viewBox="0 0 20 20" aria-hidden="true"><path d={path} /></svg>;
}
function leafName(path: string) { const parts = path.split(/[\\/]/).filter(Boolean); return parts[parts.length - 1] ?? path; }
function isTypingTarget(target: EventTarget | null) { return target instanceof HTMLInputElement || target instanceof HTMLTextAreaElement || target instanceof HTMLSelectElement || (target instanceof HTMLElement && target.isContentEditable); }
function parentPath(path: string) { const index = Math.max(path.lastIndexOf("/"), path.lastIndexOf("\\")); return index > 0 ? path.slice(0, index) : path; }
function parentEntryPath(path: string) { const index = path.lastIndexOf("/"); return index < 0 ? "" : path.slice(0, index); }
function isHiddenArchivePath(path: string) { return path.split("/").some((part) => part.startsWith(".") && part !== "."); }
function contextMenuPosition(event: React.MouseEvent, entry: ArchiveEntry): EntryMenu {
  const margin = 8;
  const width = 190;
  const height = entry.isDirectory ? 220 : 290;
  return {
    entry,
    x: Math.max(margin, Math.min(event.clientX, window.innerWidth - width - margin)),
    y: Math.max(margin, Math.min(event.clientY, window.innerHeight - height - margin)),
  };
}
function siblingExtractionPath(path: string, name: string) {
  const separator = path.includes("\\") ? "\\" : "/";
  const parent = path.slice(0, Math.max(0, path.lastIndexOf(separator)));
  const folder = name.replace(/\.(?:tar\.(?:gz|bz2|xz|zst)|tgz|tbz2?|txz|tzst)$/i, "").replace(/\.[^.]+$/, "");
  return `${parent}${separator}${folder}`;
}
function entryLabel(entry: ArchiveEntry) { return [leafName(entry.path), entry.isDirectory ? "folder" : "file", entry.size === null ? "unknown size" : formatBytes(entry.size), entry.encrypted ? "encrypted" : "not encrypted"].join(", "); }
function formatBytes(bytes: number) {
  if (bytes < 1024) return `${bytes.toLocaleString()} B`;
  const units = ["KB", "MB", "GB", "TB"];
  let value = bytes / 1024; let index = 0;
  while (value >= 1024 && index < units.length - 1) { value /= 1024; index += 1; }
  return `${value.toFixed(value >= 10 ? 1 : 2)} ${units[index]}`;
}
function formatRatio(entry: ArchiveEntry) { return !entry.size || entry.packedSize === null ? "—" : `${Math.round((1 - entry.packedSize / entry.size) * 100)}%`; }
function formatDuration(milliseconds: number) {
  if (milliseconds < 1000) return `${milliseconds} ms`;
  const seconds = Math.floor(milliseconds / 1000);
  return seconds < 60 ? `${seconds}s` : `${Math.floor(seconds / 60)}m ${seconds % 60}s`;
}
export default App;
