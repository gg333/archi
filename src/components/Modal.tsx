import { useEffect, useRef, type ReactNode } from "react";

export function Modal({
  children,
  className = "",
  labelledBy,
  onClose,
}: {
  children: ReactNode;
  className?: string;
  labelledBy: string;
  onClose: () => void;
}) {
  const dialog = useRef<HTMLDialogElement>(null);
  const close = useRef(onClose);
  close.current = onClose;

  useEffect(() => {
    const previous = document.activeElement instanceof HTMLElement ? document.activeElement : null;
    dialog.current?.showModal();
    return () => {
      dialog.current?.close();
      previous?.focus();
    };
  }, []);

  return (
    <dialog
      ref={dialog}
      className={`modal-card ${className}`}
      aria-labelledby={labelledBy}
      onCancel={(event) => {
        event.preventDefault();
        close.current();
      }}
    >
      {children}
    </dialog>
  );
}
