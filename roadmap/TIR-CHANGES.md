# TIR changes

## 26.11.1

Additions: `tirc analyze` package reports, `tirc package` package writing,
driver source and license validation, unique platform and dependency lists,
and portable package entry-path rules.

Removals: None.

Semantic changes: None.

Encoding changes: IMPORTS function entities carry sorted effective
`@Requires` entitlement names and LibTessera-implied entitlement names after
their type index.

Producer notes: Emit both entitlement lists on RIDL function imports. Leave
them empty on other entities. Use the effective `@Requires` annotations from
the function and its containing type; populate the LibTessera list from
LibTessera.md. Preserve the input lists when re-encoding IMPORTS.
