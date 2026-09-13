/** A component, so a JSX tag has something to be a use of. */
export function Row(props: { label: string; ok: boolean }) {
  return <span>{props.ok ? props.label : ""}</span>;
}
