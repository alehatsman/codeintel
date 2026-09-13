import type { Handler } from "../kinds";

/** One entry in the store. */
export class Entry {
  value: string;

  constructor(value: string) {
    this.value = value;
  }

  describe(): string {
    return this.value.toUpperCase();
  }
}

/** A tiny key-value store, implementing an interface from another file. */
export class Store implements Handler {
  private entries = new Map<string, Entry>();

  /**
   * Read one entry.
   *
   * Two lines of documentation, to prove newlines survive.
   */
  get(key: string): Entry | undefined {
    return this.entries.get(key);
  }

  put(key: string, value: string): void {
    this.entries.set(key, new Entry(value));
  }

  handle(key: string): boolean {
    return this.get(key) !== undefined;
  }

  #evict(): void {
    this.entries.clear();
  }
}

/** A free function that calls a method, so `calls` has an edge to find. */
export function warm(store: Store): Entry | undefined {
  return store.get("warm");
}
