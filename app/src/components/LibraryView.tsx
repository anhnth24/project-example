import { useMemo, useState } from "react";
import { CheckSquare, FileCheck2, RefreshCw, Upload } from "lucide-react";
import { fileIcon } from "../lib/icons";
import { flattenFiles, folderLabel } from "../lib/tree";
import { useStore } from "../state/store";
import { Button } from "./ui";

type Filter = "all" | "raw" | "done";

export function LibraryView({ onUpload }: { onUpload: () => void }) {
  const tree = useStore((state) => state.tree);
  const openNode = useStore((state) => state.openNode);
  const enqueueConversions = useStore((state) => state.enqueueConversions);
  const jobs = useStore((state) => state.jobs);
  const [filter, setFilter] = useState<Filter>("all");
  const [selected, setSelected] = useState<string[]>([]);

  const files = useMemo(() => flattenFiles(tree), [tree]);
  const visible = files.filter((node) => {
    const raw = node.supported && !node.mdRelPath;
    if (filter === "raw") return raw;
    if (filter === "done") return !!node.mdRelPath || node.standaloneMd;
    return true;
  });
  const selectedRaw = selected.filter((relPath) => {
    const node = files.find((file) => file.relPath === relPath);
    return !!node?.supported && !node.mdRelPath;
  });

  function toggle(relPath: string) {
    setSelected((current) =>
      current.includes(relPath)
        ? current.filter((item) => item !== relPath)
        : [...current, relPath],
    );
  }

  return (
    <section className="library-view">
      <header className="library-header">
        <div>
          <span className="eyebrow">DATA workspace</span>
          <h1>Thư viện tài liệu</h1>
          <p>Tìm, kiểm tra trạng thái và convert nhiều tài liệu trong một lượt.</p>
        </div>
        <Button variant="primary" icon={<Upload size={15} />} onClick={onUpload}>
          Tải file
        </Button>
      </header>

      <div className="library-toolbar">
        <div className="filter-pills" aria-label="Lọc thư viện">
          {(
            [
              ["all", `Tất cả ${files.length}`],
              ["raw", `Chưa convert ${files.filter((node) => node.supported && !node.mdRelPath).length}`],
              ["done", `Đã convert ${files.filter((node) => !!node.mdRelPath || node.standaloneMd).length}`],
            ] as const
          ).map(([value, label]) => (
            <button
              type="button"
              aria-pressed={filter === value}
              className={filter === value ? "active" : ""}
              key={value}
              onClick={() => setFilter(value)}
            >
              {label}
            </button>
          ))}
        </div>
        {!!selected.length && <span>{selected.length} file được chọn</span>}
      </div>

      {!visible.length ? (
        <div className="library-empty">
          <FileCheck2 size={30} />
          <strong>Không có tài liệu ở trạng thái này.</strong>
          <span>Đổi bộ lọc hoặc tải thêm file để tiếp tục.</span>
        </div>
      ) : (
        <div className="library-grid">
          {visible.map((node) => {
            const raw = node.supported && !node.mdRelPath;
            const processing = jobs.some(
              (job) =>
                job.relPath === node.relPath &&
                (job.status === "queued" || job.status === "running"),
            );
            const checked = selected.includes(node.relPath);
            return (
              <article
                className={`library-card ${checked ? "selected" : ""}`}
                key={node.relPath}
              >
                <button
                  type="button"
                  className="library-select"
                  aria-label={`${checked ? "Bỏ chọn" : "Chọn"} ${node.name}`}
                  aria-pressed={checked}
                  onClick={() => toggle(node.relPath)}
                >
                  <CheckSquare size={15} />
                </button>
                <button
                  type="button"
                  className="library-card-main"
                  onClick={() => openNode(node)}
                >
                  <span className="library-file-icon">{fileIcon(node, { size: 24 })}</span>
                  <span className="library-name">{node.name}</span>
                  <span className="library-folder">{folderLabel(node.relPath)}</span>
                  <span
                    className={`status-chip ${
                      processing ? "processing" : raw ? "raw" : "done"
                    }`}
                  >
                    {processing ? "Đang convert" : raw ? "Chưa convert" : "Đã có Markdown"}
                  </span>
                </button>
              </article>
            );
          })}
        </div>
      )}

      {!!selected.length && (
        <div className="batch-bar">
          <div>
            <CheckSquare size={16} />
            <span>
              Đã chọn <b>{selected.length}</b> file
            </span>
          </div>
          <Button variant="ghost" size="sm" onClick={() => setSelected([])}>
            Bỏ chọn
          </Button>
          <Button
            variant="primary"
            size="sm"
            icon={<RefreshCw size={14} />}
            disabled={!selectedRaw.length}
            onClick={() => {
              enqueueConversions(selectedRaw);
              setSelected([]);
            }}
          >
            Convert {selectedRaw.length || ""}
          </Button>
        </div>
      )}
    </section>
  );
}
