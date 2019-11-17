// Ported from js-yaml v3.13.1:
// https://github.com/nodeca/js-yaml/commit/665aadda42349dcae869f12040d9b10ef18d12da
// Copyright 2018-2019 the Deno authors. All rights reserved. MIT license.

import { Type } from "../type.ts";
import { Any } from "../utils.ts";

export const seq = new Type("tag:yaml.org,2002:seq", {
  construct(data): Any {
    return data !== null ? data : [];
  },
  kind: "sequence"
});
