/** The database layer. Nothing here may import from ui/. */
export function open(url: string): boolean {
  return url !== "";
}
