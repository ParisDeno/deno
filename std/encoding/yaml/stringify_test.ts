// Ported from js-yaml v3.13.1:
// https://github.com/nodeca/js-yaml/commit/665aadda42349dcae869f12040d9b10ef18d12da
// Copyright 2018-2019 the Deno authors. All rights reserved. MIT license.
// Copyright 2018-2019 the Deno authors. All rights reserved. MIT license.

import { test } from "../../testing/mod.ts";
import { assertEquals } from "../../testing/asserts.ts";
import { stringify } from "./stringify.ts";

test({
  name: "stringified correctly",
  fn(): void {
    const FIXTURE = {
      foo: {
        bar: true,
        test: [
          "a",
          "b",
          {
            a: false
          },
          {
            a: false
          }
        ]
      },
      test: "foobar"
    };

    const ASSERTS = `foo:
  bar: true
  test:
    - a
    - b
    - a: false
    - a: false
test: foobar
`;

    assertEquals(stringify(FIXTURE), ASSERTS);
  }
});
