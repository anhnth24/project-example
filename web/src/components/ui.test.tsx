import { cleanup, fireEvent, render, screen } from '@testing-library/react';
import { useState } from 'react';
import { afterEach, describe, expect, it, vi } from 'vitest';
import { Button, IconButton, Modal, Notice, SelectControl, Toggle } from './ui';

afterEach(() => {
  cleanup();
});

describe('Button', () => {
  it('renders a spinner and disables itself while loading', () => {
    render(
      <Button loading variant="primary">
        Lưu
      </Button>,
    );
    const button = screen.getByRole('button', { name: 'Lưu' });
    expect(button).toBeDisabled();
    expect(button.querySelector('svg')).not.toBeNull();
  });

  it('fires onClick when enabled', () => {
    const onClick = vi.fn();
    render(<Button onClick={onClick}>Tiếp tục</Button>);
    fireEvent.click(screen.getByRole('button', { name: 'Tiếp tục' }));
    expect(onClick).toHaveBeenCalledOnce();
  });
});

describe('IconButton', () => {
  it('exposes an accessible name and a 9+ badge cap', () => {
    render(
      <IconButton label="Thông báo" badge={42}>
        <span />
      </IconButton>,
    );
    const button = screen.getByRole('button', { name: 'Thông báo' });
    expect(button).toHaveTextContent('9+');
  });
});

describe('Toggle', () => {
  it('reports the new checked state without mutating props', () => {
    const onChange = vi.fn();
    render(<Toggle checked={false} onChange={onChange} label="Bật OCR" />);
    fireEvent.click(screen.getByRole('checkbox', { name: 'Bật OCR' }));
    expect(onChange).toHaveBeenCalledWith(true);
  });
});

describe('Notice', () => {
  it('uses role=alert for error tone and role=status otherwise', () => {
    const { rerender } = render(<Notice tone="error">Lỗi tải lên</Notice>);
    expect(screen.getByRole('alert')).toHaveTextContent('Lỗi tải lên');
    rerender(<Notice tone="info">Đang xử lý</Notice>);
    expect(screen.getByRole('status')).toHaveTextContent('Đang xử lý');
  });
});

describe('SelectControl', () => {
  const options = [
    { value: 'owner', label: 'Chủ sở hữu' },
    { value: 'viewer', label: 'Người xem' },
  ];

  function ControlledSelect() {
    const [value, setValue] = useState('viewer');
    return (
      <SelectControl value={value} options={options} onChange={setValue} ariaLabel="Vai trò" />
    );
  }

  it('opens on click and selects an option', () => {
    render(<ControlledSelect />);
    const trigger = screen.getByRole('combobox', { name: 'Vai trò' });
    expect(trigger).toHaveTextContent('Người xem');
    fireEvent.click(trigger);
    fireEvent.click(screen.getByRole('option', { name: /Chủ sở hữu/ }));
    expect(trigger).toHaveTextContent('Chủ sở hữu');
  });

  // P2-14 (plans/markhand-web/phase-2-web-spa.md §P2.7): "keyboard-operable
  // … search" — this is the status filter's underlying widget
  // (DocumentFilters.tsx). Nothing in this suite previously drove it by
  // keyboard at all (the only test above is mouse-only), so the component's
  // own hand-rolled ArrowDown/Enter/Escape handling (`handleKeyDown` in
  // ui.tsx) had no regression coverage.
  it('opens with ArrowDown, moves with arrow keys, selects with Enter, and Escape closes without changing the value', () => {
    render(<ControlledSelect />);
    const trigger = screen.getByRole('combobox', { name: 'Vai trò' });
    trigger.focus();
    expect(document.activeElement).toBe(trigger);

    fireEvent.keyDown(trigger, { key: 'ArrowDown' });
    expect(trigger).toHaveAttribute('aria-expanded', 'true');

    fireEvent.keyDown(trigger, { key: 'ArrowUp' });
    fireEvent.keyDown(trigger, { key: 'Enter' });
    expect(trigger).toHaveTextContent('Chủ sở hữu');
    expect(trigger).toHaveAttribute('aria-expanded', 'false');

    fireEvent.keyDown(trigger, { key: 'ArrowDown' });
    expect(trigger).toHaveAttribute('aria-expanded', 'true');
    fireEvent.keyDown(trigger, { key: 'Escape' });
    expect(trigger).toHaveAttribute('aria-expanded', 'false');
    // Escape only closes the popup; it must not also change the value.
    expect(trigger).toHaveTextContent('Chủ sở hữu');
  });
});

describe('Modal', () => {
  it('focuses the first focusable element and closes on Escape', () => {
    const onClose = vi.fn();
    render(
      <Modal title="Xoá tài liệu" onClose={onClose}>
        <button type="button">Xác nhận</button>
      </Modal>,
    );
    expect(screen.getByRole('dialog', { name: 'Xoá tài liệu' })).toBeVisible();
    // The title above already claimed this ("focuses the first focusable
    // element…") but nothing below it ever checked `activeElement` — P2-14
    // (plans/markhand-web/phase-2-web-spa.md §P2.7, "focus sau … modal
    // open/close") needs that actually proven, not just asserted in prose.
    //
    // The actually-first focusable element in DOM order is the header's own
    // close (Đóng) icon-button — it renders before `children` inside
    // `.dialog-header` (see ui.tsx's Modal markup) — not whatever the caller
    // passed as body content. Documented here as the real, observed
    // behaviour rather than assumed: useFocusTrap's querySelector runs in
    // document order and has no special case for "past the header".
    expect(document.activeElement).toBe(screen.getByRole('button', { name: 'Đóng' }));
    fireEvent.keyDown(window, { key: 'Escape' });
    expect(onClose).toHaveBeenCalledOnce();
  });

  it('restores focus to the triggering element once the modal unmounts', () => {
    function Harness() {
      const [open, setOpen] = useState(false);
      return (
        <>
          <button type="button" onClick={() => setOpen(true)}>
            Mở hộp thoại
          </button>
          {open && (
            <Modal title="Xoá tài liệu" onClose={() => setOpen(false)}>
              <button type="button">Xác nhận</button>
            </Modal>
          )}
        </>
      );
    }
    render(<Harness />);
    const trigger = screen.getByRole('button', { name: 'Mở hộp thoại' });
    trigger.focus();
    expect(document.activeElement).toBe(trigger);

    fireEvent.click(trigger);
    expect(document.activeElement).toBe(screen.getByRole('button', { name: 'Đóng' }));

    fireEvent.keyDown(window, { key: 'Escape' });
    expect(screen.queryByRole('dialog')).not.toBeInTheDocument();
    expect(document.activeElement).toBe(trigger);
  });

  it('closes when the backdrop is clicked but not when the panel is clicked', () => {
    const onClose = vi.fn();
    render(
      <Modal title="Xoá tài liệu" onClose={onClose}>
        <button type="button">Xác nhận</button>
      </Modal>,
    );
    fireEvent.mouseDown(screen.getByRole('button', { name: 'Xác nhận' }));
    expect(onClose).not.toHaveBeenCalled();
    fireEvent.mouseDown(screen.getByRole('dialog').parentElement!);
    expect(onClose).toHaveBeenCalledOnce();
  });
});
