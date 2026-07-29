// Part B of the owner's Q&A redesign spec: "Picker đa dự án trong composer
// (kiểu chọn model của agent chat)" — replaces the old single-select "Phạm
// vi" dropdown with a multi-select chip popover sending `projectIds[]`
// (`AskRequest`/`SearchRequest`, P2-19 — `projectId` singular is deprecated,
// see `contract.ts`'s own doc comment on that field).
//
// Disclosure/focus-management wiring is deliberately the exact same
// `useRailPopover` the rail's own `OrgSwitch` popover already uses (task
// brief: "focus quản lý như OrgSwitch hiện có (đọc pattern)") — trigger
// button with `aria-haspopup="dialog"`/`aria-expanded`, a portalled
// `role="dialog"` panel, outside-pointerdown-closes, Escape-closes-and-
// refocuses-trigger. Native `<input type="checkbox">` per project (not a
// hand-rolled `role="option"` listbox) for the actual multi-select: real
// checkboxes get correct accessible name/state/keyboard behavior for free,
// and there is no drag-reorderable "chip" affordance in this spec that would
// need anything fancier.
import { createPortal } from 'react-dom';
import { ChevronDownIcon } from '../icons';
import { useRailPopover } from '../shell/useRailPopover';

export interface ProjectOption {
  id: string;
  name: string;
}

function scopeLabel(selected: ProjectOption[]): string {
  if (selected.length === 0) return 'Tất cả dự án';
  const names = selected.map((p) => p.name);
  const shown = names.slice(0, 2).join(', ');
  return `${selected.length} dự án: ${shown}${names.length > 2 ? '…' : ''}`;
}

export function ProjectPicker({
  projects,
  selectedProjectIds,
  onChange,
  disabled = false,
}: {
  projects: ProjectOption[];
  selectedProjectIds: string[];
  onChange: (projectIds: string[]) => void;
  disabled?: boolean;
}) {
  const { open, setOpen, triggerRef, menuRef, menuStyle } = useRailPopover(260);
  const selectedSet = new Set(selectedProjectIds);
  const selected = projects.filter((p) => selectedSet.has(p.id));
  const label = 'Phạm vi dự án';

  function toggle(projectId: string) {
    if (selectedSet.has(projectId)) {
      onChange(selectedProjectIds.filter((id) => id !== projectId));
    } else {
      onChange([...selectedProjectIds, projectId]);
    }
  }

  return (
    <div>
      <span className="field-label" id="qa-project-picker-label">
        Dự án
      </span>
      <button
        ref={triggerRef}
        type="button"
        className="ui-select-trigger"
        role="combobox"
        aria-label={label}
        aria-haspopup="dialog"
        aria-expanded={open}
        disabled={disabled}
        onClick={() => setOpen((current) => !current)}
      >
        <span>{scopeLabel(selected)}</span>
        <ChevronDownIcon className="ui-select-chevron" size={15} />
      </button>
      {open &&
        menuStyle &&
        createPortal(
          <div
            ref={menuRef}
            role="dialog"
            aria-label="Chọn dự án"
            className="ui-select-menu"
            style={menuStyle}
          >
            <label
              className="ui-select-option"
              style={{ display: 'flex', alignItems: 'center', gap: 'var(--space-2)' }}
            >
              <input
                type="checkbox"
                checked={selectedProjectIds.length === 0}
                onChange={() => onChange([])}
              />
              <span>Tất cả dự án</span>
            </label>
            {projects.map((project) => (
              <label
                key={project.id}
                className="ui-select-option"
                style={{ display: 'flex', alignItems: 'center', gap: 'var(--space-2)' }}
              >
                <input
                  type="checkbox"
                  checked={selectedSet.has(project.id)}
                  onChange={() => toggle(project.id)}
                />
                <span>{project.name}</span>
              </label>
            ))}
          </div>,
          document.body,
        )}
    </div>
  );
}
