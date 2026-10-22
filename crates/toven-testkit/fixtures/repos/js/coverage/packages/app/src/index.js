import { sign } from "@acme/core";

export function label(value) {
  return sign(value) < 0 ? "negative" : "positive";
}
