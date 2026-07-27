// Search + status filter toolbar for the document list (plan P2.4
// §"document list with filter and pagination"). Filtering is client-side
// over the current page only — the API has no filter query params, and
// pagination must round-trip the real `PageInfo` cursor rather than
// inventing offset paging, so filtering can't be folded into the request.
import { useId } from 'react';
import { SelectControl, type SelectOption } from '../ui';
import type { DocumentState } from './types';

export type StatusFilterValue = 'all' | DocumentState;

const STATUS_OPTIONS: SelectOption[] = [
  { value: 'all', label: 'Tất cả trạng thái' },
  { value: 'uploaded', label: 'Đã tải lên' },
  { value: 'converting', label: 'Đang chuyển đổi' },
  { value: 'converted', label: 'Đã chuyển đổi' },
  { value: 'indexing', label: 'Đang lập chỉ mục' },
  { value: 'indexed', label: 'Đã lập chỉ mục' },
  { value: 'failed', label: 'Lỗi chuyển đổi' },
];

export function DocumentFilters({
  searchText,
  onSearchTextChange,
  statusFilter,
  onStatusFilterChange,
}: {
  searchText: string;
  onSearchTextChange: (value: string) => void;
  statusFilter: StatusFilterValue;
  onStatusFilterChange: (value: StatusFilterValue) => void;
}) {
  const searchId = useId();
  return (
    <div
      style={{ display: 'flex', gap: 'var(--space-3)', flexWrap: 'wrap', alignItems: 'flex-end' }}
    >
      <div className="field">
        <label htmlFor={searchId}>Tìm tài liệu</label>
        <input
          id={searchId}
          className="input"
          type="search"
          placeholder="Tìm theo tên tài liệu…"
          value={searchText}
          onChange={(event) => onSearchTextChange(event.target.value)}
        />
      </div>
      <SelectControl
        value={statusFilter}
        options={STATUS_OPTIONS}
        onChange={(value) => onStatusFilterChange(value as StatusFilterValue)}
        ariaLabel="Lọc theo trạng thái"
      />
    </div>
  );
}
