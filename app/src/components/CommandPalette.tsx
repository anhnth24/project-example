import { useEffect, useMemo, useRef, useState, type ReactNode } from "react";
import { FileSearch, Home, LayoutGrid, Search, Settings } from "lucide-react";
import { fileIcon } from "../lib/icons";
import { flattenFiles, folderLabel, normalizeSearch } from "../lib/tree";
import { useStore } from "../state/store";

interface PaletteItem {
  id: string;
  label: string;
  meta: string;
  icon: ReactNode;
  run: () => void;
}

export function CommandPalette({
  onClose,
  onOpenSettings,
}: {
  onClose: () => void;
  onOpenSettings: () => void;
}) {
  const tree = useStore((state) => state.tree);
  const openNode = useStore((state) => state.openNode);
  const setView = useStore((state) => state.setView);
  const [query, setQuery] = useState("");
  const [selected, setSelected] = useState(0);
  const inputRef = useRef<HTMLInputElement>(null);
  const panelRef = useRef<HTMLDivElement>(null);

  const items = useMemo<PaletteItem[]>(() => {
    const normalized = normalizeSearch(query);
    const commands: PaletteItem[] = [
      {
        id: "home",
        label: "Mở trang chủ",
        meta: "Điều hướng",
        icon: <Home size={15} />,
        run: () => setView("home"),
      },
      {
        id: "library",
        label: "Mở thư viện",
        meta: "Điều hướng",
        icon: <LayoutGrid size={15} />,
        run: () => setView("library"),
      },
      {
        id: "settings",
        label: "Cài đặt convert",
        meta: "Lệnh",
        icon: <Settings size={15} />,
        run: onOpenSettings,
      },
    ];
    const files = flattenFiles(tree).map<PaletteItem>((node) => ({
      id: `file:${node.relPath}`,
      label: node.name,
      meta: folderLabel(node.relPath),
      icon: fileIcon(node, { size: 15 }),
      run: () => openNode(node),
    }));
    return [...commands, ...files]
      .filter(
        (item) =>
          !normalized ||
          normalizeSearch(item.label).includes(normalized) ||
          normalizeSearch(item.meta).includes(normalized),
      )
      .slice(0, 30);
  }, [onOpenSettings, openNode, query, setView, tree]);

  useEffect(() => {
    const previous = document.activeElement as HTMLElement | null;
    inputRef.current?.focus();
    return () => previous?.focus();
  }, []);

  useEffect(() => {
    setSelected(0);
  }, [query]);

  function run(item: PaletteItem | undefined) {
    if (!item) return;
    item.run();
    onClose();
  }

  return (
    <div
      className="palette-backdrop"
      onMouseDown={(event) => event.target === event.currentTarget && onClose()}
    >
      <div
        ref={panelRef}
        className="command-palette"
        role="dialog"
        aria-modal="true"
        aria-label="Tìm kiếm và chạy lệnh"
        onKeyDown={(event) => {
          if (event.key !== "Tab") return;
          const focusable = panelRef.current?.querySelectorAll<HTMLElement>(
            "input, button:not(:disabled)",
          );
          if (!focusable?.length) return;
          const first = focusable[0];
          const last = focusable[focusable.length - 1];
          if (event.shiftKey && document.activeElement === first) {
            event.preventDefault();
            last.focus();
          } else if (!event.shiftKey && document.activeElement === last) {
            event.preventDefault();
            first.focus();
          }
        }}
      >
        <label className="palette-input">
          <Search size={17} />
          <input
            ref={inputRef}
            value={query}
            onChange={(event) => setQuery(event.target.value)}
            onKeyDown={(event) => {
              if (event.key === "ArrowDown") {
                event.preventDefault();
                setSelected((value) => Math.min(value + 1, items.length - 1));
              } else if (event.key === "ArrowUp") {
                event.preventDefault();
                setSelected((value) => Math.max(value - 1, 0));
              } else if (event.key === "Enter") {
                event.preventDefault();
                run(items[selected]);
              } else if (event.key === "Escape") {
                event.preventDefault();
                onClose();
              }
            }}
            placeholder="Tìm trong DATA hoặc chạy lệnh…"
            aria-label="Tìm trong DATA hoặc chạy lệnh"
            role="combobox"
            aria-expanded="true"
            aria-controls="command-palette-results"
            aria-activedescendant={
              items[selected] ? `palette-option-${selected}` : undefined
            }
          />
          <kbd>Esc</kbd>
        </label>
        <div id="command-palette-results" className="palette-results" role="listbox">
          {!items.length && (
            <div className="palette-empty">
              <FileSearch size={22} />
              Không có kết quả cho “{query}”.
            </div>
          )}
          {items.map((item, index) => (
            <button
              type="button"
              role="option"
              id={`palette-option-${index}`}
              aria-selected={selected === index}
              className={selected === index ? "selected" : ""}
              key={item.id}
              onMouseEnter={() => setSelected(index)}
              onClick={() => run(item)}
            >
              <span>{item.icon}</span>
              <b>{item.label}</b>
              <small>{item.meta}</small>
            </button>
          ))}
        </div>
        <footer>
          <span>↑↓ Chọn</span>
          <span>↵ Mở</span>
          <span>Esc Đóng</span>
        </footer>
      </div>
    </div>
  );
}
