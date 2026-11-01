// File drag and drop on the whole window. Every drag that carries files has its default action blocked at the window
// (otherwise the browser opens or navigates to the dropped file and the app is gone), even when it ends outside the
// canvas or over a dialog. While files are over the window a full-window overlay says what a drop will do; an
// enter/leave counter keeps it from flickering as the pointer crosses child elements. Drags without files (text,
// links) are ignored. The routing itself is pure, in api/drop.ts.

import { useEffect, useRef, useState } from "react";
import { carriesFiles, dragDepth, overlayLabel } from "../api/drop.ts";
import { Icon } from "./ui.tsx";
import { useT } from "../workspace/usePrefs.ts";

/** Listen for file drags on the window; `onFiles` gets the dropped files. Returns the overlay label, or null. */
export function useFileDrop(onFiles: (files: File[]) => void): string | null {
  const [label, setLabel] = useState<string | null>(null);
  const depth = useRef(0);
  const onFilesRef = useRef(onFiles);
  onFilesRef.current = onFiles;

  useEffect(() => {
    const files = (e: DragEvent) => carriesFiles(e.dataTransfer ? Array.from(e.dataTransfer.types) : null);
    const onEnter = (e: DragEvent) => {
      if (!files(e)) return;
      e.preventDefault();
      depth.current = dragDepth(depth.current, "enter");
      const items = e.dataTransfer ? Array.from(e.dataTransfer.items).filter((i) => i.kind === "file") : [];
      setLabel(overlayLabel(items.map((i) => ({ type: i.type }))));
    };
    const onOver = (e: DragEvent) => {
      if (!files(e)) return;
      // Without this the drop event never fires and the browser opens the file instead.
      e.preventDefault();
      if (e.dataTransfer) e.dataTransfer.dropEffect = "copy";
    };
    const onLeave = (e: DragEvent) => {
      if (!files(e)) return;
      depth.current = dragDepth(depth.current, "leave");
      if (depth.current === 0) setLabel(null);
    };
    const onDrop = (e: DragEvent) => {
      if (!files(e)) return;
      e.preventDefault();
      depth.current = dragDepth(depth.current, "drop");
      setLabel(null);
      const list = e.dataTransfer ? Array.from(e.dataTransfer.files) : [];
      if (list.length) onFilesRef.current(list);
    };
    // A drag cancelled with Esc outside the page never sends a matching leave; reset when the window loses focus.
    const reset = () => {
      depth.current = 0;
      setLabel(null);
    };
    window.addEventListener("dragenter", onEnter);
    window.addEventListener("dragover", onOver);
    window.addEventListener("dragleave", onLeave);
    window.addEventListener("drop", onDrop);
    window.addEventListener("blur", reset);
    return () => {
      window.removeEventListener("dragenter", onEnter);
      window.removeEventListener("dragover", onOver);
      window.removeEventListener("dragleave", onLeave);
      window.removeEventListener("drop", onDrop);
      window.removeEventListener("blur", reset);
    };
  }, []);

  return label;
}

/** The full-window "Drop to import" overlay. It ignores pointer events so the drop reaches the window listeners. */
export function DropOverlay({ label }: { label: string }) {
  const t = useT();
  return (
    <div className="drop-overlay" aria-live="polite">
      <div className="drop-card">
        <Icon name="import" size={40} />
        <div className="drop-title">{label}</div>
        <div className="drop-hint">{t("fd.svgDxfPdfAiEpsHpgl")}</div>
      </div>
    </div>
  );
}
