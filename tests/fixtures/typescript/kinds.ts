/**
 * One of every kind our TypeScript tags.scm can emit.
 *
 * The fixture behind the kind-fidelity test in docs/plan.md M5: upstream's
 * tags.scm captures only ambient forms, so none of this would be a definition.
 */

export const BANNER = "codeintel";
let retries = 3;
var legacy: string | undefined;

/** An arrow binding is a function, so `calls` can reach it. */
export const describe = (config: Config): string => config.describe();

function sealed(target: unknown): void {
  void target;
}

/** A documented interface. */
export interface Handler {
  /** Handle one key. */
  handle(key: string): boolean;
  name?: string;
}

export enum Mode {
  Fast,
  Slow = 2,
}

export type Key = string;

/**
 * A class, with a documented field and a documented method.
 *
 * Two lines of documentation, to prove newlines survive.
 */
@sealed
export class Config implements Handler {
  /** The limit. */
  limit = 64;
  private quiet = false;
  protected attempts = 3;
  #secret = "s";
  static readonly DEFAULT = 64;

  constructor(limit: number) {
    this.limit = limit;
  }

  handle(key: string): boolean {
    return this.limit > 0 && key.length > 0 && !this.quiet;
  }

  describe(): string {
    return BANNER + this.attempts;
  }

  get size(): number {
    return this.limit;
  }

  set size(value: number) {
    this.limit = value;
  }

  #check(): boolean {
    return this.#secret !== "";
  }
}

export abstract class Base {
  abstract run(): void;
}

export function parse(input: string): number;
export function parse(input: number): number;
export function parse(input: string | number): number {
  return Number(input);
}

export function* ids(): Generator<number> {
  yield retries;
}

function outer(): number {
  const local = 1;
  function inner(): number {
    return retries + local;
  }
  return inner();
}

export namespace Limits {
  export const MAX = 100;
  const hidden = () => outer();
}

// A non-ASCII character before a definition on its own line. scip-typescript
// counts that column in UTF-16 and declares no encoding, so the second
// definition's SCIP occurrence is ambiguous and skipped (#21).
export const WIDE = "é"; export const AFTER_WIDE = legacy;
