// Storage of the design-only build: a small key-value interface over IndexedDB (`IdbStore`), with an in-memory one
// (`MemoryStore`) for tests and for browser windows where IndexedDB is not available (the page then says that
// nothing outlives the tab).
//
// One IndexedDB object store, "kv", keyed by `[store, key]`, holds every logical store, so webcore can add stores
// without a database upgrade, and one `batch` (a single transaction) can change several stores at once: a project's
// listing and its bytes are written together or not at all. Values are anything structured clone takes; bytes are
// kept as Uint8Array (Blobs in IndexedDB have had bugs in Safari).

/** One write: `value` null (or undefined) deletes the key. */
export interface KvWrite {
  store: string;
  key: string;
  value: unknown;
}

export interface KeyValueStore {
  /** "indexeddb" or "memory". */
  readonly kind: "indexeddb" | "memory";
  get<T = unknown>(store: string, key: string): Promise<T | undefined>;
  /** Every `[key, value]` of `store`, sorted by key. */
  entries<T = unknown>(store: string): Promise<[string, T][]>;
  keys(store: string): Promise<string[]>;
  /** Every `[store, key, value]` of the stores whose name starts with `prefix`, sorted. */
  scan<T = unknown>(prefix: string): Promise<[string, string, T][]>;
  /** Apply the writes in order, all or nothing. A full store throws `StorageFullError`. */
  batch(writes: KvWrite[]): Promise<void>;
  close(): void;
}

/** The browser refused to store more (quota). */
export class StorageFullError extends Error {
  constructor(cause?: unknown) {
    super("The browser's storage for this site is full: download a copy of your work, then delete old projects or autosaves");
    this.name = "StorageFullError";
    if (cause !== undefined) (this as { cause?: unknown }).cause = cause;
  }
}

function clone<T>(v: T): T {
  return typeof structuredClone === "function" ? structuredClone(v) : v;
}

/** Everything in memory (tests; windows without IndexedDB). Values are cloned in and out, as IndexedDB does. */
export class MemoryStore implements KeyValueStore {
  readonly kind = "memory" as const;
  private readonly data = new Map<string, Map<string, unknown>>();
  /** Bytes that may be stored at most (tests of the full-storage path); Infinity by default. */
  quotaBytes = Infinity;

  async get<T = unknown>(store: string, key: string): Promise<T | undefined> {
    const v = this.data.get(store)?.get(key);
    return v === undefined ? undefined : clone(v as T);
  }

  async entries<T = unknown>(store: string): Promise<[string, T][]> {
    const m = this.data.get(store);
    if (!m) return [];
    return [...m.entries()].sort(([a], [b]) => (a < b ? -1 : a > b ? 1 : 0)).map(([k, v]) => [k, clone(v as T)]);
  }

  async keys(store: string): Promise<string[]> {
    return [...(this.data.get(store)?.keys() ?? [])].sort();
  }

  async scan<T = unknown>(prefix: string): Promise<[string, string, T][]> {
    const out: [string, string, T][] = [];
    for (const name of [...this.data.keys()].filter((n) => n.startsWith(prefix)).sort()) {
      for (const [k, v] of await this.entries<T>(name)) out.push([name, k, v]);
    }
    return out;
  }

  async batch(writes: KvWrite[]): Promise<void> {
    // All or nothing: work on copies of the touched stores, then swap them in.
    const next = new Map<string, Map<string, unknown>>();
    for (const w of writes) {
      let m = next.get(w.store);
      if (!m) {
        m = new Map(this.data.get(w.store) ?? []);
        next.set(w.store, m);
      }
      if (w.value === null || w.value === undefined) m.delete(w.key);
      else m.set(w.key, clone(w.value));
    }
    if (this.quotaBytes !== Infinity) {
      let total = 0;
      for (const [name, m] of this.data) total += sizeOf(next.get(name) ?? m);
      for (const [name, m] of next) if (!this.data.has(name)) total += sizeOf(m);
      if (total > this.quotaBytes) throw new StorageFullError();
    }
    for (const [name, m] of next) this.data.set(name, m);
  }

  close(): void {}
}

/** Rough size of a store's values (bytes and strings), for MemoryStore's quota. */
function sizeOf(m: Map<string, unknown>): number {
  let n = 0;
  for (const v of m.values()) n += valueSize(v);
  return n;
}

export function valueSize(v: unknown): number {
  if (v instanceof Uint8Array) return v.byteLength;
  if (typeof v === "string") return v.length * 2;
  if (v && typeof v === "object") return JSON.stringify(v, (_k, x: unknown) => (x instanceof Uint8Array ? x.byteLength : x))?.length ?? 0;
  return 8;
}

// --- IndexedDB ---------------------------------------------------------------------------------------------------

/** Database name: prefixed, because every project site of one github.io account shares the origin's storage. */
export const DB_NAME = "openetch-design";
const DB_VERSION = 1;
const OS = "kv";

function req<T>(r: IDBRequest<T>): Promise<T> {
  return new Promise((resolve, reject) => {
    r.onsuccess = () => resolve(r.result);
    r.onerror = () => reject(r.error);
  });
}

function isQuota(e: unknown): boolean {
  const name = (e as { name?: unknown } | null)?.name;
  return name === "QuotaExceededError" || name === "NS_ERROR_DOM_QUOTA_REACHED";
}

/** The range of every `[store, key]` with a string key (arrays sort after strings in IndexedDB). */
function storeRange(store: string): IDBKeyRange {
  return IDBKeyRange.bound([store], [store, []]);
}

export class IdbStore implements KeyValueStore {
  readonly kind = "indexeddb" as const;
  private readonly db: IDBDatabase;
  private constructor(db: IDBDatabase) {
    this.db = db;
    // A newer version opened in another tab: close so its upgrade is not blocked; this tab then stops storing.
    db.onversionchange = () => db.close();
  }

  /** Open (or create) the database. Rejects when IndexedDB is missing or refused (some private windows). */
  static async open(name = DB_NAME, factory: IDBFactory | undefined = globalThis.indexedDB): Promise<IdbStore> {
    if (!factory) throw new Error("IndexedDB is not available");
    const r = factory.open(name, DB_VERSION);
    r.onupgradeneeded = () => {
      if (!r.result.objectStoreNames.contains(OS)) r.result.createObjectStore(OS);
    };
    const db = await new Promise<IDBDatabase>((resolve, reject) => {
      r.onsuccess = () => resolve(r.result);
      r.onerror = () => reject(r.error);
      r.onblocked = () => reject(new Error("the storage is held by an older OpenEtch tab: close it and reload"));
    });
    return new IdbStore(db);
  }

  async get<T = unknown>(store: string, key: string): Promise<T | undefined> {
    const tx = this.db.transaction(OS, "readonly");
    return (await req(tx.objectStore(OS).get([store, key]))) as T | undefined;
  }

  async entries<T = unknown>(store: string): Promise<[string, T][]> {
    const os = this.db.transaction(OS, "readonly").objectStore(OS);
    const [keys, values] = await Promise.all([req(os.getAllKeys(storeRange(store))), req(os.getAll(storeRange(store)))]);
    return keys.map((k, i) => [String((k as [string, string])[1]), values[i] as T]);
  }

  async keys(store: string): Promise<string[]> {
    const os = this.db.transaction(OS, "readonly").objectStore(OS);
    return (await req(os.getAllKeys(storeRange(store)))).map((k) => String((k as [string, string])[1]));
  }

  async scan<T = unknown>(prefix: string): Promise<[string, string, T][]> {
    const os = this.db.transaction(OS, "readonly").objectStore(OS);
    const range = IDBKeyRange.bound([prefix], [`${prefix}￿`, []]);
    const [keys, values] = await Promise.all([req(os.getAllKeys(range)), req(os.getAll(range))]);
    return keys.map((k, i) => {
      const [s, key] = k as [string, string];
      return [String(s), String(key), values[i] as T] as [string, string, T];
    });
  }

  async batch(writes: KvWrite[]): Promise<void> {
    if (writes.length === 0) return;
    let tx: IDBTransaction;
    try {
      tx = this.db.transaction(OS, "readwrite");
    } catch (e) {
      throw isQuota(e) ? new StorageFullError(e) : e;
    }
    const os = tx.objectStore(OS);
    const done = new Promise<void>((resolve, reject) => {
      tx.oncomplete = () => resolve();
      tx.onerror = () => reject(isQuota(tx.error) ? new StorageFullError(tx.error) : tx.error);
      tx.onabort = () => reject(isQuota(tx.error) ? new StorageFullError(tx.error) : (tx.error ?? new Error("the storage transaction was aborted")));
    });
    try {
      for (const w of writes) {
        if (w.value === null || w.value === undefined) os.delete([w.store, w.key]);
        else os.put(w.value, [w.store, w.key]);
      }
    } catch (e) {
      tx.abort();
      await done.catch(() => undefined);
      throw isQuota(e) ? new StorageFullError(e) : e;
    }
    await done;
  }

  close(): void {
    this.db.close();
  }
}

/** IndexedDB when it works, else memory (the caller tells the person). */
export async function openStore(name = DB_NAME): Promise<KeyValueStore> {
  try {
    return await IdbStore.open(name);
  } catch {
    return new MemoryStore();
  }
}
