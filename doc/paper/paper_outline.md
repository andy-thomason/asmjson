# Asmjson - A SIMD parallel JSON parser for AVX512BW in Rust and x86_64 assembly.

This document is hand-generated as a guide to the agent (Claude 4.6).

## Abstract

JSON is a commonly used format for sending data across networks. It has fast implementations
in web browsers but is often a bottleneck when reading. Writing JSON quickly is fairly simple
and is well supported by by libraries such as serde_json in Rust, but reading JSON is a much
harder problem.

## Prior art

*Add a section here about prior art listing well known JSON parsers such as serde_json, sonic-rs,
RapidJSON, simd-json. Describe the problem of skipping whitespace and string contents and the problem
of building an efficient DOM. Compare DOM vs SAX*

Add multiple references to both github and any associated papers.


## Benchmarking section

*Add a section here describing our benchmarking results based on the README.md data*

Say how we are faster than sonic-rs and much faster than serde_json, even when driving a serde interface
as in the serde example.

## Our method

Describe our method of decoding 64 byte blocks at a time using zmm registers and masks.
Show how this can be used to advance states rapidly using vcmp, kmov and tzcnt.

Describe the SAX and DOM variants and list the JSONWriter trait.

Make some code listings of the vcmp section of parse_json_zmm_sax.S and show an example
of the tzcnt in action for whitespace in a separate listing. Make all the listings floats
in Tex.

Describe how we have generated two versions in x86_64 asm for AVX512BW architectures such as Zen 5
and a generic version in Rust that uses SWAR bit tricks which is also very fast and will run on
legacy hardware.

Describe our tape-based DOM and how this is generated directly from the assembly code in the zmm_dom case.
The use of a tape avoids calls to malloc and makes a big difference to the performance over the serde_json
Value which uses multiple memory allocations.

## Further work

Describe how we might be able to skip multiple states at once using bit-combination techniques
to make this even faster, for example in the string array case if there are no escapes or whitespace to
skip strings and commas using a series of inline tzcnt and shift operations.

Describe how the technique may also be used to parse CSV and TSV files as well as bioinformatics data formats.

Describe how we could improve the DOM tape format, using bytes and ULEB instead of a fixed record length.

We could also make a procedural macro which uses the SAX interface to directly generate nested structures
and arrays without using the DOM improving on the serde_json performance but breaking compatibility with
serde.

## Caveats

asmjson is permissive, ignoring some JSON rules. For example it allows control characters in whitespace
and strings. However it should parse all known well-formed JSON. It is possible to reject strings containg
control characters and sweep the entire file for illegal characters and non-conforming UTF8 before parsing
if this level of compatibility is desired, however many existing JSON files such as LLM model definition files
would fail this test.

## Conclusion

We have produced what we think is the fastest JSON parser available in the Rust ecosystem and possibly
in all low level languages.

