// Imports the built output of `@acme/core`, so it passes only after the
// dependency's `build` ran.
const assert = require("node:assert");
const { answer } = require("@acme/core");

assert.strictEqual(answer, 42);
console.log("tested @acme/app");
