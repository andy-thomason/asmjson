# asmjson — developer notes

## Internal state machine

Each byte of the input is labelled below with the state that handles it.
States that skip whitespace via `trailing_zeros` handle both the whitespace
bytes **and** the following dispatch byte in the same loop iteration.

```text
{ "key1" : "value1" , "key2": [123, 456 , 768], "key3" : { "nested_key" : true} }
VOOKKKKKDDCCSSSSSSSFFOOKKKKKDCCRAAARRAAAFRRAAAFOOKKKKKDDCCOOKKKKKKKKKKKDDCCAAAAFF
```

State key:
* `V` = `ValueWhitespace` — waiting for the first byte of any value
* `O` = `ObjectStart`     — after `{` or `,` in an object; skips whitespace, expects `"` or `}`
* `K` = `KeyChars`        — inside a quoted key; bulk-skipped via the backslash/quote masks
* `D` = `KeyEnd`          — after closing `"` of a key; skips whitespace, expects `:`
* `C` = `AfterColon`      — after `:`; skips whitespace, dispatches to the value type
* `S` = `StringChars`     — inside a quoted string value; bulk-skipped via the backslash/quote masks
* `F` = `AfterValue`      — after any complete value; skips whitespace, expects `,`/`}`/`]`
* `R` = `ArrayStart`      — after `[` or `,` in an array; skips whitespace, dispatches value
* `A` = `AtomChars`       — inside a number, `true`, `false`, or `null`

A few things to notice in the annotation:

* `OO`: `ObjectStart` eats the space *and* the opening `"` of a key in one
  shot via the `trailing_zeros` whitespace skip.
* `DD` / `CC`: `KeyEnd` eats the space *and* `:` together; `AfterColon`
  eats the space *and* the value-start byte — structural punctuation costs
  no extra iterations.
* `SSSSSSS`: `StringChars` covers the entire `value1"` run including the
  closing quote (bulk AVX-512 skip + dispatch in one pass through the chunk).
* `RAAARRAAAFRRAAAF`: inside the array `[123, 456 , 768]` each `R` covers
  the skip-to-digit hop; `AAA` covers the digit characters plus their
  terminating `,` / space / `]`.
* `KKKKKKKKKKK` (11 bytes): the 10-character `nested_key` body *and* its
  closing `"` are all handled by `KeyChars` in one bulk-skip pass.
