import {
  useEffect,
  useId,
  useLayoutEffect,
  useRef,
  useState,
  type ButtonHTMLAttributes,
  type CSSProperties,
  type KeyboardEvent as ReactKeyboardEvent,
  type ReactNode,
  type RefObject,
} from "react";
import { createPortal } from "react-dom";
import { Check, ChevronDown, LoaderCircle, X } from "lucide-react";

type ButtonVariant = "primary" | "secondary" | "ghost" | "danger";
type ButtonSize = "sm" | "md";

interface ButtonProps extends ButtonHTMLAttributes<HTMLButtonElement> {
  variant?: ButtonVariant;
  size?: ButtonSize;
  icon?: ReactNode;
  loading?: boolean;
}

export function Button({
  variant = "secondary",
  size = "md",
  icon,
  loading = false,
  className = "",
  children,
  disabled,
  ...props
}: ButtonProps) {
  return (
    <button
      type="button"
      className={`ui-button ui-button-${variant} ui-button-${size} ${className}`}
      disabled={disabled || loading}
      {...props}
    >
      {loading ? <LoaderCircle className="spin" size={15} /> : icon}
      {children}
    </button>
  );
}

interface IconButtonProps extends ButtonHTMLAttributes<HTMLButtonElement> {
  label: string;
  active?: boolean;
  badge?: number;
  children: ReactNode;
}

export function IconButton({
  label,
  active = false,
  badge,
  className = "",
  children,
  ...props
}: IconButtonProps) {
  return (
    <button
      type="button"
      aria-label={label}
      title={label}
      className={`ui-icon-button ${active ? "active" : ""} ${className}`}
      {...props}
    >
      {children}
      {!!badge && <span className="ui-icon-badge">{badge > 9 ? "9+" : badge}</span>}
    </button>
  );
}

function useFloatingMenu(
  open: boolean,
  anchorRef: RefObject<HTMLElement | null>,
  minWidth = 200,
) {
  const [style, setStyle] = useState<CSSProperties | null>(null);

  useLayoutEffect(() => {
    if (!open) {
      setStyle(null);
      return;
    }
    const update = () => {
      const anchor = anchorRef.current;
      if (!anchor) return;
      const rect = anchor.getBoundingClientRect();
      const menuWidth = Math.min(
        Math.max(rect.width, minWidth),
        window.innerWidth - 16,
      );
      const spaceBelow = window.innerHeight - rect.bottom - 12;
      const spaceAbove = rect.top - 12;
      const opensAbove = spaceBelow < 160 && spaceAbove > spaceBelow;
      const available = opensAbove ? spaceAbove : spaceBelow;
      const left = Math.min(
        Math.max(8, rect.left),
        window.innerWidth - menuWidth - 8,
      );
      setStyle({
        left,
        width: menuWidth,
        maxHeight: Math.min(280, Math.max(96, available)),
        ...(opensAbove
          ? { bottom: window.innerHeight - rect.top + 6 }
          : { top: rect.bottom + 6 }),
      });
    };
    update();
    window.addEventListener("resize", update);
    window.addEventListener("scroll", update, true);
    return () => {
      window.removeEventListener("resize", update);
      window.removeEventListener("scroll", update, true);
    };
  }, [anchorRef, minWidth, open]);

  return style;
}

export interface SelectOption {
  value: string;
  label: string;
  disabled?: boolean;
}

export function SelectControl({
  value,
  options,
  onChange,
  ariaLabel,
  placeholder = "Chọn một mục",
  disabled = false,
  compact = false,
}: {
  value: string;
  options: SelectOption[];
  onChange: (value: string) => void;
  ariaLabel: string;
  placeholder?: string;
  disabled?: boolean;
  compact?: boolean;
}) {
  const listId = useId();
  const rootRef = useRef<HTMLDivElement>(null);
  const buttonRef = useRef<HTMLButtonElement>(null);
  const menuRef = useRef<HTMLDivElement>(null);
  const selectedIndex = options.findIndex((option) => option.value === value);
  const [open, setOpen] = useState(false);
  const [activeIndex, setActiveIndex] = useState(Math.max(0, selectedIndex));
  const menuStyle = useFloatingMenu(open, buttonRef, compact ? 220 : 240);
  const selected = selectedIndex >= 0 ? options[selectedIndex] : null;

  useEffect(() => {
    if (!open) return;
    const closeOnOutsideClick = (event: PointerEvent) => {
      const target = event.target as Node;
      if (
        !rootRef.current?.contains(target) &&
        !menuRef.current?.contains(target)
      ) {
        setOpen(false);
      }
    };
    document.addEventListener("pointerdown", closeOnOutsideClick);
    return () => document.removeEventListener("pointerdown", closeOnOutsideClick);
  }, [open]);

  useEffect(() => {
    if (selectedIndex >= 0) setActiveIndex(selectedIndex);
  }, [selectedIndex]);

  function moveActive(direction: 1 | -1) {
    if (!options.length) return;
    let next = activeIndex;
    for (let count = 0; count < options.length; count += 1) {
      next = (next + direction + options.length) % options.length;
      if (!options[next].disabled) {
        setActiveIndex(next);
        return;
      }
    }
  }

  function choose(index: number) {
    const option = options[index];
    if (!option || option.disabled) return;
    onChange(option.value);
    setActiveIndex(index);
    setOpen(false);
    buttonRef.current?.focus();
  }

  function handleKeyDown(event: ReactKeyboardEvent<HTMLButtonElement>) {
    if (event.key === "ArrowDown" || event.key === "ArrowUp") {
      event.preventDefault();
      if (!open) {
        setOpen(true);
        setActiveIndex(Math.max(0, selectedIndex));
      } else {
        moveActive(event.key === "ArrowDown" ? 1 : -1);
      }
    } else if (event.key === "Enter" || event.key === " ") {
      event.preventDefault();
      if (open) choose(activeIndex);
      else setOpen(true);
    } else if (event.key === "Escape" && open) {
      event.preventDefault();
      setOpen(false);
    } else if (event.key === "Home" && open) {
      event.preventDefault();
      const first = options.findIndex((option) => !option.disabled);
      setActiveIndex(Math.max(0, first));
    } else if (event.key === "End" && open) {
      event.preventDefault();
      const reversed = [...options].reverse();
      const offset = reversed.findIndex((option) => !option.disabled);
      setActiveIndex(offset < 0 ? 0 : options.length - 1 - offset);
    }
  }

  return (
    <div
      ref={rootRef}
      className={`ui-select ${compact ? "ui-select-compact" : ""}`}
    >
      <button
        ref={buttonRef}
        type="button"
        className="ui-select-trigger"
        role="combobox"
        aria-label={ariaLabel}
        aria-expanded={open}
        aria-controls={listId}
        aria-haspopup="listbox"
        aria-activedescendant={
          open && options[activeIndex]
            ? `${listId}-option-${activeIndex}`
            : undefined
        }
        disabled={disabled}
        onClick={() => setOpen((current) => !current)}
        onKeyDown={handleKeyDown}
      >
        <span className={selected ? "" : "placeholder"}>
          {selected?.label ?? placeholder}
        </span>
        <ChevronDown
          className="ui-select-chevron"
          size={compact ? 13 : 15}
          aria-hidden="true"
        />
      </button>
      {open &&
        menuStyle &&
        createPortal(
          <div
            ref={menuRef}
            id={listId}
            className="ui-select-menu"
            role="listbox"
            aria-label={ariaLabel}
            style={menuStyle}
          >
            {options.map((option, index) => (
              <button
                type="button"
                role="option"
                id={`${listId}-option-${index}`}
                className={`ui-select-option ${
                  index === activeIndex ? "active" : ""
                }`}
                aria-selected={option.value === value}
                disabled={option.disabled}
                key={option.value}
                onMouseEnter={() => setActiveIndex(index)}
                onMouseDown={(event) => event.preventDefault()}
                onClick={() => choose(index)}
              >
                <span>{option.label}</span>
                {option.value === value && <Check size={14} aria-hidden="true" />}
              </button>
            ))}
          </div>,
          document.body,
        )}
    </div>
  );
}

function foldForSearch(value: string): string {
  return value
    .normalize("NFD")
    .replace(/\p{Diacritic}/gu, "")
    .toLocaleLowerCase();
}

export function Combobox({
  value,
  options,
  onChange,
  ariaLabel,
  placeholder,
}: {
  value: string;
  options: string[];
  onChange: (value: string) => void;
  ariaLabel: string;
  placeholder?: string;
}) {
  const listId = useId();
  const rootRef = useRef<HTMLDivElement>(null);
  const inputRef = useRef<HTMLInputElement>(null);
  const menuRef = useRef<HTMLDivElement>(null);
  const [open, setOpen] = useState(false);
  const [activeIndex, setActiveIndex] = useState(0);
  const foldedValue = foldForSearch(value);
  const filtered = options.filter((option) =>
    foldForSearch(option).includes(foldedValue),
  );
  const menuStyle = useFloatingMenu(
    open && filtered.length > 0,
    rootRef,
    240,
  );

  useEffect(() => {
    if (!open) return;
    const closeOnOutsideClick = (event: PointerEvent) => {
      const target = event.target as Node;
      if (
        !rootRef.current?.contains(target) &&
        !menuRef.current?.contains(target)
      ) {
        setOpen(false);
      }
    };
    document.addEventListener("pointerdown", closeOnOutsideClick);
    return () => document.removeEventListener("pointerdown", closeOnOutsideClick);
  }, [open]);

  function choose(option: string) {
    onChange(option);
    setOpen(false);
    inputRef.current?.focus();
  }

  function handleKeyDown(event: ReactKeyboardEvent<HTMLInputElement>) {
    if (event.key === "ArrowDown" || event.key === "ArrowUp") {
      event.preventDefault();
      if (!filtered.length) return;
      if (!open) {
        setOpen(true);
        setActiveIndex(0);
      } else {
        const direction = event.key === "ArrowDown" ? 1 : -1;
        setActiveIndex(
          (current) =>
            (current + direction + filtered.length) % filtered.length,
        );
      }
    } else if (event.key === "Enter" && open && filtered[activeIndex]) {
      event.preventDefault();
      choose(filtered[activeIndex]);
    } else if (event.key === "Escape" && open) {
      event.preventDefault();
      setOpen(false);
    }
  }

  return (
    <div ref={rootRef} className="ui-combobox">
      <input
        ref={inputRef}
        value={value}
        placeholder={placeholder}
        role="combobox"
        aria-label={ariaLabel}
        aria-autocomplete="list"
        aria-expanded={open && filtered.length > 0}
        aria-controls={listId}
        aria-activedescendant={
          open && filtered[activeIndex]
            ? `${listId}-option-${activeIndex}`
            : undefined
        }
        onFocus={() => {
          setActiveIndex(0);
          setOpen(true);
        }}
        onChange={(event) => {
          onChange(event.target.value);
          setActiveIndex(0);
          setOpen(true);
        }}
        onKeyDown={handleKeyDown}
      />
      <button
        type="button"
        className="ui-combobox-toggle"
        aria-label={open ? "Đóng gợi ý" : "Mở gợi ý"}
        tabIndex={-1}
        onMouseDown={(event) => {
          event.preventDefault();
          if (open) setOpen(false);
          else {
            inputRef.current?.focus();
            setOpen(true);
          }
        }}
      >
        <ChevronDown
          className="ui-select-chevron"
          size={15}
          aria-hidden="true"
        />
      </button>
      {open &&
        menuStyle &&
        createPortal(
          <div
            ref={menuRef}
            id={listId}
            className="ui-select-menu"
            role="listbox"
            aria-label={`${ariaLabel} gợi ý`}
            style={menuStyle}
          >
            {filtered.map((option, index) => (
              <button
                type="button"
                role="option"
                id={`${listId}-option-${index}`}
                className={`ui-select-option ${
                  index === activeIndex ? "active" : ""
                }`}
                aria-selected={option === value}
                key={option}
                onMouseEnter={() => setActiveIndex(index)}
                onMouseDown={(event) => event.preventDefault()}
                onClick={() => choose(option)}
              >
                <span>{option}</span>
                {option === value && <Check size={14} aria-hidden="true" />}
              </button>
            ))}
          </div>,
          document.body,
        )}
    </div>
  );
}

export function Modal({
  title,
  description,
  children,
  footer,
  onClose,
  width = 480,
}: {
  title: string;
  description?: string;
  children: ReactNode;
  footer?: ReactNode;
  onClose: () => void;
  width?: number;
}) {
  const titleId = useId();
  const panelRef = useRef<HTMLDivElement>(null);
  const onCloseRef = useRef(onClose);
  onCloseRef.current = onClose;

  useEffect(() => {
    const previous = document.activeElement as HTMLElement | null;
    const first =
      panelRef.current?.querySelector<HTMLElement>("[autofocus]") ??
      panelRef.current?.querySelector<HTMLElement>(
        "input, button, textarea, select, [tabindex]:not([tabindex='-1'])",
      );
    first?.focus();
    const onKey = (event: KeyboardEvent) => {
      if (event.key === "Escape") {
        event.preventDefault();
        onCloseRef.current();
      } else if (event.key === "Tab") {
        const focusable = panelRef.current?.querySelectorAll<HTMLElement>(
          "input:not(:disabled), button:not(:disabled), textarea:not(:disabled), select:not(:disabled), [tabindex]:not([tabindex='-1'])",
        );
        if (!focusable?.length) return;
        const firstElement = focusable[0];
        const lastElement = focusable[focusable.length - 1];
        if (event.shiftKey && document.activeElement === firstElement) {
          event.preventDefault();
          lastElement.focus();
        } else if (!event.shiftKey && document.activeElement === lastElement) {
          event.preventDefault();
          firstElement.focus();
        }
      }
    };
    window.addEventListener("keydown", onKey);
    return () => {
      window.removeEventListener("keydown", onKey);
      previous?.focus();
    };
  }, []);

  return (
    <div
      className="modal-backdrop"
      onMouseDown={(event) => event.target === event.currentTarget && onClose()}
    >
      <div
        ref={panelRef}
        className="modal-panel"
        role="dialog"
        aria-modal="true"
        aria-labelledby={titleId}
        style={{ width }}
      >
        <header className="modal-header">
          <div>
            <h2 id={titleId}>{title}</h2>
            {description && <p>{description}</p>}
          </div>
          <IconButton label="Đóng" onClick={onClose}>
            <X size={15} />
          </IconButton>
        </header>
        <div className="modal-content">{children}</div>
        {footer && <footer className="modal-footer">{footer}</footer>}
      </div>
    </div>
  );
}

export function Toggle({
  checked,
  onChange,
  label,
  description,
}: {
  checked: boolean;
  onChange: (checked: boolean) => void;
  label: string;
  description?: string;
}) {
  return (
    <label className="toggle-row">
      <span className="toggle-copy">
        <span>{label}</span>
        {description && <small>{description}</small>}
      </span>
      <input
        type="checkbox"
        checked={checked}
        onChange={(event) => onChange(event.target.checked)}
      />
      <span className="toggle-track" aria-hidden="true">
        <span />
      </span>
    </label>
  );
}

export function Notice({
  tone,
  children,
  action,
}: {
  tone: "warning" | "error" | "info";
  children: ReactNode;
  action?: ReactNode;
}) {
  return (
    <div className={`notice notice-${tone}`} role={tone === "error" ? "alert" : "status"}>
      <span>{children}</span>
      {action}
    </div>
  );
}
