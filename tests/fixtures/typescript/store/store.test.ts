import { Store, warm } from "./store";

/** A test file, so is_test(F) has something to match on by path. */
export function testWarm(): boolean {
  return warm(new Store()) === undefined;
}
