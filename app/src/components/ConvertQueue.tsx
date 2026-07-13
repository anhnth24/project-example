import { Check, LoaderCircle, RefreshCw, RotateCcw, Trash2, XCircle } from "lucide-react";
import { fileIcon } from "../lib/icons";
import type { FsNode } from "../lib/types";
import { useStore } from "../state/store";
import { Button, IconButton } from "./ui";

const statusLabel = {
  queued: "Đang chờ",
  running: "Đang convert",
  done: "Hoàn tất",
  error: "Có lỗi",
} as const;

export function ConvertQueue({ onClose }: { onClose: () => void }) {
  const jobs = useStore((state) => state.jobs);
  const retryJob = useStore((state) => state.retryJob);
  const clearFinishedJobs = useStore((state) => state.clearFinishedJobs);
  const active = jobs.filter((job) => job.status === "queued" || job.status === "running");

  return (
    <aside className="convert-queue" aria-label="Hàng đợi convert">
      <header>
        <div>
          <span className="eyebrow">Xử lý nền</span>
          <strong>
            {active.length ? `${active.length} file đang xử lý` : "Hàng đợi convert"}
          </strong>
        </div>
        <IconButton label="Đóng hàng đợi" onClick={onClose}>
          <XCircle size={15} />
        </IconButton>
      </header>

      <div className="queue-list" role="status" aria-live="polite">
        {!jobs.length && (
          <div className="queue-empty">Không có file nào trong hàng đợi.</div>
        )}
        {jobs.map((job) => {
          const iconNode: FsNode = {
            name: job.name,
            relPath: job.relPath,
            kind: job.kind,
            isDir: false,
            supported: true,
            mdRelPath: null,
            standaloneMd: false,
            children: [],
          };
          return (
            <div className={`queue-item queue-${job.status}`} key={job.id}>
              <span className="queue-file-icon">{fileIcon(iconNode, { size: 14 })}</span>
              <span className="queue-copy">
                <b>{job.name}</b>
                <small>{job.error ?? statusLabel[job.status]}</small>
                {job.status === "running" && (
                  <span className="queue-progress indeterminate" />
                )}
              </span>
              <span className="queue-status-icon" aria-label={statusLabel[job.status]}>
                {job.status === "running" && <LoaderCircle className="spin" size={14} />}
                {job.status === "queued" && <RefreshCw size={14} />}
                {job.status === "done" && <Check size={14} />}
                {job.status === "error" && (
                  <IconButton label={`Thử lại ${job.name}`} onClick={() => retryJob(job.id)}>
                    <RotateCcw size={13} />
                  </IconButton>
                )}
              </span>
            </div>
          );
        })}
      </div>

      {!!jobs.length && (
        <footer>
          <span>Convert chạy nền — bạn vẫn có thể tiếp tục làm việc.</span>
          {jobs.some((job) => job.status === "done" || job.status === "error") && (
            <Button
              variant="ghost"
              size="sm"
              icon={<Trash2 size={12} />}
              onClick={clearFinishedJobs}
            >
              Dọn xong
            </Button>
          )}
        </footer>
      )}
    </aside>
  );
}
