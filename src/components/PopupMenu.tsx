import { useEffect, useId, useRef, type KeyboardEvent as ReactKeyboardEvent, type ReactNode } from "react";

export function PopupMenu({
  children,
  disabled = false,
  label,
  open,
  onOpenChange,
  trigger,
  triggerClassName = "toolbar-icon-button",
}: {
  children: ReactNode;
  disabled?: boolean;
  label: string;
  open: boolean;
  onOpenChange: (open: boolean) => void;
  trigger: ReactNode;
  triggerClassName?: string;
}) {
  const id = useId();
  const wrapper = useRef<HTMLDivElement>(null);
  const button = useRef<HTMLButtonElement>(null);
  const menu = useRef<HTMLDivElement>(null);
  const setOpen = useRef(onOpenChange);
  setOpen.current = onOpenChange;

  useEffect(() => {
    if (!open) return;
    menu.current?.querySelector<HTMLElement>("[role='menuitem']")?.focus();
    const close = () => {
      setOpen.current(false);
      button.current?.focus();
    };
    const dismissOutside = (event: PointerEvent) => {
      if (!(event.target instanceof Node) || !wrapper.current?.contains(event.target)) close();
    };
    const dismissWithEscape = (event: KeyboardEvent) => {
      if (event.key !== "Escape") return;
      event.preventDefault();
      event.stopPropagation();
      close();
    };
    document.addEventListener("pointerdown", dismissOutside);
    document.addEventListener("keydown", dismissWithEscape);
    return () => {
      document.removeEventListener("pointerdown", dismissOutside);
      document.removeEventListener("keydown", dismissWithEscape);
    };
  }, [open]);

  function moveFocus(event: ReactKeyboardEvent) {
    if (!menu.current || !["ArrowDown", "ArrowUp", "Home", "End"].includes(event.key)) return;
    event.preventDefault();
    const items = [...menu.current.querySelectorAll<HTMLElement>("[role='menuitem']")];
    if (!items.length) return;
    const current = Math.max(0, items.indexOf(document.activeElement as HTMLElement));
    const next = event.key === "Home" ? 0
      : event.key === "End" ? items.length - 1
        : (current + (event.key === "ArrowDown" ? 1 : -1) + items.length) % items.length;
    items[next].focus();
  }

  return (
    <div ref={wrapper} className="more-menu-wrap">
      <button ref={button} type="button" className={triggerClassName} disabled={disabled} aria-label={label} title={label} aria-haspopup="menu" aria-expanded={open} aria-controls={open ? id : undefined} onClick={() => onOpenChange(!open)}>{trigger}</button>
      {open && <div ref={menu} id={id} className="toolbar-more-menu" role="menu" onKeyDown={moveFocus}>{children}</div>}
    </div>
  );
}
