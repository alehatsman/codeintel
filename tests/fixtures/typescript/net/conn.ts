/**
 * A second connection layer, exporting a function with the same name as
 * db/conn's. That ambiguity is the whole point: tier A cannot decide which
 * `open` a caller in a third file means, and gives up rather than guessing.
 */
export function open(url: string): boolean {
  return url.startsWith("tcp://");
}
