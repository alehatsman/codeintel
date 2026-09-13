import { open } from "../db/conn";

/** Start up. `open` is defined twice repo-wide, so only tier B resolves it. */
export function start(): boolean {
  return open("sqlite://memory");
}
