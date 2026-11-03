// Types of the wasm-bindgen glue of crates/webcore (generated into src/wasm/pkg by scripts/wasm.mjs, not committed),
// imported as "webcore-pkg" (a Vite alias in vite.config.ts). Keep in step with crates/webcore/src/lib.rs.

declare module "webcore-pkg" {
  /** Fetch, compile and instantiate the wasm (by default from the URL next to the glue). */
  export default function init(opts?: { module_or_path?: RequestInfo | URL | Response | BufferSource | WebAssembly.Module }): Promise<unknown>;

  export class PackedPreview {
    free(): void;
    meta(): string;
    take_xy(): Float32Array;
    take_t(): Float32Array;
    take_info(): Uint32Array;
    take_pass(): Uint32Array;
    take_power(): Float32Array;
  }

  export class Core {
    constructor();
    free(): void;
    /** Returns the document's revision. Throws on JSON that is not a document. */
    set_document(json: string): number;
    set_context(json: string): void;
    preview(): PackedPreview;
    stats(): string;
    fill_region_preview(params: string): string;
  }
}
