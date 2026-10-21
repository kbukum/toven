// Imports the built output of `@acme/core`, so it passes only after the
// dependency's `build` ran.
import { expect, test } from "bun:test";
import { answer } from "@acme/core";

test("core answers", () => {
  expect(answer).toBe(42);
});
