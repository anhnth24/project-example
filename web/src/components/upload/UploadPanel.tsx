// P2-08 — multipart upload + document/job lifecycle panel
// (plans/markhand-web/phase-2-web-spa.md §P2.4). Self-contained: does not
// mount itself anywhere and does not touch `state/`, `mocks/`, `hooks/`,
// `components/{ui,icons,shell}/`, or `styles.css` — only reads their already
// public exports. The caller decides where this renders (e.g. inside a
// dialog it owns) and receives `onUploaded(documentId)` once each accepted
// file's document is known, whatever its eventual conversion/indexing
// outcome turns out to be.
import { useRef, useState, type ChangeEvent, type DragEvent, type ReactNode } from 'react';
import { UploadCloud } from 'lucide-react';
import { apiClient, type ApiClient } from '../../api/client';
import { UploadItemRow } from './UploadItemRow';

/**
 * Client-side hint only (`accept` never substitutes for server-side
 * validation) — the extension list documented in this repo's own
 * `CLAUDE.md` ("pdf/docx/pptx/xlsx/csv/html/txt + ảnh OCR + audio") plus the
 * common container extensions for those same formats/codecs
 * (xls/xlsb/ods for spreadsheets; htm; png/jpg/jpeg for OCR images; mp3/wav/ogg
 * for audio, matching `audio.rs`'s documented decode support). [Inference]
 * — not independently verified against the server's own accepted-format
 * allowlist.
 */
const ACCEPTED_EXTENSIONS =
  '.pdf,.docx,.pptx,.xlsx,.xls,.xlsb,.ods,.csv,.html,.htm,.txt,.png,.jpg,.jpeg,.wav,.mp3,.ogg';

let localIdSeed = 0;
function nextLocalId(): string {
  localIdSeed += 1;
  return `upload-item-${localIdSeed}`;
}

interface QueuedFile {
  localId: string;
  file: File;
}

export interface UploadPanelProps {
  collectionId: string;
  onUploaded?: (documentId: string) => void;
  /** Injectable for tests; defaults to the app-wide singleton, same convention as `LibraryPage` and `AuthProvider`. */
  client?: ApiClient;
}

export function UploadPanel({
  collectionId,
  onUploaded,
  client = apiClient,
}: UploadPanelProps): ReactNode {
  const [items, setItems] = useState<QueuedFile[]>([]);
  const [isDragging, setIsDragging] = useState(false);
  const inputRef = useRef<HTMLInputElement>(null);

  function addFiles(fileList: FileList | null): void {
    if (!fileList || fileList.length === 0) return;
    const next = Array.from(fileList).map((file) => ({ localId: nextLocalId(), file }));
    setItems((current) => [...current, ...next]);
  }

  function removeItem(localId: string): void {
    setItems((current) => current.filter((item) => item.localId !== localId));
  }

  function handleInputChange(event: ChangeEvent<HTMLInputElement>): void {
    addFiles(event.target.files);
    event.target.value = ''; // allow re-selecting the same file(s) later
  }

  function handleDrop(event: DragEvent<HTMLLabelElement>): void {
    event.preventDefault();
    setIsDragging(false);
    addFiles(event.dataTransfer.files);
  }

  return (
    <div className="upload-panel">
      <label
        className={`upload-dropzone${isDragging ? ' upload-dropzone--dragging' : ''}`}
        onDragOver={(event) => {
          event.preventDefault();
          setIsDragging(true);
        }}
        onDragLeave={() => setIsDragging(false)}
        onDrop={handleDrop}
      >
        <input
          ref={inputRef}
          className="upload-dropzone-input"
          type="file"
          multiple
          accept={ACCEPTED_EXTENSIONS}
          aria-label="Chọn tệp để tải lên"
          onChange={handleInputChange}
        />
        <span className="upload-dropzone-icon" aria-hidden="true">
          <UploadCloud size={22} strokeWidth={2.75} />
        </span>
        <strong>Kéo thả file hoặc bấm để chọn</strong>
        <span>PDF, Word, Excel, PowerPoint, ảnh và audio</span>
      </label>

      {items.length > 0 && (
        <ul className="upload-list">
          {items.map((item) => (
            <li key={item.localId} className="upload-list-item">
              <UploadItemRow
                file={item.file}
                collectionId={collectionId}
                client={client}
                onDocumentId={(documentId) => onUploaded?.(documentId)}
                onRemove={() => removeItem(item.localId)}
              />
            </li>
          ))}
        </ul>
      )}
    </div>
  );
}
