import { Columns2, FolderPlus, Upload } from "lucide-react";
import { Button } from "./ui";

const planetUrl = new URL(
  "../../../re-design/project/assets/planet-magician.png",
  import.meta.url,
).href;

export function HomeView({
  onUpload,
  onDocuments,
}: {
  onUpload: () => void;
  onDocuments: () => void;
}) {
  const steps = [
    {
      icon: <FolderPlus size={18} />,
      title: "Tổ chức trong DATA",
      description: "Tạo cây thư mục theo từng nhóm nghiệp vụ và dự án.",
      action: onDocuments,
    },
    {
      icon: <Upload size={18} />,
      title: "Tải file gốc",
      description: "PDF, Word, Excel, ảnh và audio được đưa vào hàng đợi.",
      action: onUpload,
    },
    {
      icon: <Columns2 size={18} />,
      title: "Đối chiếu và sửa",
      description: "So bản convert gốc với Markdown theo từng khối liên kết.",
      action: onDocuments,
    },
  ];

  return (
    <section className="home-view">
      <img className="home-planet" src={planetUrl} alt="" />
      <span className="eyebrow">Offline document workbench</span>
      <h1>Markhand</h1>
      <p>
        Biến mọi tài liệu nguồn thành Markdown sạch để bàn giao cho Dev — dành cho
        BA và PM.
      </p>
      <div className="home-cards">
        {steps.map((step) => (
          <button type="button" key={step.title} onClick={step.action}>
            <span>{step.icon}</span>
            <strong>{step.title}</strong>
            <small>{step.description}</small>
          </button>
        ))}
      </div>
      <Button variant="primary" icon={<Upload size={15} />} onClick={onUpload}>
        Tải file
      </Button>
      <small className="home-drop-hint">
        hoặc kéo-thả file vào bất kỳ đâu trong cửa sổ.
      </small>
    </section>
  );
}
