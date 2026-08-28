import { cleanup, fireEvent, render, screen } from "@testing-library/react";
import type { FormEvent } from "react";
import { afterEach, describe, expect, it, vi } from "vitest";
import type { JobSnapshot } from "../types";
import { ExtractDialog, JobShelf, PasswordDialog } from "./OperationPanels";

afterEach(cleanup);

describe("operation panels", () => {
  it("keeps password submission blocked until a password is entered", () => {
    const onPasswordChange = vi.fn();
    const onShowPasswordChange = vi.fn();
    const { rerender } = render(
      <PasswordDialog archiveName="private.zip" error="Wrong password" password="" showPassword={false} busy={false} onPasswordChange={onPasswordChange} onShowPasswordChange={onShowPasswordChange} onClose={vi.fn()} onSubmit={vi.fn()} />,
    );
    expect(screen.getByRole("alert").textContent).toBe("Wrong password");
    expect(screen.getByRole("button", { name: "Continue" })).toHaveProperty("disabled", true);
    fireEvent.change(screen.getByLabelText("Password"), { target: { value: "secret" } });
    fireEvent.click(screen.getByLabelText("Show password"));
    expect(onPasswordChange).toHaveBeenCalledWith("secret");
    expect(onShowPasswordChange).toHaveBeenCalledWith(true);

    rerender(<PasswordDialog archiveName="private.zip" error={null} password="secret" showPassword busy={false} onPasswordChange={onPasswordChange} onShowPasswordChange={onShowPasswordChange} onClose={vi.fn()} onSubmit={vi.fn()} />);
    expect(screen.getByLabelText("Password")).toHaveProperty("type", "text");
    expect(screen.getByRole("button", { name: "Continue" })).toHaveProperty("disabled", false);
  });

  it("surfaces conflicts and forwards extraction confirmation choices", () => {
    const onPolicyChange = vi.fn();
    const onRevealChange = vi.fn();
    const onSubmit = vi.fn((event: FormEvent) => event.preventDefault());
    render(<ExtractDialog selectedCount={2} destination="/tmp/output" error="Files already exist" policy="ask" reveal onChooseDestination={vi.fn()} onPolicyChange={onPolicyChange} onRevealChange={onRevealChange} onClose={vi.fn()} onSubmit={onSubmit} />);
    expect(screen.getByRole("alert").textContent).toBe("Files already exist");
    fireEvent.change(screen.getByLabelText("Existing files"), { target: { value: "keepBoth" } });
    fireEvent.click(screen.getByLabelText("Open destination after extraction"));
    fireEvent.click(screen.getByRole("button", { name: "Extract 2" }));
    expect(onPolicyChange).toHaveBeenCalledWith("keepBoth");
    expect(onRevealChange).toHaveBeenCalledWith(false);
    expect(onSubmit).toHaveBeenCalledOnce();
  });

  it("forwards cancellation only when the job is cancellable", () => {
    const onCancel = vi.fn();
    const job: JobSnapshot = { id: 1, operation: "extract", phase: "running", percent: 25, processedBytes: 25, totalBytes: 100, elapsedMs: 500, bytesPerSecond: 50, currentEntry: null, warningCount: 0, cancellable: true };
    const { rerender } = render(<JobShelf job={job} onCancel={onCancel} />);
    fireEvent.click(screen.getByRole("button", { name: "Cancel" }));
    expect(onCancel).toHaveBeenCalledOnce();
    rerender(<JobShelf job={{ ...job, phase: "finishing", cancellable: false }} onCancel={onCancel} />);
    expect(screen.queryByRole("button", { name: "Cancel" })).toBeNull();
  });
});
