# Simplified Technical English deviations

This project's documents follow Simplified Technical English (ASD-STE100).
Some content cannot follow it without losing precision. This file records
every known case.

| File | Section | Deviation | Reason |
|---|---|---|---|
| `SPEC.md` | §1.3 | Uses the RFC 2119 keywords MUST, MUST NOT, SHOULD, SHOULD NOT, and MAY with their RFC 2119 meaning, instead of plain-language equivalents. Does not use SHALL, since ASD-STE100 does not approve it and MUST carries the same meaning. | A specification needs a small, fixed, unambiguous vocabulary for "required" versus "optional" that a plain-language paraphrase would blur across a long document. |
| `SPEC.md` | throughout, especially §1.4, §3.1, §4.3, §5.3 | Mathematical and set notation (for example `V(log, spec) -> Verdict`, the `lc(e) = Σ ...` formula) is kept as notation rather than converted fully to prose. A plain-language sentence follows the notation on its first use. | Some rules are precisely and unambiguously stated only in notation; a prose-only restatement would be longer and less exact, not clearer. |
| `SPEC.md`, `DESIGN.md`, and code comments | throughout | A Rust code identifier (for example `snake_case` names, `::` paths, `LogBundle`, `Verdict`) keeps its exact spelling and is not reworded or translated. | An identifier is not prose; changing it would make it stop matching the actual code. |
| `SPEC.md` | §5.3 | Byte-layout tables use exact field names and byte counts in table cells; the plain-language sentence rules (word count, sentence structure) do not apply inside a table cell. | A byte layout is a specification artifact, not a sentence, and needs to be exact rather than readable as prose. |
