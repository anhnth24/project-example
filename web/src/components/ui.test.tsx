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
    fireEvent.keyDown(window, { key: 'Escape' });
    expect(onClose).toHaveBeenCalledOnce();
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
