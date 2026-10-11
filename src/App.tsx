import { useEffect, useMemo, useRef, useState } from "react";
import { getVersion } from "@tauri-apps/api/app";
import { listen } from "@tauri-apps/api/event";
import { getCurrentWebview } from "@tauri-apps/api/webview";
import { getCurrentWindow } from "@tauri-apps/api/window";
import { startDrag } from "@crabnebula/tauri-plugin-drag";
import { confirm, message, open, save } from "@tauri-apps/plugin-dialog";
import { isPermissionGranted, requestPermission, sendNotification } from "@tauri-apps/plugin-notification";
import {
  addToArchive, archiveChanged, archiveError, archiveFolders, cancelJob, clearDiagnostics, clearRecentArchives,
  createArchive, defaultZipOutput, deleteArchiveEntries, entryIcons, entryPage, entryPaths, exportDiagnostics,
  extractArchive, getSettings, jobStatus, openArchive, openArchiveEntry, openDestination, prepareArchiveEntry, recentArchives,
  recordDiagnostic, renameArchiveEntry, resetSettings, saveSettings, setArchiveComment, setMenuEnabled, setRecentMenu,
  shellIntegrationStatus, takeShellRejections, takeShellRequests, testArchive,
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
import { drainShellRequests, fileDropAction } from "./shellRequests";
import appIcon from "../src-tauri/icons/128x128@2x.png";

const PAGE_SIZE = 200;
// Most archives open at once; only a slow open is worth a progress shelf.
const OPENING_SHELF_DELAY_MS = 700;
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
  | { kind: "drag"; path: string; entry: string }
  | { kind: "extract"; path: string; destination: string; entries: string[]; reveal: boolean; conflictPolicy?: ConflictPolicy; enclose?: boolean }
  | { kind: "test"; path: string }
  | { kind: "add"; path: string; inputs: string[]; compression: CompressionLevel; folder: string; replaceExisting: boolean }
  | { kind: "delete"; path: string; entries: string[] }
  | { kind: "rename"; path: string; entry: string; newName: string }
  | { kind: "comment"; path: string; comment: string };
type RunOutcome = "done" | "password" | "cancelled" | "skipped" | "conflict" | "failed";
interface ShellBatch {
  kind: "extract" | "test";
  remaining: string[];
  destination: string | null;
  pending: string | null;
  done: number;
  skipped: string[];
  failed: string[];
  firstDestination: string | null;
}
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
  const [fileType, setFileType] = useState("");
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
  const [createError, setCreateError] = useState<string | null>(null);
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
  const [settingsError, setSettingsError] = useState<string | null>(null);
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
  const tableRef = useRef<HTMLDivElement>(null);
  const loadedView = useRef({ key: "", pages: 0 });
  const loadingMore = useRef(false);
  const shellBusy = useRef(false);
  const shellBatch = useRef<ShellBatch | null>(null);
  // The open archive's password, kept in memory only until the archive is closed or replaced.
  const sessionPassword = useRef<{ path: string; password: string } | null>(null);
  const requestedIcons = useRef(new Set<string>());
  const archivePathRef = useRef<string | null>(null);
  const commandRef = useRef<(name: string) => void>(() => undefined);
  const openRecentRef = useRef<(path: string) => void>(() => undefined);
  const lastCommand = useRef({ name: "", at: 0 });
  const menuReturnFocus = useRef<HTMLElement | null>(null);
  const preparedDrag = useRef<{ archive: string; entry: string; path: string } | null>(null);
  const draggingOut = useRef(false);
  const busy = operation !== null;
  const selectedEntries = useMemo(
    () => entries?.entries.filter((entry) => selected.has(entry.path)) ?? [],
    [entries, selected],
  );
  const selectedSize = selectedEntries.reduce((total, entry) => total + (entry.size ?? 0), 0);

  useEffect(() => {
    archivePathRef.current = archive?.path ?? null;
  }, [archive?.path]);

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

  // Menu items and keyboard shortcuts both arrive here. The listeners are registered
  // once, so they call through refs to reach the current state.
  useEffect(() => {
    let disposed = false;
    const removers: (() => void)[] = [];
    const keep = (remove: () => void) => { if (disposed) remove(); else removers.push(remove); };
    void listen<string>("menu-action", ({ payload }) => commandRef.current(payload)).then(keep);
    void listen<string>("menu-open-recent", ({ payload }) => openRecentRef.current(payload)).then(keep);
    return () => { disposed = true; removers.forEach((remove) => remove()); };
  }, []);

  useEffect(() => {
    void getCurrentWindow().setTitle(archive ? archive.name : "Archi");
  }, [archive]);

  useEffect(() => {
    if (busy || passwordAction) return;
    let active = true;
    const poll = async () => {
      if (!active || shellBusy.current) return;
      shellBusy.current = true;
      try {
        await drainShellRequests(takeShellRequests, async (request) => {
          if (active) await handleShellRequest(request);
        });
        const refused = await takeShellRejections();
        if (active && refused.length) setError({ code: "finder_request_refused", message: refused.join(" ") });
      } catch (caught) {
        if (active) setError(archiveError(caught));
      } finally { shellBusy.current = false; }
    };
    void poll();
    const timer = window.setInterval(poll, 400);
    return () => { active = false; window.clearInterval(timer); };
  }, [busy, conflictPolicy, passwordAction, settings]);

  useEffect(() => {
    if (!archive) { setEntries(null); return; }
    const viewKey = JSON.stringify([archive.path, folder, query, fileType, sort, descending, settings.showHiddenEntries]);
    // A later page only makes sense on top of the earlier pages of the same view.
    if (pageNumber > 1 && (loadedView.current.key !== viewKey || loadedView.current.pages !== pageNumber - 1)) {
      setPageNumber(1);
      return;
    }
    let active = true;
    const timer = window.setTimeout(() => {
      void entryPage(
        archive.path, folder, query, fileType, sort, descending, pageNumber, PAGE_SIZE,
        settings.showHiddenEntries,
      ).then((value) => {
        if (!active) return;
        if (pageNumber === 1) {
          setEntries(value);
          tableRef.current?.scrollTo({ top: 0 });
        } else {
          // Further pages are appended as the list is scrolled.
          setEntries((current) => current ? { ...value, entries: [...current.entries, ...value.entries] } : value);
        }
        loadedView.current = { key: viewKey, pages: value.page };
      }).catch((caught) => active && setError(archiveError(caught))).finally(() => { if (active) loadingMore.current = false; });
    }, query ? 150 : 0);
    return () => { active = false; window.clearTimeout(timer); };
  }, [archive, folder, query, fileType, sort, descending, pageNumber, settings.showHiddenEntries]);

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

  // Keep the sidebar open down to the folder being viewed, however it was reached.
  useEffect(() => {
    if (!archive || !folder || !sidebarVisible) return;
    let active = true;
    const parts = folder.split("/");
    const ancestors = parts.slice(0, -1).map((_, index) => parts.slice(0, index + 1).join("/"));
    void (async () => {
      for (const ancestor of ancestors) {
        if (folderChildren[ancestor]) continue;
        const children = await archiveFolders(archive.path, ancestor, settings.showHiddenEntries);
        if (!active) return;
        setFolderChildren((current) => ({ ...current, [ancestor]: children }));
      }
      if (active && ancestors.length) setExpandedFolders((current) => new Set([...current, ...ancestors]));
    })().catch(() => undefined);
    return () => { active = false; };
  }, [archive, folder, sidebarVisible, settings.showHiddenEntries]);

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
    if (!operation) { setJob(null); return; }
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
    // Opened from the keyboard, the menu takes focus and hands it back when it closes.
    const returnTo = menuReturnFocus.current;
    if (returnTo) document.querySelector<HTMLElement>(".context-menu [role='menuitem']")?.focus();
    return () => {
      window.removeEventListener("click", close);
      if (returnTo) { menuReturnFocus.current = null; if (returnTo.isConnected) returnTo.focus(); }
    };
  }, [entryMenu]);

  useEffect(() => {
    let disposed = false;
    let unlisten: (() => void) | undefined;
    void getCurrentWebview().onDragDropEvent((event) => {
      if (draggingOut.current) { setDragActive(false); return; }
      if (event.payload.type === "enter" || event.payload.type === "over") setDragActive(true);
      else if (event.payload.type === "leave") setDragActive(false);
      else if (event.payload.type === "drop") {
        setDragActive(false);
        const paths = event.payload.paths;
        if (busy) return;
        const action = fileDropAction(paths, createOpen, archive?.canModify === true, isArchivePath);
        if (action === "add") void requestArchiveAdditions(paths);
        else if (action === "open") void loadArchive(paths[0]);
        else if (action === "extract") void extractDroppedArchives(paths);
        else if (action === "ask") void askAboutDroppedArchives(paths);
        else if (action === "create") { addCreateInputs(paths); setCreateOpen(true); }
      }
    }).then((remove) => { if (disposed) remove(); else unlisten = remove; });
    return () => { disposed = true; unlisten?.(); };
  }, [archive, archiveNeedsPassword, busy, createOpen, settings]);

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
        else if (createOpen) setCreateOpen(false);
        else if (passwordAction) cancelPasswordPrompt();
        else if (job?.cancellable) void requestCancellation();
        return;
      }
      const dialogOpen = aboutOpen || propertiesEntry || passwordAction || createOpen || settingsOpen || extractDialog || renameDialog || commentOpen;
      if (event.key === " " && !modifier && archive && !busy && !dialogOpen && selected.size === 1 && selectedEntries.length === 1 && !selectedEntries[0].isDirectory && !isTypingTarget(event.target)) {
        event.preventDefault();
        void requestEntryOpen(selectedEntries[0], true);
        return;
      }
      if (!modifier || dialogOpen) return;
      const key = event.key.toLowerCase();
      if (key === "a" && event.shiftKey && busy) { event.preventDefault(); blockedByJob(); return; }
      // These also have menu items; runCommand ignores the second of two arrivals.
      if (key === "o") { event.preventDefault(); commandRef.current("open-archive"); }
      else if (key === "n") { event.preventDefault(); commandRef.current("new-archive"); }
      else if (key === "e" && archive) { event.preventDefault(); commandRef.current("extract"); }
      else if (key === "t" && archive) { event.preventDefault(); commandRef.current("test"); }
      else if (key === "," ) { event.preventDefault(); commandRef.current("settings"); }
      else if (key === "w" && event.shiftKey && archive) { event.preventDefault(); commandRef.current("close-archive"); }
      else if (key === "s" && event.ctrlKey && event.metaKey && archive) { event.preventDefault(); commandRef.current("toggle-sidebar"); }
      else if (key === "f" && archive) { event.preventDefault(); searchRef.current?.focus(); }
      else if (key === "a" && event.shiftKey) { event.preventDefault(); openCreateDialog(); void addFiles(); }
      // Text fields keep the system Select All and Copy.
      else if ((key === "a" || key === "c") && isTypingTarget(event.target)) return;
      else if (key === "a" && entries) { event.preventDefault(); void selectAllInView(); }
      else if (key === "i" && selected.size === 1 && selectedEntries.length === 1) { event.preventDefault(); commandRef.current("properties"); }
      else if (key === "c" && selected.size) {
        event.preventDefault();
        void copyPaths([...selected]);
      }
    };
    window.addEventListener("keydown", shortcut);
    return () => window.removeEventListener("keydown", shortcut);
  }, [aboutOpen, archive, busy, commentOpen, createOpen, entries, entryMenu, extractDialog, fileType, folder, job, passwordAction, propertiesEntry, query, renameDialog, selected, selectedEntries, settings.showHiddenEntries, settingsOpen]);

  const anyDialogOpen = Boolean(aboutOpen || propertiesEntry || passwordAction || createOpen || settingsOpen || extractDialog || renameDialog || commentOpen);
  const onlySelected = selected.size === 1 && selectedEntries.length === 1 ? selectedEntries[0] : null;
  const zipArchive = archive?.canModify === true && archive.path.toLowerCase().endsWith(".zip");

  // One entry point for menu items and their keyboard shortcuts. A shortcut can reach the
  // window both as a key press and as its menu item, so a repeat within a moment is dropped.
  function runCommand(name: string, deletePaths?: string[]) {
    const now = performance.now();
    if (lastCommand.current.name === name && now - lastCommand.current.at < 400) return;
    lastCommand.current = { name, at: now };
    if (name === "about") { if (!anyDialogOpen) setAboutOpen(true); return; }
    if (anyDialogOpen) return;
    if (name === "toggle-sidebar") { if (archive) setSidebarVisible((value) => !value); return; }
    if (name === "properties") { if (onlySelected) setPropertiesEntry(onlySelected); return; }
    if (blockedByJob()) return;
    if (name === "new-archive") openCreateDialog();
    else if (name === "open-archive") void chooseArchive();
    else if (name === "close-archive") { if (archive) closeArchive(); }
    else if (name === "settings") openSettings();
    else if (name === "clear-recents") void clearHistory();
    else if (name === "extract") { if (archive) void requestExtraction(selected.size ? [...selected] : []); }
    else if (name === "test") void requestTest();
    else if (name === "add-files") void chooseArchiveAdditions(false);
    else if (name === "add-folder") void chooseArchiveAdditions(true);
    else if (name === "rename") { if (onlySelected) requestRename(onlySelected); }
    else if (name === "delete") { const paths = deletePaths ?? [...selected]; if (paths.length) void requestDelete(paths); }
    else if (name === "comment") openCommentEditor();
  }
  commandRef.current = runCommand;
  openRecentRef.current = (path) => { if (!anyDialogOpen) void loadArchive(path); };

  // Tell the menu bar which items apply right now, and what Open Recent should list.
  useEffect(() => {
    const idle = !busy && !anyDialogOpen;
    const editable = idle && archive?.canModify === true;
    void setMenuEnabled({
      "about": !anyDialogOpen,
      "settings": idle,
      "new-archive": idle,
      "open-archive": idle,
      "open-recent": idle,
      "close-archive": idle && archive !== null,
      "extract": idle && archive !== null,
      "test": idle && archive !== null,
      "add-files": editable,
      "add-folder": editable,
      "rename": editable && onlySelected !== null,
      "delete": editable && selected.size > 0,
      "comment": idle && zipArchive,
      "properties": !anyDialogOpen && onlySelected !== null,
      "toggle-sidebar": !anyDialogOpen && archive !== null,
    }).catch(() => undefined);
  }, [anyDialogOpen, archive, busy, onlySelected, selected.size, zipArchive]);

  useEffect(() => {
    void setRecentMenu(settings.historyEnabled ? recents : []).catch(() => undefined);
  }, [recents, settings.historyEnabled]);

  function knownPassword(path: string) {
    return sessionPassword.current?.path === path ? sessionPassword.current.password : undefined;
  }
  function rememberPassword(path: string, value: string | undefined) {
    if (value !== undefined && archivePathRef.current === path) sessionPassword.current = { path, password: value };
  }
  function forgetPassword(path: string) {
    if (sessionPassword.current?.path === path) sessionPassword.current = null;
  }

  // One archive job runs at a time; starting another would fail and drop the active job's progress.
  function blockedByJob() {
    if (!busy) return false;
    setStatus("Wait for the current operation to finish, or cancel it first.");
    return true;
  }

  async function chooseArchive() {
    if (blockedByJob()) return;
    const path = await open({ multiple: false, directory: false, title: "Open Archive", filters: [{ name: "Archives", extensions: archiveFilters }] });
    if (path) await loadArchive(path);
  }

  async function handleShellRequest(request: ShellRequest) {
    if (!request.paths.length) return;
    if (request.action === "open") {
      const others = request.paths.length - 1;
      if (await loadArchive(request.paths[0]) && others > 0) {
        setStatus(`Opened ${leafName(request.paths[0])}. ${others} other ${others === 1 ? "archive was" : "archives were"} not opened: Archi shows one archive at a time.`);
      }
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
        await runCreation(request.paths, output, "zip", settings.zipCompression, undefined, undefined, undefined, false, undefined, archivePathRef.current === null);
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

  // Extraction writes to disk, so a drop of several archives is confirmed first.
  async function extractDroppedArchives(paths: string[]) {
    if (await confirm(`Extract ${paths.length} archives next to the originals?`, { title: "Extract archives", okLabel: "Extract", cancelLabel: "Cancel" })) {
      await runShellExtractions(paths, null);
    }
  }

  async function askAboutDroppedArchives(paths: string[]) {
    if (!archive) return;
    const add = `Add to ${archive.name}`;
    const other = paths.length === 1 ? "Open" : `Extract ${paths.length} Archives`;
    const choice = await message(
      paths.length === 1 ? `${leafName(paths[0])} is an archive. Add it to ${archive.name}, or open it instead?` : `Add these ${paths.length} archives to ${archive.name}, or extract them next to the originals?`,
      { title: "Dropped archives", buttons: { yes: add, no: other, cancel: "Cancel" } },
    );
    if (choice === add) await requestArchiveAdditions(paths);
    else if (choice === other && paths.length === 1) await loadArchive(paths[0]);
    else if (choice === other) await runShellExtractions(paths, null);
  }

  async function runShellExtractions(paths: string[], sharedDestination: string | null) {
    shellBatch.current = { kind: "extract", remaining: [...paths], destination: sharedDestination, pending: null, done: 0, skipped: [], failed: [], firstDestination: null };
    await continueShellBatch();
  }

  async function runShellTests(paths: string[]) {
    shellBatch.current = { kind: "test", remaining: [...paths], destination: null, pending: null, done: 0, skipped: [], failed: [], firstDestination: null };
    await continueShellBatch();
  }

  // Works through a Finder selection one archive at a time. An archive that needs a password
  // parks the batch; it resumes with `resolved` once the prompt is answered or dismissed.
  async function continueShellBatch(resolved?: RunOutcome | "dismissed") {
    const batch = shellBatch.current;
    if (!batch) return;
    let stopped = false;
    if (batch.pending) {
      if (!resolved || resolved === "password") return;
      recordBatchOutcome(batch, batch.pending, resolved);
      batch.pending = null;
      stopped = resolved === "cancelled";
    }
    while (!stopped && batch.remaining.length) {
      const path = batch.remaining.shift()!;
      const outcome = await runBatchItem(batch, path);
      if (outcome === "password") { batch.pending = path; return; }
      recordBatchOutcome(batch, path, outcome);
      stopped = outcome === "cancelled";
    }
    shellBatch.current = null;
    const total = batch.done + batch.skipped.length + batch.failed.length + batch.remaining.length;
    if (total > 1) {
      setStatus([
        `${batch.done} of ${total} archives ${batch.kind === "extract" ? "extracted" : "passed the integrity test"}`,
        batch.skipped.length ? `${batch.skipped.length} skipped` : "",
        batch.failed.length ? `${batch.failed.length} failed` : "",
        batch.remaining.length ? `${batch.remaining.length} not started` : "",
      ].filter(Boolean).join(", ") + ".");
      if (batch.failed.length) setError({ code: "finder_selection_incomplete", message: `Could not ${batch.kind === "extract" ? "extract" : "test"} ${batch.failed.join(", ")}. Open an archive in Archi to see the reason.` });
    }
    if (batch.kind === "extract" && settings.revealOnCompletion && batch.firstDestination) {
      try { await openDestination(batch.firstDestination); } catch (caught) { setError(archiveError(caught)); }
    }
  }

  function recordBatchOutcome(batch: ShellBatch, path: string, outcome: RunOutcome | "dismissed") {
    if (outcome === "done") {
      batch.done += 1;
      if (batch.kind === "extract") batch.firstDestination ??= batch.destination ?? parentPath(path);
    } else if (outcome === "failed" || outcome === "conflict") batch.failed.push(leafName(path));
    else batch.skipped.push(leafName(path));
  }

  // Finder actions run on their own archives and leave the one being browsed alone.
  async function runBatchItem(batch: ShellBatch, path: string): Promise<RunOutcome> {
    return batch.kind === "extract"
      ? runExtraction(path, batch.destination ?? parentPath(path), [], false, undefined, false, "keepBoth", true)
      : runTest(path);
  }

  // Dismissing the prompt skips that archive; the rest of a Finder selection still runs.
  function cancelPasswordPrompt() {
    const waiting = passwordAction && shellBatch.current?.pending === passwordAction.path;
    clearPasswordPrompt();
    if (waiting) void continueShellBatch("dismissed");
  }

  async function loadArchive(path: string, suppliedPassword?: string): Promise<boolean> {
    if (blockedByJob()) return false;
    shellBatch.current = null;
    setOperation("opening"); setError(null); setStatus("Reading archive…");
    const usedPassword = suppliedPassword ?? knownPassword(path);
    try {
      const document = await openArchive(path, usedPassword);
      installDocument(document, usedPassword !== undefined);
      rememberPassword(path, usedPassword);
      setStatus(`${document.entryCount.toLocaleString()} entries`);
      void refreshRecents();
      clearPasswordPrompt();
      void recordDiagnostic("open").catch(() => undefined);
      return true;
    } catch (caught) {
      const failure = archiveError(caught);
      if (failure.code === "cancelled") { setStatus("Opening cancelled."); clearPasswordPrompt(); }
      else if (isPasswordError(failure)) {
        forgetPassword(path);
        setPasswordError(suppliedPassword === undefined ? null : failure.message);
        setPasswordAction({ kind: "open", path }); setStatus(suppliedPassword === undefined ? "Password required to open archive." : "The password was not accepted. Try again.");
      } else { setError(failure); setStatus("Could not open archive."); }
      return false;
    } finally { setOperation(null); setJob(null); }
  }

  function installDocument(document: ArchiveDocument, passwordProvided: boolean) {
    preparedDrag.current = null;
    archivePathRef.current = document.path;
    if (sessionPassword.current?.path !== document.path) sessionPassword.current = null;
    setArchive(document);
    setCommentDraft(document.comment ?? "");
    setArchiveNeedsPassword(passwordProvided || document.encrypted);
    resetBrowser(); setArchiveOutdated(false); setMonitorChanges(true);
  }

  function closeArchive() {
    preparedDrag.current = null; shellBatch.current = null; sessionPassword.current = null;
    setArchive(null); setEntries(null); setCommentDraft(""); setArchiveNeedsPassword(false);
    setArchiveOutdated(false); setMonitorChanges(false); setEntryMenu(null); setPropertiesEntry(null);
    setRenameDialog(null); setCommentOpen(false); setExtractDialog(null); clearPasswordPrompt();
    resetBrowser(); setStatus("Choose an archive to inspect its contents.");
  }

  function resetBrowser() {
    setFolder(""); setFolderHistory([""]); setFolderHistoryIndex(0); setQuery(""); setFileType("");
    setPageNumber(1); setSelected(new Set()); setLastSelected(null);
  }

  function navigateFolder(next: string) {
    setFolder(next); setQuery(""); setFileType(""); setPageNumber(1); setSelected(new Set());
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
    setFolderHistoryIndex(nextIndex); setFolder(next); setQuery(""); setFileType(""); setPageNumber(1); setSelected(new Set());
  }

  function goUp() {
    if (folder) navigateFolder(folder.includes("/") ? folder.slice(0, folder.lastIndexOf("/")) : "");
  }

  async function requestExtraction(paths: string[] = []) {
    if (!archive || blockedByJob()) return;
    shellBatch.current = null;
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
    const usedPassword = suppliedPassword ?? knownPassword(archive.path);
    try {
      await openArchiveEntry(archive.path, entry.path, quickLook, usedPassword);
      rememberPassword(archive.path, usedPassword);
      setStatus(quickLook ? `Previewing ${leafName(entry.path)}.` : `Opened a temporary read-only copy of ${leafName(entry.path)}.`);
      clearPasswordPrompt(); void recordDiagnostic("open").catch(() => undefined);
      return true;
    } catch (caught) {
      const failure = archiveError(caught);
      if (failure.code === "cancelled") { setStatus("Cancelled."); clearPasswordPrompt(); }
      else if (isPasswordError(failure)) {
        forgetPassword(archive.path);
        setPasswordError(suppliedPassword === undefined ? null : failure.message);
        setPasswordAction({ kind: "entry", path: archive.path, entry: entry.path, quickLook });
        setStatus(suppliedPassword === undefined ? "Enter the archive password and try again." : "The password was not accepted. Try again.");
      } else {
        setError(failure);
        setStatus(quickLook ? "Quick Look failed." : "Could not open the archive entry.");
        clearPasswordPrompt();
      }
      return false;
    } finally { setOperation(null); setJob(null); }
  }

  async function requestEntryDrag(entry: ArchiveEntry, suppliedPassword?: string, start = true): Promise<boolean> {
    if (!archive || busy || entry.isDirectory || isSymbolicLink(entry)) { draggingOut.current = false; return false; }
    setOperation("opening"); setError(null); setStatus(`Preparing ${leafName(entry.path)} to drag…`);
    const usedPassword = suppliedPassword ?? knownPassword(archive.path);
    try {
      const cached = preparedDrag.current;
      const path = cached?.archive === archive.path && cached.entry === entry.path
        ? cached.path
        : await prepareArchiveEntry(archive.path, entry.path, usedPassword);
      rememberPassword(archive.path, usedPassword);
      preparedDrag.current = { archive: archive.path, entry: entry.path, path };
      clearPasswordPrompt();
      if (!start) {
        setStatus(`${leafName(entry.path)} is ready. Drag it to Finder again.`);
        return true;
      }
      const key = entryIconKey(entry);
      const icon = nativeIcons[key] ?? (await entryIcons([key]))[key] ?? await imageDataUrl(appIcon);
      await startDrag({ item: [path], icon, mode: "copy" }, ({ result }) => {
        draggingOut.current = false; setDragActive(false);
        setStatus(result === "Dropped" ? `${leafName(entry.path)} extracted by drag.` : "File drag cancelled.");
      });
      return true;
    } catch (caught) {
      draggingOut.current = false; setDragActive(false);
      const failure = archiveError(caught);
      if (failure.code === "cancelled") { setStatus("File drag cancelled."); clearPasswordPrompt(); }
      else if (isPasswordError(failure)) {
        forgetPassword(archive.path);
        setPasswordError(suppliedPassword === undefined ? null : failure.message);
        setPasswordAction({ kind: "drag", path: archive.path, entry: entry.path });
        setStatus(suppliedPassword === undefined ? "Enter the archive password, then drag the file again." : "The password was not accepted. Try again.");
      } else {
        setError(failure); setStatus("Could not prepare the file for dragging."); clearPasswordPrompt();
      }
      return false;
    } finally { setOperation(null); setJob(null); }
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
    if (!archive || !extractDialog || blockedByJob()) return;
    const request = extractDialog;
    setConflictMessage(null);
    setExtractDialog(null);
    if (archiveNeedsPassword && !knownPassword(archive.path)) {
      setPasswordError(null);
      setPasswordAction({ kind: "extract", path: archive.path, destination: request.destination, entries: request.entries, reveal: revealExtraction });
    } else await runExtraction(archive.path, request.destination, request.entries, revealExtraction);
  }

  async function runExtraction(archivePath: string, destination: string, paths: string[], reveal: boolean, suppliedPassword?: string, allowUnbounded = false, policy: ConflictPolicy = conflictPolicy, enclose = false): Promise<RunOutcome> {
    setOperation("extracting"); setError(null); setStatus(paths.length ? "Extracting selected entries…" : `Extracting ${leafName(archivePath)}…`);
    const usedPassword = suppliedPassword ?? knownPassword(archivePath);
    try {
      const result = await extractArchive(archivePath, destination, policy, paths.length ? paths : undefined, usedPassword, settings.maxExpandedBytes, allowUnbounded, enclose);
      rememberPassword(archivePath, usedPassword);
      const changes = [
        `${result.filesExtracted.toLocaleString()} ${result.filesExtracted === 1 ? "file" : "files"} extracted`,
        result.filesSkipped ? `${result.filesSkipped.toLocaleString()} skipped` : "",
        result.renamed ? `${result.renamed.toLocaleString()} renamed` : "",
        result.linksSkipped ? `${result.linksSkipped.toLocaleString()} ${result.linksSkipped === 1 ? "link" : "links"} skipped` : "",
        result.warningCount ? `${result.warningCount.toLocaleString()} warnings` : "",
      ].filter(Boolean);
      setStatus(`${changes.join(", ")} in ${formatDuration(result.elapsedMs)}`);
      clearPasswordPrompt(); void recordDiagnostic("extract").catch(() => undefined);
      void notifyCompletion("Extraction complete", changes.join(", "));
      if (reveal) { try { await openDestination(result.destination); } catch (caught) { setError(archiveError(caught)); } }
      return "done";
    } catch (caught) {
      const failure = archiveError(caught);
      if (failure.code === "cancelled") { setStatus("Extraction cancelled. No destination files were changed."); clearPasswordPrompt(); return "cancelled"; }
      else if (!allowUnbounded && (failure.code === "expansion_limit_exceeded" || failure.code === "expansion_size_unknown")) {
        if (await confirm(`${failure.message}\n\nContinue this extraction anyway?`, { title: "Extraction size warning", kind: "warning" })) {
          return await runExtraction(archivePath, destination, paths, reveal, suppliedPassword, true, policy, enclose);
        }
        setStatus("Extraction cancelled."); clearPasswordPrompt();
        return "skipped";
      }
      else if (isPasswordError(failure)) {
        forgetPassword(archivePath);
        setPasswordError(suppliedPassword === undefined ? null : failure.message);
        setPasswordAction({ kind: "extract", path: archivePath, destination, entries: paths, reveal, conflictPolicy: policy, enclose });
        setStatus(suppliedPassword === undefined ? "Enter the archive password and try again." : "The password was not accepted. Try again.");
        return "password";
      } else if (failure.code === "conflict" && policy === "ask") {
        setConflictMessage(failure.message); setConflictPolicy("keepBoth"); setRevealExtraction(reveal);
        setExtractDialog({ entries: paths, destination }); setStatus("Choose how to handle the existing files.");
        return "conflict";
      }
      setError(failure); setStatus("Extraction failed."); clearPasswordPrompt();
      return "failed";
    } finally { setOperation(null); setJob(null); }
  }

  async function runTest(archivePath: string, suppliedPassword?: string): Promise<RunOutcome> {
    setOperation("testing"); setError(null); setStatus(`Testing ${leafName(archivePath)}…`);
    const usedPassword = suppliedPassword ?? knownPassword(archivePath);
    try {
      const result = await testArchive(archivePath, usedPassword);
      rememberPassword(archivePath, usedPassword);
      setStatus(result.warningCount ? `Integrity test passed with ${result.warningCount} warnings in ${formatDuration(result.elapsedMs)}.` : `Integrity test passed in ${formatDuration(result.elapsedMs)}.`);
      clearPasswordPrompt(); void recordDiagnostic("test").catch(() => undefined); return "done";
    } catch (caught) {
      const failure = archiveError(caught);
      if (failure.code === "cancelled") { setStatus("Integrity test cancelled."); clearPasswordPrompt(); return "cancelled"; }
      if (isPasswordError(failure)) { forgetPassword(archivePath); setPasswordError(suppliedPassword === undefined ? null : failure.message); setPasswordAction({ kind: "test", path: archivePath }); setStatus(suppliedPassword === undefined ? "Enter the archive password and try again." : "The password was not accepted. Try again."); return "password"; }
      setError(failure); setStatus("Integrity test failed."); clearPasswordPrompt();
      return "failed";
    } finally { setOperation(null); setJob(null); }
  }

  async function requestTest() {
    if (!archive || blockedByJob()) return;
    shellBatch.current = null;
    if (archiveNeedsPassword && !knownPassword(archive.path)) { setPasswordError(null); setPasswordAction({ kind: "test", path: archive.path }); } else await runTest(archive.path);
  }

  async function chooseArchiveAdditions(directory: boolean) {
    if (!archive?.canModify) return;
    const paths = await open({
      multiple: true,
      directory,
      title: directory ? "Add Folders to Archive" : "Add Files to Archive",
    });
    if (!paths) return;
    await requestArchiveAdditions(Array.isArray(paths) ? paths : [paths]);
  }

  // Items are added to the folder being browsed, as they would be in Finder.
  async function requestArchiveAdditions(inputs: string[]) {
    if (!archive?.canModify || !inputs.length || blockedByJob()) return;
    const level = compressionFor(archive.path.toLowerCase().endsWith(".zip") ? "zip" : "sevenZip", settings);
    if (archiveNeedsPassword && !knownPassword(archive.path)) {
      setPasswordError(null);
      setPasswordAction({ kind: "add", path: archive.path, inputs, compression: level, folder, replaceExisting: false });
    } else await runAdditions(archive.path, inputs, level, undefined, folder, false);
  }

  async function runAdditions(path: string, inputs: string[], level: CompressionLevel, suppliedPassword: string | undefined, targetFolder: string, replaceExisting: boolean): Promise<boolean> {
    setOperation("modifying"); setError(null); setStatus("Adding items to a recoverable archive copy…");
    const usedPassword = suppliedPassword ?? knownPassword(path);
    let existing: ArchiveError | null = null;
    try {
      const document = await addToArchive(path, inputs, level, usedPassword, targetFolder, replaceExisting);
      installDocument(document, archiveNeedsPassword || usedPassword !== undefined);
      rememberPassword(path, usedPassword);
      if (targetFolder) navigateFolder(targetFolder);
      setStatus(`Added ${inputs.length.toLocaleString()} ${inputs.length === 1 ? "item" : "items"} to ${targetFolder || document.name}.`);
      clearPasswordPrompt(); void recordDiagnostic("modify").catch(() => undefined); return true;
    } catch (caught) {
      const failure = archiveError(caught);
      if (failure.code === "entry_exists" && !replaceExisting) existing = failure;
      else return handleModificationError(caught, suppliedPassword, { kind: "add", path, inputs, compression: level, folder: targetFolder, replaceExisting }, "Add failed.");
    } finally { setOperation(null); setJob(null); }
    clearPasswordPrompt();
    if (await confirm(`${existing.message}\n\nReplace the existing ${inputs.length === 1 ? "item" : "items"} with the ${inputs.length === 1 ? "one" : "ones"} you are adding?`, { title: "Replace existing items", kind: "warning" })) {
      return runAdditions(path, inputs, level, usedPassword, targetFolder, true);
    }
    setStatus("Nothing was added. The archive was not changed.");
    return false;
  }

  async function requestDelete(paths: string[]) {
    if (!archive?.canModify || !paths.length || blockedByJob()) return;
    if (!await confirm(`Delete ${paths.length} selected ${paths.length === 1 ? "entry" : "entries"} from ${archive.name}?`, { title: "Delete archive entries", kind: "warning" })) return;
    if (archiveNeedsPassword && !knownPassword(archive.path)) {
      setPasswordError(null); setPasswordAction({ kind: "delete", path: archive.path, entries: paths });
    } else await runDelete(archive.path, paths);
  }

  async function runDelete(path: string, paths: string[], suppliedPassword?: string) {
    setOperation("modifying"); setError(null); setStatus("Deleting from a recoverable archive copy…");
    const usedPassword = suppliedPassword ?? knownPassword(path);
    try {
      const document = await deleteArchiveEntries(path, paths, usedPassword);
      installDocument(document, archiveNeedsPassword || usedPassword !== undefined);
      rememberPassword(path, usedPassword);
      setStatus(`Deleted ${paths.length.toLocaleString()} selected ${paths.length === 1 ? "entry" : "entries"}.`);
      clearPasswordPrompt(); void recordDiagnostic("modify").catch(() => undefined); return true;
    } catch (caught) {
      return handleModificationError(caught, suppliedPassword, { kind: "delete", path, entries: paths }, "Delete failed.");
    } finally { setOperation(null); setJob(null); }
  }

  function requestRename(entry: ArchiveEntry) {
    if (archive?.canModify && !blockedByJob()) setRenameDialog({ entry, name: leafName(entry.path) });
  }

  async function submitRename(event: React.FormEvent) {
    event.preventDefault();
    if (!archive || !renameDialog || blockedByJob()) return;
    const request = { entry: renameDialog.entry.path, newName: renameDialog.name };
    setRenameDialog(null);
    if (archiveNeedsPassword && !knownPassword(archive.path)) {
      setPasswordError(null); setPasswordAction({ kind: "rename", path: archive.path, ...request });
    } else await runRename(archive.path, request.entry, request.newName);
  }

  async function runRename(path: string, entry: string, newName: string, suppliedPassword?: string) {
    setOperation("modifying"); setError(null); setStatus("Renaming in a recoverable archive copy…");
    const usedPassword = suppliedPassword ?? knownPassword(path);
    try {
      const document = await renameArchiveEntry(path, entry, newName, usedPassword);
      installDocument(document, archiveNeedsPassword || usedPassword !== undefined);
      rememberPassword(path, usedPassword);
      setStatus(`Renamed ${leafName(entry)} to ${newName}.`);
      clearPasswordPrompt(); void recordDiagnostic("modify").catch(() => undefined); return true;
    } catch (caught) {
      return handleModificationError(caught, suppliedPassword, { kind: "rename", path, entry, newName }, "Rename failed.");
    } finally { setOperation(null); setJob(null); }
  }

  function openCommentEditor() {
    if (!archive?.canModify || !archive.path.toLowerCase().endsWith(".zip") || blockedByJob()) return;
    setCommentDraft(archive.comment ?? ""); setCommentOpen(true);
  }

  async function submitComment(event: React.FormEvent) {
    event.preventDefault();
    if (!archive || blockedByJob()) return;
    setCommentOpen(false);
    if (archiveNeedsPassword && !knownPassword(archive.path)) {
      setPasswordError(null); setPasswordAction({ kind: "comment", path: archive.path, comment: commentDraft });
    } else await runComment(archive.path, commentDraft);
  }

  async function runComment(path: string, comment: string, suppliedPassword?: string) {
    setOperation("modifying"); setError(null); setStatus("Saving ZIP comment safely…");
    const usedPassword = suppliedPassword ?? knownPassword(path);
    try {
      const document = await setArchiveComment(path, comment, usedPassword);
      installDocument(document, archiveNeedsPassword || usedPassword !== undefined);
      rememberPassword(path, usedPassword);
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
      forgetPassword(action.path);
      setPasswordError(suppliedPassword === undefined ? null : failure.message);
      setPasswordAction(action); setStatus(suppliedPassword === undefined ? "Enter the archive password and try again." : "The password was not accepted. Try again.");
    } else { setError(failure); setStatus(fallback); clearPasswordPrompt(); }
    return false;
  }

  async function submitPassword(event: React.FormEvent) {
    event.preventDefault();
    if (!passwordAction || !password || blockedByJob()) return;
    if (passwordAction.kind === "open") await loadArchive(passwordAction.path, password);
    else if (passwordAction.kind === "entry") {
      const entry = entries?.entries.find((candidate) => candidate.path === passwordAction.entry);
      if (entry) await requestEntryOpen(entry, passwordAction.quickLook, password);
      else {
        setError({ code: "entry_not_found", message: "The archive entry is no longer visible." });
        clearPasswordPrompt();
      }
    }
    else if (passwordAction.kind === "drag") {
      const entry = entries?.entries.find((candidate) => candidate.path === passwordAction.entry);
      if (entry) await requestEntryDrag(entry, password, false);
      else {
        setError({ code: "entry_not_found", message: "The archive entry is no longer visible." });
        clearPasswordPrompt();
      }
    }
    else if (passwordAction.kind === "extract" || passwordAction.kind === "test") {
      const outcome = passwordAction.kind === "extract"
        ? await runExtraction(passwordAction.path, passwordAction.destination, passwordAction.entries, passwordAction.reveal, password, false, passwordAction.conflictPolicy ?? conflictPolicy, passwordAction.enclose ?? false)
        : await runTest(passwordAction.path, password);
      if (shellBatch.current?.pending === passwordAction.path) await continueShellBatch(outcome);
    }
    else if (passwordAction.kind === "add") await runAdditions(passwordAction.path, passwordAction.inputs, passwordAction.compression, password, passwordAction.folder, passwordAction.replaceExisting);
    else if (passwordAction.kind === "delete") await runDelete(passwordAction.path, passwordAction.entries, password);
    else if (passwordAction.kind === "rename") await runRename(passwordAction.path, passwordAction.entry, passwordAction.newName, password);
    else await runComment(passwordAction.path, passwordAction.comment, password);
  }

  function openCreateDialog() {
    setCreateFormat(settings.defaultFormat); setCompression(compressionFor(settings.defaultFormat, settings)); setCreateError(null); setCreateOpen(true);
  }
  // The dialog covers the main window's error banner, so its problems are shown inside it.
  function failCreate(message: string, focusId: string) {
    setCreateError(message);
    document.getElementById(focusId)?.focus();
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
    if (blockedByJob()) return;
    if (!createInputs.length) { failCreate("Add at least one file or folder to archive.", "create-add-files"); return; }
    if (isStreamFormat(createFormat) && createInputs.length !== 1) { failCreate("GZIP, XZ, and Zstandard streams hold exactly one file. Remove the extra items or choose a TAR format.", "create-add-files"); return; }
    if (!createOutput) { failCreate("Choose where to save the archive.", "create-output-choose"); return; }
    if (encrypt && !createPassword) { failCreate("Enter a password, or turn off encryption.", "create-password"); return; }
    if (encrypt && createPassword !== createConfirmation) { failCreate("The passwords do not match.", "create-password-confirmation"); return; }
    setCreateError(null);
    const passwordForJob = encrypt ? createPassword : undefined;
    const confirmationForJob = encrypt ? createConfirmation : undefined;
    setCreatePassword(""); setCreateConfirmation(""); setCreateOpen(false);
    // The save panel already asked before returning an existing path.
    const created = await runCreation(createInputs, createOutput, createFormat, compression, passwordForJob, confirmationForJob, volumeSize ?? undefined, true, (failure) => setCreateError(failure.message));
    if (!created) setCreateOpen(true);
  }

  async function runCreation(inputs: string[], output: string, format: ArchiveFormat, level: CompressionLevel, passwordForJob?: string, confirmationForJob?: string, splitSize?: number, replaceExisting = false, onFailure?: (failure: ArchiveError) => void, openResult = true, replaceVolumes = false): Promise<boolean> {
    setOperation("creating"); setError(null); setStatus("Creating archive…");
    let oldSet: ArchiveError | null = null;
    try {
      const document = await createArchive(inputs, output, format, level, passwordForJob, confirmationForJob, splitSize, replaceExisting, openResult, replaceVolumes);
      if (openResult) {
        setArchive(document); setArchiveNeedsPassword(passwordForJob !== undefined); resetBrowser();
        archivePathRef.current = document.path;
        sessionPassword.current = passwordForJob === undefined ? null : { path: document.path, password: passwordForJob };
        setArchiveOutdated(false); setMonitorChanges(true);
      }
      setStatus(`Created ${document.name} with ${document.entryCount.toLocaleString()} ${document.entryCount === 1 ? "entry" : "entries"}${document.volumeCount > 1 ? ` across ${document.volumeCount} volumes` : ""}.${document.skippedLinks ? ` Skipped ${document.skippedLinks.toLocaleString()} symbolic ${document.skippedLinks === 1 ? "link" : "links"}.` : ""}`);
      setCreateInputs([]); setCreateOutput(""); setVolumeSize(null); void refreshRecents(); void recordDiagnostic("create").catch(() => undefined);
      void notifyCompletion("Archive created", document.name);
      return true;
    } catch (caught) {
      const failure = archiveError(caught);
      if (failure.code === "volumes_exist" && !replaceVolumes) oldSet = failure;
      else {
        if (failure.code === "cancelled") setStatus("Archive creation cancelled. No output archive was created.");
        else { if (onFailure) onFailure(failure); else setError(failure); setStatus("Archive creation failed."); }
        return false;
      }
    } finally { setOperation(null); setJob(null); }
    // Nothing has been compressed yet; the old split archive is only replaced on request.
    if (await confirm(`${oldSet.message}\n\nReplace the existing split archive? All of its files will be removed.`, { title: "Replace split archive", kind: "warning", okLabel: "Replace", cancelLabel: "Cancel" })) {
      return runCreation(inputs, output, format, level, passwordForJob, confirmationForJob, splitSize, replaceExisting, onFailure, openResult, true);
    }
    setStatus("No archive was created.");
    if (onFailure) onFailure({ code: "volumes_exist", message: "Choose a different name, or replace the existing split archive." });
    return false;
  }

  function openSettings() {
    setSettingsDraft({ ...settings }); setSettingsError(null); setSettingsOpen(true); void refreshIntegration();
  }
  async function refreshIntegration() {
    try { setIntegration(await shellIntegrationStatus()); }
    catch (caught) { setError(archiveError(caught)); }
  }
  async function chooseCustomDestination() {
    const destination = await open({ multiple: false, directory: true, title: "Default Extraction Folder" });
    if (destination) setSettingsDraft({ ...settingsDraft, customDestination: destination });
  }
  // The dialog covers the main window's error banner, so its problems are shown inside it.
  function failSettings(message: string) {
    setSettingsError(message);
    document.querySelector(".settings-dialog")?.scrollTo({ top: 0 });
  }
  async function submitSettings(event: React.FormEvent) {
    event.preventDefault();
    setSettingsError(null);
    try {
      if (settingsDraft.notifications && !await notificationPermission()) {
        failSettings("Notifications for Archi are turned off in System Settings. Allow them there, or turn off completion notifications.");
        return;
      }
      const saved = await saveSettings({ ...settingsDraft, defaultCompression: compressionFor(settingsDraft.defaultFormat, settingsDraft) });
      setSettings(saved); setSettingsDraft(saved); setCreateFormat(saved.defaultFormat); setCompression(compressionFor(saved.defaultFormat, saved)); setSettingsOpen(false); setStatus("Settings saved.");
      if (saved.historyEnabled) void refreshRecents(); else setRecents([]);
    } catch (caught) { failSettings(archiveError(caught).message); }
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

  async function selectAllInView() {
    if (!archive || !entries) return;
    if (entries.entries.length >= entries.total) { setSelected(new Set(entries.entries.map((entry) => entry.path))); return; }
    const archivePath = archive.path;
    try {
      const paths = await entryPaths(archivePath, folder, query, fileType, settings.showHiddenEntries);
      if (archivePathRef.current === archivePath) setSelected(new Set(paths));
    } catch (caught) { setError(archiveError(caught)); }
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

  function loadMoreWhenNearEnd(event: React.UIEvent<HTMLDivElement>) {
    const list = event.currentTarget;
    if (!entries || loadingMore.current || entries.entries.length >= entries.total) return;
    if (list.scrollTop + list.clientHeight >= list.scrollHeight - 600) {
      loadingMore.current = true;
      setPageNumber((value) => value + 1);
    }
  }

  function openRowMenu(row: HTMLTableRowElement, entry: ArchiveEntry) {
    const box = row.getBoundingClientRect();
    if (!selected.has(entry.path)) setSelected(new Set([entry.path]));
    menuReturnFocus.current = row;
    setEntryMenu(contextMenuPosition({ clientX: box.left + 48, clientY: box.bottom - 4 }, entry));
  }

  function handleMenuKey(event: React.KeyboardEvent<HTMLDivElement>) {
    if (!["ArrowDown", "ArrowUp", "Home", "End"].includes(event.key)) return;
    event.preventDefault();
    const items = [...event.currentTarget.querySelectorAll<HTMLElement>("[role='menuitem']")];
    if (!items.length) return;
    const current = Math.max(0, items.indexOf(document.activeElement as HTMLElement));
    const next = event.key === "Home" ? 0 : event.key === "End" ? items.length - 1
      : (current + (event.key === "ArrowDown" ? 1 : -1) + items.length) % items.length;
    items[next].focus();
  }

  function handleRowKey(event: React.KeyboardEvent<HTMLTableRowElement>, index: number, entry: ArchiveEntry) {
    if (event.key === "ContextMenu" || (event.key === "F10" && event.shiftKey)) { event.preventDefault(); openRowMenu(event.currentTarget, entry); }
    else if (event.key === "Enter") { event.preventDefault(); if (entry.isDirectory) navigateFolder(entry.path); else void requestEntryOpen(entry, false); }
    else if (event.key === "Backspace" && (event.metaKey || event.ctrlKey)) { event.preventDefault(); if (!selected.size) setSelected(new Set([entry.path])); runCommand("delete", selected.size ? undefined : [entry.path]); }
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
  // Shown beneath the toolbar so the window controls and toolbar never move.
  const banners = <>
    {error && <section className="error-banner" role="alert"><div><strong>{errorTitle(error.code)}</strong><p>{error.message}</p></div><button className="icon-button" onClick={() => setError(null)} aria-label="Dismiss error"><Icon name="close" /></button></section>}
    {archiveOutdated && archive && <section className="change-banner" role="alert"><div><strong>Archive changed on disk</strong><p>Reload to browse the current file. No archive contents have been edited by this app.</p></div><div className="header-actions"><button onClick={() => { setArchiveOutdated(false); setMonitorChanges(false); }}>Keep browsing</button><button className="primary-button" onClick={() => void loadArchive(archive.path)}>Reload</button></div></section>}
    {job && (operation !== "opening" || job.elapsedMs >= OPENING_SHELF_DELAY_MS) && <JobShelf job={job} onCancel={() => void requestCancellation()} />}
  </>;
  // A right-click on a selected row acts on the whole selection, as in Finder.
  const menuPaths = entryMenu ? (selected.size > 1 && selected.has(entryMenu.entry.path) ? [...selected] : [entryMenu.entry.path]) : [];
  const menuMulti = menuPaths.length > 1;
  return (
    <main className={`app-shell${dragActive ? " drag-active" : ""}`}>
      {!archive && <header className="welcome-toolbar" data-tauri-drag-region>
        <div className="welcome-title" data-tauri-drag-region><strong>Archi</strong></div>
      </header>}
      {!archive && banners}
      {dragActive && <div className="drop-overlay" role="status">{createOpen
        ? "Drop files or folders to include in the new archive"
        : archive?.canModify
          ? `Drop files or folders to add to ${folder || archive.name}`
          : "Drop an archive to open it, or files and folders to compress"}</div>}

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
              <span className="sr-only">Filter by filename</span>
              <input ref={searchRef} type="search" value={query} onChange={(event) => { setQuery(event.target.value); setPageNumber(1); setSelected(new Set()); }} placeholder="Filter by name" />
              {query && <button type="button" className="search-clear" aria-label="Clear search" onClick={() => { setQuery(""); setPageNumber(1); setSelected(new Set()); searchRef.current?.focus(); }}><Icon name="close" /></button>}
            </label>
            {archive.canModify && <PopupMenu label="Add to archive" open={addMenuOpen} onOpenChange={setAddMenuOpen} disabled={busy} triggerClassName="toolbar-label-button" trigger={<><Icon name="add" /><span>Add</span><Icon name="disclosureDown" /></>}>
              <button role="menuitem" onClick={() => { setAddMenuOpen(false); void chooseArchiveAdditions(false); }}>Add Files…</button>
              <button role="menuitem" onClick={() => { setAddMenuOpen(false); void chooseArchiveAdditions(true); }}>Add Folder…</button>
            </PopupMenu>}
            <button className="toolbar-label-button" onClick={requestTest} disabled={busy} title="Test archive" aria-label="Test archive"><Icon name="test" /><span>Test</span></button>
            <PopupMenu label="Extract options" open={extractMenuOpen} onOpenChange={setExtractMenuOpen} disabled={busy} triggerClassName="primary-button toolbar-label-button" trigger={<><Icon name="extract" /><span>Extract</span><Icon name="disclosureDown" /></>}>
              {selected.size > 0 && <button role="menuitem" onClick={() => { setExtractMenuOpen(false); void requestExtraction([...selected]); }}>Extract Selected ({selected.size})…</button>}
              <button role="menuitem" onClick={() => { setExtractMenuOpen(false); void requestExtraction([]); }}>Extract All…</button>
            </PopupMenu>
            <PopupMenu label="More actions" open={moreOpen} onOpenChange={setMoreOpen} trigger={<Icon name="more" />}>
                {archive.canModify && selected.size === 1 && selectedEntries.length === 1 && <button role="menuitem" onClick={() => { setMoreOpen(false); requestRename(selectedEntries[0]); }}>Rename…</button>}
                {archive.canModify && selected.size > 0 && <button className="danger-button" role="menuitem" onClick={() => { setMoreOpen(false); void requestDelete([...selected]); }}>Delete</button>}
                {archive.path.toLowerCase().endsWith(".zip") && <button role="menuitem" onClick={() => { setMoreOpen(false); openCommentEditor(); }}>Comment…</button>}
                <button role="menuitem" onClick={() => { setMoreOpen(false); openSettings(); }}>Settings…</button>
            </PopupMenu>
          </div>
        </div>
        {banners}
        <div className="browser-bar">
          <nav className="breadcrumbs" aria-label="Archive folder"><button onClick={() => navigateFolder("")} aria-current={!folder ? "page" : undefined}>{archive.name}</button>{crumbs.map((crumb, index) => { const path = crumbs.slice(0, index + 1).join("/"); return <span key={path}><span aria-hidden="true">/</span><button onClick={() => navigateFolder(path)} aria-current={path === folder ? "page" : undefined}>{crumb}</button></span>; })}</nav>
          <label className="type-filter"><span>Type</span><select value={fileType} onChange={(event) => { setFileType(event.target.value); setPageNumber(1); setSelected(new Set()); }}><option value="">All types</option>{fileType && !entries?.fileTypes.includes(fileType) && <option value={fileType}>{fileTypeLabel(fileType)}</option>}{entries?.fileTypes.map((type) => <option key={type} value={type}>{fileTypeLabel(type)}</option>)}</select></label>
        </div>
        <div className={`browser-content${sidebarVisible ? " with-sidebar" : ""}`}>
          {sidebarVisible && <aside className="folder-sidebar" aria-label="Archive folders">
            <button className={`sidebar-root${folder === "" ? " active" : ""}`} onClick={() => navigateFolder("")}><Icon name="archive" /><span>{archive.name}</span></button>
            <FolderTree nodes={folderChildren[""] ?? []} childrenByFolder={folderChildren} activePath={folder} expanded={expandedFolders} onToggle={(node) => void toggleFolder(node)} onNavigate={navigateFolder} />
          </aside>}
          <div className="table-wrap" ref={tableRef} onScroll={loadMoreWhenNearEnd}><table><thead><tr><SortableHeader label="Name" value="name" current={sort} descending={descending} onSort={changeSort} /><SortableHeader label="Type" value="type" current={sort} descending={descending} onSort={changeSort} /><SortableHeader label="Size" value="size" current={sort} descending={descending} onSort={changeSort} /><SortableHeader label="Compressed" value="packedSize" current={sort} descending={descending} onSort={changeSort} /><SortableHeader label="Ratio" value="ratio" current={sort} descending={descending} onSort={changeSort} /><SortableHeader label="Modified" value="modified" current={sort} descending={descending} onSort={changeSort} /></tr></thead>
          <tbody>{entries?.entries.map((entry, index) => <tr key={entry.path} data-entry={entry.path} draggable={!busy && !entry.isDirectory && !isSymbolicLink(entry)} tabIndex={index === 0 || selected.has(entry.path) ? 0 : -1} aria-selected={selected.has(entry.path)} aria-label={entryLabel(entry)} className={selected.has(entry.path) ? "selected" : undefined} onClick={(event) => selectEntry(event, entry)} onDragStart={(event) => { event.preventDefault(); draggingOut.current = true; setDragActive(false); setSelected(new Set([entry.path])); void requestEntryDrag(entry); }} onKeyDown={(event) => handleRowKey(event, index, entry)} onDoubleClick={() => entry.isDirectory ? navigateFolder(entry.path) : void requestEntryOpen(entry, false)} onContextMenu={(event) => { event.preventDefault(); if (!selected.has(entry.path)) setSelected(new Set([entry.path])); setEntryMenu(contextMenuPosition(event, entry)); }} title={entry.isDirectory ? "Double-click to open; right-click for actions" : "Double-click to open; drag to Finder to extract; press Spacebar for Quick Look"}>
            <td className="entry-name" title={entry.path}><EntryIcon entry={entry} source={nativeIcons[entryIconKey(entry)]} />{leafName(entry.path)}{entry.encrypted && <span className="entry-lock" title="Encrypted"><Icon name="lock" /></span>}{query && parentEntryPath(entry.path) && <small>{parentEntryPath(entry.path)}</small>}</td><td>{entryTypeLabel(entry)}</td><td>{entry.size === null ? "—" : formatBytes(entry.size)}</td><td>{entry.packedSize === null ? "—" : formatBytes(entry.packedSize)}</td><td>{formatRatio(entry)}</td><td title={entry.modified ?? undefined}>{formatModified(entry.modified)}</td>
          </tr>)}{entries?.total === 0 && <tr><td colSpan={6} className="no-results">No entries match this view.</td></tr>}</tbody></table></div>
        </div>
        <footer className="status-bar"><span aria-live="polite">{status}</span><span className="status-counts">{selected.size ? `${selected.size.toLocaleString()} selected${selectedEntries.length === selected.size ? ` · ${formatBytes(selectedSize)}` : ""} · ` : ""}{entries ? `${entries.total.toLocaleString()} ${entries.total === 1 ? "item" : "items"}` : "Loading…"}</span></footer>
      </section> : <section className="empty-state">
        <div className="empty-hero">
          <img className="empty-app-icon" src={appIcon} alt="" aria-hidden="true" />
          <h2>Open an Archive</h2>
          <p>Browse, preview or safely extract files entirely on your Mac.</p>
          <div className="empty-actions">
            <button className="primary-button" onClick={chooseArchive} disabled={busy}><Icon name="open" /><span>Open Archive…</span></button>
            <button onClick={openCreateDialog} disabled={busy}><Icon name="add" /><span>New Archive…</span></button>
          </div>
          <p className="drop-hint">Drop an archive to open it, or files and folders to compress</p>
          {settings.historyEnabled && recents.length > 0 && <div className="welcome-recents">
            <h3>Recent</h3>
            <div className="recent-list">{recents.slice(0, 5).map((path) => <button key={path} onClick={() => void loadArchive(path)} disabled={busy} title={path}><Icon name="archive" /><span className="recent-copy"><strong>{leafName(path)}</strong><small>{parentPath(path)}</small></span></button>)}</div>
          </div>}
        </div>
        <span className="empty-status" role="status">{status}</span>
      </section>}

      {entryMenu && <div className="context-menu" role="menu" style={{ left: entryMenu.x, top: entryMenu.y }} onClick={(event) => event.stopPropagation()} onKeyDown={handleMenuKey}>
        {!menuMulti && (entryMenu.entry.isDirectory
          ? <button role="menuitem" onClick={() => { const path = entryMenu.entry.path; setEntryMenu(null); navigateFolder(path); }}>Open Folder</button>
          : <><button role="menuitem" onClick={() => { const entry = entryMenu.entry; setEntryMenu(null); void requestEntryOpen(entry, false); }}>Open</button><button role="menuitem" onClick={() => { const entry = entryMenu.entry; setEntryMenu(null); void requestEntryOpen(entry, true); }}><Icon name="quickLook" />Quick Look</button></>)}
        <button role="menuitem" onClick={() => { setEntryMenu(null); void requestExtraction(menuPaths); }}><Icon name="extract" />Extract {menuMulti ? `${menuPaths.length.toLocaleString()} Items` : entryMenu.entry.isDirectory ? "Folder" : "File"}…</button>
        {archive?.canModify && <>
          {!menuMulti && <button role="menuitem" onClick={() => { const entry = entryMenu.entry; setEntryMenu(null); requestRename(entry); }}>Rename…</button>}
          <button className="danger-button" role="menuitem" onClick={() => { setEntryMenu(null); void requestDelete(menuPaths); }}>Delete{menuMulti ? ` ${menuPaths.length.toLocaleString()} Items` : ""}</button>
        </>}
        <button role="menuitem" onClick={() => { setEntryMenu(null); void copyPaths(menuPaths); }}>{menuMulti ? "Copy Paths" : "Copy Path"}</button>
        {!menuMulti && <button role="menuitem" onClick={() => { setPropertiesEntry(entryMenu.entry); setEntryMenu(null); }}>Properties</button>}
      </div>}
      {aboutOpen && <Modal className="about-dialog" labelledBy="about-title" onClose={() => setAboutOpen(false)}><div className="about-heading"><img src={appIcon} alt="" /><div><h2 id="about-title">Archi</h2>{appVersion && <p>Version {appVersion}</p>}</div></div><p>Fast, private archive management for macOS.</p><dl><dt>Archive engine</dt><dd>7-Zip 26.02</dd></dl><p className="about-legal">7-Zip is licensed separately under the GNU LGPL with the unRAR restriction. Full notices and corresponding source are included with Archi.</p><p>© 2026 Nitivar</p><div className="modal-actions"><button autoFocus onClick={() => setAboutOpen(false)}>Close</button></div></Modal>}
      {propertiesEntry && <PropertiesDialog entry={propertiesEntry} onClose={() => setPropertiesEntry(null)} />}
      {renameDialog && <Modal className="password-dialog" labelledBy="rename-title" onClose={() => setRenameDialog(null)}><form className="modal-form" onSubmit={submitRename}><h2 id="rename-title">Rename archive entry</h2><p>{renameDialog.entry.path}</p><label>New name<input autoFocus value={renameDialog.name} onChange={(event) => setRenameDialog({ ...renameDialog, name: event.target.value })} /></label><div className="modal-actions"><button type="button" onClick={() => setRenameDialog(null)}>Cancel</button><button className="primary-button" type="submit">Rename</button></div></form></Modal>}
      {commentOpen && <Modal className="comment-dialog" labelledBy="comment-title" onClose={() => setCommentOpen(false)}><form className="modal-form" onSubmit={submitComment}><h2 id="comment-title">ZIP archive comment</h2><label>Comment<textarea autoFocus rows={6} maxLength={65535} value={commentDraft} onChange={(event) => setCommentDraft(event.target.value)} /></label><p>{new TextEncoder().encode(commentDraft).length.toLocaleString()} of 65,535 UTF-8 bytes</p><div className="modal-actions"><button type="button" onClick={() => setCommentOpen(false)}>Cancel</button><button className="primary-button" type="submit">Save Comment</button></div></form></Modal>}
      {extractDialog && <ExtractPanel selectedCount={extractDialog.entries.length} destination={extractDialog.destination} error={conflictMessage} policy={conflictPolicy} reveal={revealExtraction} onChooseDestination={() => void changeExtractFolder()} onPolicyChange={setConflictPolicy} onRevealChange={setRevealExtraction} onClose={() => setExtractDialog(null)} onSubmit={submitExtraction} />}
      {passwordAction && <PasswordDialog archiveName={leafName(passwordAction.path)} error={passwordError} password={password} showPassword={showPassword} busy={busy} onPasswordChange={setPassword} onShowPasswordChange={setShowPassword} onClose={cancelPasswordPrompt} onSubmit={submitPassword} />}
      {createOpen && <Modal className="create-dialog" labelledBy="create-title" onClose={() => setCreateOpen(false)}><form className="modal-form" onSubmit={submitCreate}><h2 id="create-title">New Archive</h2>{createError && <p className="inline-error" role="alert">{createError}</p>}<div className="source-picker"><div><strong>Items</strong><span>{createInputs.length ? `${createInputs.length} selected` : "Drop items here or choose them"}</span></div><div className="header-actions"><button id="create-add-files" type="button" onClick={addFiles}>Add Files…</button><button type="button" onClick={addFolder}>Add Folder…</button></div></div>{createInputs.length > 0 && <ul className="source-list">{createInputs.map((path) => <li key={path} title={path}><span>{leafName(path)}</span><button type="button" aria-label={`Remove ${leafName(path)}`} onClick={() => setCreateInputs((current) => current.filter((value) => value !== path))}><Icon name="close" /></button></li>)}</ul>}<label>Output<div className="output-picker"><input readOnly value={createOutput} placeholder="Choose where to save the archive" /><button id="create-output-choose" type="button" onClick={chooseCreateOutput}>Choose…</button></div></label><div className="form-grid"><label>Format<select value={createFormat} onChange={(event) => { const format = event.target.value as ArchiveFormat; setCreateFormat(format); setCompression(compressionFor(format, settings)); setCreateOutput(""); setEncrypt(false); setVolumeSize(null); }}>{createFormats.map((option) => <option key={option.value} value={option.value}>{option.label}</option>)}</select></label><label>Compression<select value={compression} onChange={(event) => setCompression(event.target.value as CompressionLevel)}><option value="store">Store</option><option value="fast">Fast</option><option value="normal">Normal</option><option value="maximum">Maximum</option></select></label>{supportsArchiveOptions(createFormat) && <label>Split into volumes<select value={volumeSize ?? ""} onChange={(event) => setVolumeSize(event.target.value ? Number(event.target.value) : null)}><option value="">Single archive</option><option value={10 * 1024 ** 2}>10 MiB</option><option value={100 * 1024 ** 2}>100 MiB</option><option value={1024 ** 3}>1 GiB</option><option value={4 * 1024 ** 3}>4 GiB</option></select></label>}</div>{isStreamFormat(createFormat) && <p className="compatibility-note">Compressed streams accept one regular file. Use a TAR format for folders or multiple items.</p>}{supportsArchiveOptions(createFormat) && <label className="checkbox-label"><input type="checkbox" checked={encrypt} onChange={(event) => setEncrypt(event.target.checked)} />Encrypt archive</label>}{encrypt && supportsArchiveOptions(createFormat) && <div className="form-grid"><label>Password<input id="create-password" type={showCreatePassword ? "text" : "password"} value={createPassword} onChange={(event) => setCreatePassword(event.target.value)} autoComplete="new-password" /></label><label>Confirm password<input id="create-password-confirmation" type={showCreatePassword ? "text" : "password"} value={createConfirmation} onChange={(event) => setCreateConfirmation(event.target.value)} autoComplete="new-password" /></label><label className="checkbox-label"><input type="checkbox" checked={showCreatePassword} onChange={(event) => setShowCreatePassword(event.target.checked)} />Show passwords</label></div>}{encrypt && createFormat === "zip" && <p className="compatibility-note">ZIP encryption uses AES-256; some older unzip tools may not support it.</p>}<div className="modal-actions"><button type="button" onClick={() => setCreateOpen(false)}>Cancel</button><button className="primary-button" type="submit">Create {createFormats.find(({ value }) => value === createFormat)?.label}</button></div></form></Modal>}
      {settingsOpen && <Modal className="settings-dialog" labelledBy="settings-title" onClose={() => setSettingsOpen(false)}><form className="modal-form" onSubmit={submitSettings}><h2 id="settings-title">Settings</h2>{settingsError && <p className="inline-error" role="alert">{settingsError}</p>}<fieldset><legend>Defaults</legend><div className="form-grid"><label>Archive format<select value={settingsDraft.defaultFormat} onChange={(event) => setSettingsDraft({ ...settingsDraft, defaultFormat: event.target.value as ArchiveFormat })}>{createFormats.map((option) => <option key={option.value} value={option.value}>{option.label}</option>)}</select></label><label>ZIP compression<select value={settingsDraft.zipCompression} onChange={(event) => setSettingsDraft({ ...settingsDraft, zipCompression: event.target.value as CompressionLevel })}><option value="store">Store</option><option value="fast">Fast</option><option value="normal">Normal</option><option value="maximum">Maximum</option></select></label><label>7z compression<select value={settingsDraft.sevenZipCompression} onChange={(event) => setSettingsDraft({ ...settingsDraft, sevenZipCompression: event.target.value as CompressionLevel })}><option value="store">Store</option><option value="fast">Fast</option><option value="normal">Normal</option><option value="maximum">Maximum</option></select></label></div><label>Extraction destination<select value={settingsDraft.extractionDestination} onChange={(event) => setSettingsDraft({ ...settingsDraft, extractionDestination: event.target.value as Settings["extractionDestination"] })}><option value="ask">Ask every time</option><option value="sibling">Folder beside archive</option><option value="custom">Custom folder</option></select></label>{settingsDraft.extractionDestination === "custom" && <div className="output-picker"><input readOnly value={settingsDraft.customDestination ?? ""} placeholder="Choose a folder" /><button type="button" onClick={chooseCustomDestination}>Choose…</button></div>}</fieldset><fieldset><legend>Completion and browsing</legend><label className="checkbox-label"><input type="checkbox" checked={settingsDraft.revealOnCompletion} onChange={(event) => setSettingsDraft({ ...settingsDraft, revealOnCompletion: event.target.checked })} />Open destination after extraction</label><label className="checkbox-label"><input type="checkbox" checked={settingsDraft.notifications} onChange={(event) => setSettingsDraft({ ...settingsDraft, notifications: event.target.checked })} />Completion notifications</label><label className="checkbox-label"><input type="checkbox" checked={settingsDraft.showHiddenEntries} onChange={(event) => setSettingsDraft({ ...settingsDraft, showHiddenEntries: event.target.checked })} />Show hidden archive entries</label><label>Maximum declared extraction size<select value={settingsDraft.maxExpandedBytes} onChange={(event) => setSettingsDraft({ ...settingsDraft, maxExpandedBytes: Number(event.target.value) })}><option value={1024 ** 3}>1 GiB</option><option value={10 * 1024 ** 3}>10 GiB</option><option value={100 * 1024 ** 3}>100 GiB</option><option value={1024 ** 4}>1 TiB</option></select></label><label>Maximum temporary preview size<select value={settingsDraft.maxPreviewBytes} onChange={(event) => setSettingsDraft({ ...settingsDraft, maxPreviewBytes: Number(event.target.value) })}><option value={10 * 1024 ** 2}>10 MiB</option><option value={100 * 1024 ** 2}>100 MiB</option><option value={500 * 1024 ** 2}>500 MiB</option><option value={1024 ** 3}>1 GiB</option></select></label></fieldset><fieldset><legend>Finder integration</legend><p>{integration?.available && integration.providerRegistered ? `${integration.documentExtensions} archive extensions and ${integration.serviceActions} Finder Services are installed.` : integration ? "Finder integration is unavailable in this build." : "Checking Finder integration…"}</p><p>Service visibility can be enabled or disabled in macOS System Settings → Keyboard → Keyboard Shortcuts → Services.</p><button type="button" onClick={refreshIntegration}>Refresh Status</button></fieldset><fieldset><legend>Privacy and diagnostics</legend><label className="checkbox-label"><input type="checkbox" checked={settingsDraft.historyEnabled} onChange={(event) => setSettingsDraft({ ...settingsDraft, historyEnabled: event.target.checked })} />Remember recent archives on this Mac</label><p>Recent archive paths stay in this user account and are deleted when history is disabled.</p><div className="header-actions"><button type="button" onClick={clearHistory} disabled={!recents.length}>Clear History…</button><button type="button" onClick={exportLocalDiagnostics}>Export Diagnostics…</button><button type="button" className="danger-button" onClick={clearLocalDiagnostics}>Clear Logs…</button></div><p>Local diagnostics contain app, OS, architecture, engine version, operation, and error codes—never passwords, file contents, or entry lists.</p></fieldset><div className="modal-actions"><button type="button" onClick={restoreSettings}>Reset Defaults</button><span className="modal-spacer" /><button type="button" onClick={() => setSettingsOpen(false)}>Cancel</button><button className="primary-button" type="submit">Save Settings</button></div></form></Modal>}
    </main>
  );
}


type IconName =
  | "back" | "forward" | "up" | "sidebar" | "search" | "close"
  | "add" | "test" | "extract" | "more" | "folder" | "archive"
  | "open" | "quickLook" | "disclosureRight" | "disclosureDown"
  | "sortUp" | "sortDown" | "lock";

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
    case "lock": body = <><rect x="5" y="9" width="10" height="7.5" rx="1.5" /><path d="M7 9V6.75a3 3 0 0 1 6 0V9" /></>; break;
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
  return <Modal className="properties-dialog" labelledBy="properties-title" onClose={onClose}><h2 id="properties-title">Entry Properties</h2><dl><dt>Path</dt><dd>{entry.path}</dd><dt>Type</dt><dd>{isSymbolicLink(entry) ? "Link" : entry.isDirectory ? "Folder" : entry.hardLink ? "File (hard link)" : "File"}</dd><dt>Size</dt><dd>{entry.size === null ? "Unknown" : formatBytes(entry.size)}</dd><dt>Compressed</dt><dd>{entry.packedSize === null ? "Unknown" : formatBytes(entry.packedSize)}</dd><dt>Ratio</dt><dd>{formatRatio(entry)}</dd><dt>Modified</dt><dd>{entry.modified ? formatModified(entry.modified, true) : "Unknown"}</dd><dt>Method</dt><dd>{entry.method ?? "Unknown"}</dd><dt>Encrypted</dt><dd>{entry.encrypted ? "Yes" : "No"}</dd></dl><div className="modal-actions"><button autoFocus onClick={onClose}>Close</button></div></Modal>;
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
// Hard links are extracted, opened, and dragged like any other file.
function isSymbolicLink(entry: ArchiveEntry) { return entry.isLink && !entry.hardLink; }
function entryIconKey(entry: ArchiveEntry) {
  const type = entryFileTypeKey(entry);
  return type.startsWith("__") || /^[a-z0-9]{1,16}$/.test(type) ? type : "__file__";
}
function entryFileTypeKey(entry: ArchiveEntry) {
  if (isSymbolicLink(entry)) return "__link__";
  if (entry.isDirectory) return "__folder__";
  const name = leafName(entry.path).toLowerCase();
  const dot = name.lastIndexOf(".");
  return dot > 0 && dot < name.length - 1 ? name.slice(dot + 1) : "__file__";
}
function entryTypeLabel(entry: ArchiveEntry) { return fileTypeLabel(entryFileTypeKey(entry), false); }
function fileTypeLabel(type: string, plural = true) {
  if (type === "__folder__") return plural ? "Folders" : "Folder";
  if (type === "__link__") return plural ? "Links" : "Link";
  if (type === "__file__") return plural ? "Files without extension" : "File";
  return plural ? `${type.toUpperCase()} files` : type.toUpperCase();
}
function EntryIcon({ entry, source }: { entry: ArchiveEntry; source?: string }) {
  if (source) return <img className="entry-icon" src={source} alt="" aria-hidden="true" />;
  const path = entry.isDirectory
    ? "M2.5 5.5h6l1.5 2h7.5v8.5h-15z"
    : isSymbolicLink(entry)
      ? "M7.4 12.6l5.2-5.2m-7.2 7.2l-1 1a2.1 2.1 0 003 3l2.4-2.4a2.1 2.1 0 000-3m4.8-6.8l1-1a2.1 2.1 0 013 3l-2.4 2.4a2.1 2.1 0 01-3 0"
      : "M4 2.5h7l5 5V17.5H4zM11 2.5v5h5";
  return <svg className="entry-icon fallback-icon" viewBox="0 0 20 20" aria-hidden="true"><path d={path} /></svg>;
}
function leafName(path: string) { const parts = path.split(/[\\/]/).filter(Boolean); return parts[parts.length - 1] ?? path; }
function isTypingTarget(target: EventTarget | null) { return target instanceof HTMLInputElement || target instanceof HTMLTextAreaElement || target instanceof HTMLSelectElement || (target instanceof HTMLElement && target.isContentEditable); }
function parentPath(path: string) { const index = Math.max(path.lastIndexOf("/"), path.lastIndexOf("\\")); return index > 0 ? path.slice(0, index) : path; }
function parentEntryPath(path: string) { const index = path.lastIndexOf("/"); return index < 0 ? "" : path.slice(0, index); }
function isHiddenArchivePath(path: string) { return path.split("/").some((part) => part.startsWith(".") && part !== "."); }
function contextMenuPosition(event: { clientX: number; clientY: number }, entry: ArchiveEntry): EntryMenu {
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
async function imageDataUrl(url: string): Promise<string> {
  const blob = await fetch(url).then((response) => response.blob());
  return new Promise((resolve, reject) => {
    const reader = new FileReader();
    reader.onload = () => resolve(String(reader.result));
    reader.onerror = () => reject(reader.error);
    reader.readAsDataURL(blob);
  });
}
// Decimal units, as Finder shows them.
function formatBytes(bytes: number) {
  if (bytes < 1000) return `${bytes.toLocaleString()} ${bytes === 1 ? "byte" : "bytes"}`;
  const units = ["KB", "MB", "GB", "TB"];
  let value = bytes / 1000; let index = 0;
  while (value >= 1000 && index < units.length - 1) { value /= 1000; index += 1; }
  return `${value.toFixed(value >= 100 || index === 0 ? 0 : value >= 10 ? 1 : 2)} ${units[index]}`;
}
// The engine reports local time as "2026-08-30 03:05:52.1116791".
function formatModified(value: string | null, withSeconds = false) {
  if (!value) return "—";
  const match = /^(\d{4})-(\d{2})-(\d{2})[ T](\d{2}):(\d{2}):(\d{2})/.exec(value);
  if (!match) return value;
  const [year, month, day, hour, minute, second] = match.slice(1).map(Number);
  const date = new Date(year, month - 1, day, hour, minute, second);
  if (Number.isNaN(date.getTime())) return value;
  // Joined by hand: the system's combined form ("11 Oct 2026 at 3:31 PM") is too wide for the column.
  return `${date.toLocaleDateString(undefined, { dateStyle: "medium" })}, ${date.toLocaleTimeString(undefined, { timeStyle: withSeconds ? "medium" : "short" })}`;
}
const errorTitles: Record<string, string> = {
  damaged_archive: "The archive is damaged",
  invalid_archive: "Not a supported archive",
  unsupported_method: "Unsupported compression or encryption",
  engine_failed: "The archive engine reported a problem",
  engine_missing: "The archive engine is missing",
  archive_not_found: "The archive could not be found",
  archive_changed: "The archive changed on disk",
  archive_not_writable: "This archive cannot be modified",
  job_busy: "Archi is busy",
  output_exists: "A file with that name already exists",
  volumes_exist: "A split archive with that name already exists",
  entry_exists: "That item is already in the archive",
  entry_type_conflict: "Name already used in the archive",
  entry_not_found: "That item is no longer in the archive",
  only_links: "Nothing to extract",
  unsafe_path: "The archive contains an unsafe file name",
  unsafe_link: "Links cannot be opened",
  unsafe_file_type: "Unsupported kind of file",
  normalization_collision: "The archive contains clashing names",
  path_collision: "The archive contains clashing names",
  conflict: "Files already exist at the destination",
  conflict_type_mismatch: "A file and a folder share a name",
  source_overlap: "The archive is in the way",
  destination_failed: "Could not write to the destination",
  invalid_destination: "The destination is not available",
  staging_failed: "Could not prepare temporary space",
  expansion_limit_exceeded: "Larger than the extraction limit",
  expansion_size_unknown: "Extracted size is unknown",
  preview_size_exceeded: "Too large to open from the archive",
  preview_size_unknown: "Cannot be opened from the archive",
  preview_executable_blocked: "Archi does not open programs or scripts",
  open_failed: "Could not open the item",
  invalid_source: "An item to add is not available",
  no_safe_inputs: "Nothing to add",
  invalid_name: "That name cannot be used",
  rename_failed: "Rename did not complete",
  add_failed: "Adding did not complete",
  replacement_failed: "Could not update the archive",
  finder_request_refused: "Finder request not handled",
  finder_selection_incomplete: "Some archives were not processed",
  clipboard_failed: "Could not copy",
  notifications_denied: "Notifications are turned off",
  settings_invalid: "Settings could not be used",
  settings_unavailable: "Settings could not be read",
};
function errorTitle(code: string) { return errorTitles[code] ?? "Something went wrong"; }
// Space saved. Tiny and encrypted entries can be stored larger than they are; that saves nothing.
function formatRatio(entry: ArchiveEntry) { return !entry.size || entry.packedSize === null ? "—" : `${Math.max(0, Math.round((1 - entry.packedSize / entry.size) * 100))}%`; }
function formatDuration(milliseconds: number) {
  if (milliseconds < 1000) return `${milliseconds} ms`;
  const seconds = Math.floor(milliseconds / 1000);
  return seconds < 60 ? `${seconds}s` : `${Math.floor(seconds / 60)}m ${seconds % 60}s`;
}
export default App;
