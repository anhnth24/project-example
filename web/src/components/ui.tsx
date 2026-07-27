// Ported from app/src/components/ui.tsx (desktop, 600 lines). All of that
// file was already generic React (no Tauri IPC, no native dialogs, no
// filesystem/local paths), so nothing needed to be dropped on that front.
// Icons come from `./icons`, which now wraps `lucide-react` (see the note
// there). `useFloatingMenu` and the Modal focus trap were extracted into
// `../hooks` so this file only exports components.
//
// Organic re-skin: class names below now come from styles.css's Organic
// component layer (`.btn*`, `.dialog*`, etc.) instead of the old
// `.ui-button`/`.modal-*` set — see styles.css for the token mapping and
// contrast notes. Props, behaviour and accessibility semantics (roles,
// labels, keyboard handling) are unchanged.
import {
  useEffect,
  useId,
  useRef,
  useState,
  type ButtonHTMLAttributes,
  type KeyboardEvent as ReactKeyboardEvent,
  type ReactNode,
} from 'react';
import { createPortal } from 'react-dom';
import { useFloatingMenu } from '../hooks/useFloatingMenu';
import { useFocusTrap } from '../hooks/useFocusTrap';
import { CheckIcon, ChevronDownIcon, CloseIcon, SpinnerIcon } from './icons';

type ButtonVariant = 'primary' | 'secondary' | 'ghost' | 'danger';
type ButtonSize = 'sm' | 'md';

interface ButtonProps extends ButtonHTMLAttributes<HTMLButtonElement> {
  variant?: ButtonVariant;
  size?: ButtonSize;
  icon?: ReactNode;
  loading?: boolean;
}

export function Button({
  variant = 'secondary',
  size = 'md',
  icon,
  loading = false,
  className = '',
  children,
  disabled,
  ...props
}: ButtonProps) {
  return (
    <button
      type="button"
      className={`btn btn-${variant} ${size === 'sm' ? 'btn-sm' : ''} ${className}`}
      disabled={disabled || loading}
      {...props}
    >
      {loading ? <SpinnerIcon className="spin" size={15} /> : icon}
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
  className = '',
  children,
  ...props
}: IconButtonProps) {
  return (
    <button
      type="button"
      aria-label={label}
      title={label}
      className={`btn btn-icon ${active ? 'active' : ''} ${className}`}
      {...props}
    >
      {children}
      {!!badge && <span className="icon-badge">{badge > 9 ? '9+' : badge}</span>}
    </button>
  );
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
  placeholder = 'Chọn một mục',
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
  const [priorSelectedIndex, setPriorSelectedIndex] = useState(selectedIndex);
  const menuStyle = useFloatingMenu(open, buttonRef, compact ? 220 : 240);
  const selected = selectedIndex >= 0 ? options[selectedIndex] : null;

  // Keep the keyboard-active option in sync with the selected value without
  // an effect (see https://react.dev/learn/you-might-not-need-an-effect) —
  // adjusting state while rendering is the documented escape hatch for this.
  if (selectedIndex !== priorSelectedIndex) {
    setPriorSelectedIndex(selectedIndex);
    if (selectedIndex >= 0) {
      setActiveIndex(selectedIndex);
    }
  }

  useEffect(() => {
    if (!open) return;
    const closeOnOutsideClick = (event: PointerEvent) => {
      const target = event.target as Node;
      if (!rootRef.current?.contains(target) && !menuRef.current?.contains(target)) {
        setOpen(false);
      }
    };
    document.addEventListener('pointerdown', closeOnOutsideClick);
    return () => document.removeEventListener('pointerdown', closeOnOutsideClick);
  }, [open]);

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
    if (event.key === 'ArrowDown' || event.key === 'ArrowUp') {
      event.preventDefault();
      if (!open) {
        setOpen(true);
        setActiveIndex(Math.max(0, selectedIndex));
      } else {
        moveActive(event.key === 'ArrowDown' ? 1 : -1);
      }
    } else if (event.key === 'Enter' || event.key === ' ') {
      event.preventDefault();
      if (open) choose(activeIndex);
      else setOpen(true);
    } else if (event.key === 'Escape' && open) {
      event.preventDefault();
      setOpen(false);
    } else if (event.key === 'Home' && open) {
      event.preventDefault();
      const first = options.findIndex((option) => !option.disabled);
      setActiveIndex(Math.max(0, first));
    } else if (event.key === 'End' && open) {
      event.preventDefault();
      const reversed = [...options].reverse();
      const offset = reversed.findIndex((option) => !option.disabled);
      setActiveIndex(offset < 0 ? 0 : options.length - 1 - offset);
    }
  }

  return (
    <div ref={rootRef} className={`ui-select ${compact ? 'ui-select-compact' : ''}`}>
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
          open && options[activeIndex] ? `${listId}-option-${activeIndex}` : undefined
        }
        disabled={disabled}
        onClick={() => setOpen((current) => !current)}
        onKeyDown={handleKeyDown}
      >
        <span className={selected ? '' : 'placeholder'}>{selected?.label ?? placeholder}</span>
        <ChevronDownIcon className="ui-select-chevron" size={compact ? 13 : 15} />
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
                className={`ui-select-option ${index === activeIndex ? 'active' : ''}`}
                aria-selected={option.value === value}
                disabled={option.disabled}
                key={option.value}
                onMouseEnter={() => setActiveIndex(index)}
                onMouseDown={(event) => event.preventDefault()}
                onClick={() => choose(index)}
              >
                <span>{option.label}</span>
                {option.value === value && <CheckIcon size={14} />}
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
    .normalize('NFD')
    .replace(/\p{Diacritic}/gu, '')
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
  const [showAll, setShowAll] = useState(false);
  const foldedValue = foldForSearch(value);
  const filtered = showAll
    ? options
    : options.filter((option) => foldForSearch(option).includes(foldedValue));
  const menuStyle = useFloatingMenu(open && filtered.length > 0, rootRef, 240);

  useEffect(() => {
    if (!open) return;
    const closeOnOutsideClick = (event: PointerEvent) => {
      const target = event.target as Node;
      if (!rootRef.current?.contains(target) && !menuRef.current?.contains(target)) {
        setOpen(false);
      }
    };
    document.addEventListener('pointerdown', closeOnOutsideClick);
    return () => document.removeEventListener('pointerdown', closeOnOutsideClick);
  }, [open]);

  function choose(option: string) {
    onChange(option);
    setShowAll(false);
    setOpen(false);
    inputRef.current?.focus();
  }

  function handleKeyDown(event: ReactKeyboardEvent<HTMLInputElement>) {
    if (event.key === 'ArrowDown' || event.key === 'ArrowUp') {
      event.preventDefault();
      if (!filtered.length) return;
      if (!open) {
        setShowAll(true);
        setOpen(true);
        setActiveIndex(0);
      } else {
        const direction = event.key === 'ArrowDown' ? 1 : -1;
        setActiveIndex((current) => (current + direction + filtered.length) % filtered.length);
      }
    } else if (event.key === 'Enter' && open && filtered[activeIndex]) {
      event.preventDefault();
      choose(filtered[activeIndex]);
    } else if (event.key === 'Escape' && open) {
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
          open && filtered[activeIndex] ? `${listId}-option-${activeIndex}` : undefined
        }
        onFocus={() => {
          setActiveIndex(0);
          setShowAll(true);
          setOpen(true);
        }}
        onClick={() => {
          setShowAll(true);
          setOpen(true);
        }}
        onChange={(event) => {
          onChange(event.target.value);
          setActiveIndex(0);
          setShowAll(false);
          setOpen(true);
        }}
        onKeyDown={handleKeyDown}
      />
      <button
        type="button"
        className="ui-combobox-toggle"
        aria-label={open ? 'Đóng gợi ý' : 'Mở gợi ý'}
        tabIndex={-1}
        onMouseDown={(event) => {
          event.preventDefault();
          if (open) setOpen(false);
          else {
            setShowAll(true);
            inputRef.current?.focus();
            setOpen(true);
          }
        }}
      >
        <ChevronDownIcon className="ui-select-chevron" size={15} />
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
                className={`ui-select-option ${index === activeIndex ? 'active' : ''}`}
                aria-selected={option === value}
                key={option}
                onMouseEnter={() => setActiveIndex(index)}
                onMouseDown={(event) => event.preventDefault()}
                onClick={() => choose(option)}
              >
                <span>{option}</span>
                {option === value && <CheckIcon size={14} />}
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
  useFocusTrap(panelRef, onClose);

  return (
    <div
      className="dialog-backdrop"
      onMouseDown={(event) => event.target === event.currentTarget && onClose()}
    >
      <div
        ref={panelRef}
        className="dialog"
        role="dialog"
        aria-modal="true"
        aria-labelledby={titleId}
        style={{ width }}
      >
        <header className="dialog-header">
          <div>
            <h2 id={titleId} className="dialog-title">
              {title}
            </h2>
            {description && <p className="dialog-body">{description}</p>}
          </div>
          <IconButton label="Đóng" onClick={onClose}>
            <CloseIcon size={15} />
          </IconButton>
        </header>
        <div className="dialog-content">{children}</div>
        {footer && <footer className="dialog-actions">{footer}</footer>}
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
  tone: 'warning' | 'error' | 'info';
  children: ReactNode;
  action?: ReactNode;
}) {
  return (
    <div className={`notice notice-${tone}`} role={tone === 'error' ? 'alert' : 'status'}>
      <span>{children}</span>
      {action}
    </div>
  );
}
