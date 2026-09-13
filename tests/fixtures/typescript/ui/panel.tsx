/**
 * The UI layer. This file deliberately violates the layering rule, and
 * docs/plan.md M5 requires that a conformance query over import/3 finds it and
 * that removing the import returns zero rows with status: ok.
 */
import { open } from "../db/conn";
import * as net from "../net/conn";
import { Row } from "./row";

export function Panel(props: { url: string }) {
  return (
    <div>
      <Row label={props.url} ok={open(props.url) || net.open(props.url)} />
    </div>
  );
}

export const Draw = () => <Panel url="sqlite://memory" />;
