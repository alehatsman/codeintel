/** Ambient declarations, which a declaration file is made of. */

declare namespace JSX {
  interface IntrinsicElements {
    [name: string]: unknown;
  }
}

declare module "legacy-logger" {
  export function log(message: string): void;
  export const level: number;
}

declare const VERSION: string;
