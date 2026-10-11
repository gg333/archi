import type { FormEventHandler } from "react";
import type { ConflictPolicy, JobSnapshot } from "../types";
import { Modal } from "./Modal";

export function JobShelf({ job, onCancel }: { job: JobSnapshot; onCancel: () => void }) {
  // Listing an archive reports no percentage, and some extractions have no total to measure against.
  const indeterminate = job.percent === 0 && (job.operation === "open" || job.totalBytes === 0);
  const title = job.operation === "open" ? "Opening" : capitalize(job.operation);
  return (
    <section className="job-shelf" aria-label={`${job.operation} progress`}>
      <div className="job-heading"><strong>{title} · {capitalize(job.phase)}</strong>{!indeterminate && <span>{job.percent}%</span>}</div>
      {indeterminate ? <progress aria-label="Working" /> : <progress max="100" value={job.percent}>{job.percent}%</progress>}
      <div className="job-details" aria-live="polite">
        <span>{job.currentEntry ?? (indeterminate ? "Reading the archive…" : `${formatBytes(job.processedBytes)} of ${formatBytes(job.totalBytes)}`)}</span>
        <span>{formatDuration(job.elapsedMs)}{indeterminate ? "" : ` · ${formatBytes(job.bytesPerSecond)}/s`}{job.warningCount ? ` · ${job.warningCount} ${job.warningCount === 1 ? "warning" : "warnings"}` : ""}</span>
        {job.cancellable && <button className="cancel-button" onClick={onCancel}>Cancel</button>}
      </div>
    </section>
  );
}

export function ExtractDialog({
  selectedCount,
  destination,
  error,
  policy,
  reveal,
  onChooseDestination,
  onPolicyChange,
  onRevealChange,
  onClose,
  onSubmit,
}: {
  selectedCount: number;
  destination: string;
  error: string | null;
  policy: ConflictPolicy;
  reveal: boolean;
  onChooseDestination: () => void;
  onPolicyChange: (policy: ConflictPolicy) => void;
  onRevealChange: (reveal: boolean) => void;
  onClose: () => void;
  onSubmit: FormEventHandler<HTMLFormElement>;
}) {
  return (
    <Modal className="extract-dialog" labelledBy="extract-title" onClose={onClose}>
      <form className="modal-form" onSubmit={onSubmit}>
        <h2 id="extract-title">Extract {selectedCount ? `${selectedCount} selected` : "all entries"}</h2>
        {error && <p className="inline-error" role="alert">{error}</p>}
        <label>Destination<div className="output-picker"><input readOnly value={destination} /><button type="button" onClick={onChooseDestination}>Choose…</button></div></label>
        <label>Existing files<select value={policy} onChange={(event) => onPolicyChange(event.target.value as ConflictPolicy)}><option value="ask">Ask before changing</option><option value="replace">Replace files</option><option value="skip">Skip files</option><option value="keepBoth">Keep both</option></select></label>
        <label className="checkbox-label"><input type="checkbox" checked={reveal} onChange={(event) => onRevealChange(event.target.checked)} />Open destination after extraction</label>
        <p>Folder structure is preserved. Existing files follow the policy above.</p>
        <div className="modal-actions"><button type="button" onClick={onClose}>Cancel</button><button className="primary-button" type="submit">Extract {selectedCount || "All"}</button></div>
      </form>
    </Modal>
  );
}

export function PasswordDialog({
  archiveName,
  error,
  password,
  showPassword,
  busy,
  onPasswordChange,
  onShowPasswordChange,
  onClose,
  onSubmit,
}: {
  archiveName: string;
  error: string | null;
  password: string;
  showPassword: boolean;
  busy: boolean;
  onPasswordChange: (password: string) => void;
  onShowPasswordChange: (show: boolean) => void;
  onClose: () => void;
  onSubmit: FormEventHandler<HTMLFormElement>;
}) {
  return (
    <Modal className="password-dialog" labelledBy="password-title" onClose={onClose}>
      <form className="modal-form" onSubmit={onSubmit}>
        <h2 id="password-title">Archive password</h2>
        <p>Enter the password for {archiveName}.</p>
        {error && <p className="inline-error" role="alert">{error}</p>}
        <label>Password<input autoFocus type={showPassword ? "text" : "password"} value={password} onChange={(event) => onPasswordChange(event.target.value)} autoComplete="off" /></label>
        <label className="checkbox-label"><input type="checkbox" checked={showPassword} onChange={(event) => onShowPasswordChange(event.target.checked)} />Show password</label>
        <div className="modal-actions"><button type="button" onClick={onClose}>Cancel</button><button className="primary-button" type="submit" disabled={!password || busy}>Continue</button></div>
      </form>
    </Modal>
  );
}

function capitalize(value: string) {
  return value.charAt(0).toUpperCase() + value.slice(1);
}

function formatBytes(bytes: number) {
  if (bytes < 1000) return `${bytes.toLocaleString()} ${bytes === 1 ? "byte" : "bytes"}`;
  const units = ["KB", "MB", "GB", "TB"];
  let value = bytes / 1000;
  let index = 0;
  while (value >= 1000 && index < units.length - 1) { value /= 1000; index += 1; }
  return `${value.toFixed(value >= 100 || index === 0 ? 0 : value >= 10 ? 1 : 2)} ${units[index]}`;
}

function formatDuration(milliseconds: number) {
  if (milliseconds < 1000) return `${milliseconds} ms`;
  const seconds = Math.floor(milliseconds / 1000);
  return seconds < 60 ? `${seconds}s` : `${Math.floor(seconds / 60)}m ${seconds % 60}s`;
}
