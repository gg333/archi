import { cleanup, fireEvent, render, screen } from "@testing-library/react";
import { useState } from "react";
import { afterEach, describe, expect, it } from "vitest";
import { PopupMenu } from "./PopupMenu";

afterEach(cleanup);

function MenuHarness() {
  const [open, setOpen] = useState(false);
  return <PopupMenu label="More actions" open={open} onOpenChange={setOpen} trigger="•••"><button role="menuitem">Settings</button><button role="menuitem">Delete</button></PopupMenu>;
}

describe("popup menu", () => {
  it("focuses its first action and dismisses on Escape or outside click", () => {
    render(<MenuHarness />);
    const trigger = screen.getByRole("button", { name: "More actions" });

    fireEvent.click(trigger);
    expect(trigger.getAttribute("aria-expanded")).toBe("true");
    expect(document.activeElement).toBe(screen.getByRole("menuitem", { name: "Settings" }));

    fireEvent.keyDown(document, { key: "Escape" });
    expect(screen.queryByRole("menu")).toBeNull();
    expect(document.activeElement).toBe(trigger);

    fireEvent.click(trigger);
    fireEvent.pointerDown(document.body);
    expect(screen.queryByRole("menu")).toBeNull();
    expect(document.activeElement).toBe(trigger);
  });
});
